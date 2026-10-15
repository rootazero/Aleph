//! Production parent-side managed launcher (#48/#53).
//!
//! Implements [`ManagedLaunchApi`] with real OS objects and runs it through
//! the existing strict ordering in [`launch_admitted`]:
//!
//! 1. exact network plan (before any OS object exists),
//! 2. single-process Job Object,
//! 3. per-run AppContainer profile + workspace ACEs, then the SERVICE itself
//!    created by THIS process with `CREATE_SUSPENDED` and stdio pipes (no
//!    `sandbox-init-windows` wrapper, so the PID is the service PID),
//! 4. identity (PID + creation time) read from the owned process handle,
//! 5. job assignment + kernel membership readback,
//! 6. network confinement install + readback for the run's package SID,
//! 7. resume.
//!
//! The resulting [`WindowsManagedProcess`] satisfies the [`ManagedProcess`]
//! contract: `wait` returns only after the service exited, the job is
//! confirmed empty and the confinement was removed (in that order). If the
//! job cannot be confirmed empty the confinement is deliberately leaked
//! (fail closed) and `wait` reports an error.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::Security::{SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::job::SandboxJob;
use super::managed_launch::native::{self, SuspendedProcess};
use super::managed_launch::{
    launch_admitted, AdmittedService, ConfinementCapabilities, ConfinementPlan, ManagedLaunchApi,
    NetworkRequest, RunSid, ServiceIdentity,
};
use crate::sandbox::command::SandboxError;
use crate::sandbox::managed::{ManagedExit, ManagedProcess, ManagedStdio};
use crate::sandbox::windows_init::{build_command_line, AppContainerProfile, ProcAttributeList};

const LOG_CONTEXT: &str = "aleph managed launcher";

/// Everything needed to create the service. Built by the driver from the
/// resolved `OsSandboxProfile`; never from model input directly.
#[derive(Debug, Clone)]
pub(crate) struct ManagedLaunchSpec {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: HashMap<String, String>,
    pub(crate) cwd: PathBuf,
    pub(crate) app_container_capabilities: Vec<String>,
    pub(crate) workspace: Option<String>,
    pub(crate) deny_read_globs: Vec<String>,
    pub(crate) max_memory_mb: Option<u64>,
}

/// Per-run network confinement keyed by the run's package SID. Production
/// is the WFP backend; tests substitute a recording backend.
pub(crate) trait ConfinementBackend: Send + 'static {
    type Net: Send + 'static;
    fn install(&mut self, plan: &ConfinementPlan, sid: &RunSid) -> Result<Self::Net, String>;
    fn verify(
        &mut self,
        net: &Self::Net,
        plan: &ConfinementPlan,
        sid: &RunSid,
    ) -> Result<bool, String>;
    fn remove(&mut self, net: Self::Net) -> Result<(), String>;
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Transfer ownership into a `std::fs::File` (closes on its drop).
    fn into_file(self) -> File {
        let h = self.0;
        std::mem::forget(self);
        // SAFETY: `h` is an owned, valid pipe handle; ownership moves to File.
        unsafe { File::from_raw_handle(h as _) }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: owned handle, closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// One anonymous pipe: `(child_end, parent_end)`. The child end is
/// inheritable, the parent end never is: both ends are created
/// non-inheritable and only the child end is flipped (so a concurrent
/// `CreateProcess(bInheritHandles=TRUE)` elsewhere can never pick up a parent
/// end; the child end is additionally pinned by `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`
/// for our own spawn).
fn pipe(child_reads: bool) -> Result<(OwnedHandle, OwnedHandle), String> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 0,
    };
    let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: valid out-pointers and attributes.
    if unsafe { CreatePipe(&mut read, &mut write, &sa, 0) } == 0 {
        // SAFETY: thread-local error read.
        return Err(format!("CreatePipe failed: {}", unsafe { GetLastError() }));
    }
    let (read, write) = (OwnedHandle(read), OwnedHandle(write));
    let (child, parent) = if child_reads {
        (read, write)
    } else {
        (write, read)
    };
    // SAFETY: `child.0` is a valid handle owned above.
    if unsafe { SetHandleInformation(child.0, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
        // SAFETY: thread-local error read.
        return Err(format!("SetHandleInformation failed: {}", unsafe {
            GetLastError()
        }));
    }
    Ok((child, parent))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `CREATE_UNICODE_ENVIRONMENT` block, sorted case-insensitively as Windows
/// expects. An empty map yields an empty (double-NUL) block: nothing from the
/// parent environment leaks into the service.
fn environment_block(env: &HashMap<String, String>) -> Result<Vec<u16>, String> {
    let mut pairs: Vec<(&String, &String)> = env.iter().collect();
    pairs.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block = Vec::new();
    for (k, v) in pairs {
        if k.is_empty() || k.contains('=') || k.contains('\0') || v.contains('\0') {
            return Err(format!("invalid environment entry {k:?}"));
        }
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// Parent ends of the service's stdio pipes.
#[derive(Default)]
struct ParentPipes {
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

/// Real-OS [`ManagedLaunchApi`]. Owns the AppContainer profile (and through
/// it the workspace ACEs) and the confinement backend for the whole run.
pub(crate) struct WindowsLaunchApi<B: ConfinementBackend> {
    spec: ManagedLaunchSpec,
    backend: B,
    profile: Option<AppContainerProfile>,
    run_sid: Option<RunSid>,
    pipes: ParentPipes,
    confirm_timeout: Duration,
    /// Set when an admission abort could not confirm the job empty: the
    /// profile must then outlive this value (see [`launch`]).
    retained: bool,
}

impl<B: ConfinementBackend> WindowsLaunchApi<B> {
    pub(crate) fn new(spec: ManagedLaunchSpec, backend: B, confirm_timeout: Duration) -> Self {
        Self {
            spec,
            backend,
            profile: None,
            run_sid: None,
            pipes: ParentPipes::default(),
            confirm_timeout,
            retained: false,
        }
    }

    fn run_sid(&self) -> Result<&RunSid, String> {
        self.run_sid
            .as_ref()
            .ok_or_else(|| "no run SID: the service was not created".to_string())
    }

    fn spawn_suspended(&mut self) -> Result<SuspendedProcess, String> {
        if self.profile.is_some() {
            return Err("create_suspended called twice".into());
        }
        let mut profile =
            AppContainerProfile::create(&self.spec.app_container_capabilities, LOG_CONTEXT)?;
        if let Some(ws) = &self.spec.workspace {
            profile.grant_workspace(ws, &self.spec.deny_read_globs);
        }
        let run_sid = RunSid::parse(&profile.sid_string()?)?;

        let (child_in, parent_in) = pipe(true)?;
        let (child_out, parent_out) = pipe(false)?;
        let (child_err, parent_err) = pipe(false)?;

        // Only these three handles may be inherited by the service.
        let inherit: [HANDLE; 3] = [child_in.0, child_out.0, child_err.0];
        let sec_caps: SECURITY_CAPABILITIES = profile.security_capabilities();
        let mut attrs = ProcAttributeList::new(2)?;
        // SAFETY: `sec_caps` and `inherit` (and the SIDs owned by `profile`)
        // outlive the CreateProcessW call below.
        unsafe {
            attrs.update(
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &sec_caps as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
            )?;
            attrs.update(
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherit.as_ptr() as *const core::ffi::c_void,
                std::mem::size_of_val(&inherit),
            )?;
        }

        let mut cmd_line = wide(&build_command_line(&self.spec.program, &self.spec.args));
        let cwd = wide(&self.spec.cwd.to_string_lossy());
        let mut env = environment_block(&self.spec.env)?;

        // SAFETY: plain struct, zero-initialized then filled in.
        let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = child_in.0;
        si.StartupInfo.hStdOutput = child_out.0;
        si.StartupInfo.hStdError = child_err.0;
        si.lpAttributeList = attrs.as_raw();
        // SAFETY: plain struct.
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let flags = EXTENDED_STARTUPINFO_PRESENT
            | CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT
            | crate::utils::no_window::CREATE_NO_WINDOW;
        // SAFETY: every pointer is valid for the duration of the call;
        // `cmd_line` is mutable and NUL-terminated as CreateProcessW requires.
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmd_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                flags,
                env.as_mut_ptr() as *const core::ffi::c_void,
                cwd.as_ptr(),
                &si.StartupInfo,
                &mut pi,
            )
        };
        if ok == 0 {
            // SAFETY: thread-local error read.
            return Err(format!(
                "CreateProcessW(AppContainer, suspended) failed: {}",
                unsafe { GetLastError() }
            ));
        }
        // SAFETY: both handles come from the CREATE_SUSPENDED call above.
        let process = unsafe { SuspendedProcess::from_raw(pi.hProcess, pi.hThread) };
        drop(attrs);
        // The service owns its copies now; ours must close so EOF propagates.
        drop((child_in, child_out, child_err));
        self.pipes = ParentPipes {
            stdin: Some(parent_in.into_file()),
            stdout: Some(parent_out.into_file()),
            stderr: Some(parent_err.into_file()),
        };
        self.run_sid = Some(run_sid);
        self.profile = Some(profile);
        Ok(process)
    }
}

impl<B: ConfinementBackend> ManagedLaunchApi for WindowsLaunchApi<B> {
    type Proc = SuspendedProcess;
    type Job = SandboxJob;
    type Net = B::Net;

    fn create_job(&mut self) -> Result<SandboxJob, String> {
        SandboxJob::new_single_process(self.spec.max_memory_mb)
    }

    fn create_suspended(&mut self) -> Result<SuspendedProcess, String> {
        self.spawn_suspended()
    }

    fn read_identity(&mut self, p: &SuspendedProcess) -> Result<(u32, u64), String> {
        let id = p.read_identity()?;
        Ok((id.pid(), id.creation_filetime()))
    }

    fn assign_to_job(&mut self, job: &SandboxJob, p: &SuspendedProcess) -> Result<(), String> {
        // SAFETY: `p` owns a valid process handle for the duration of the call.
        unsafe { job.assign_process(p.process_handle()) }
    }

    fn verify_job_membership(
        &mut self,
        job: &SandboxJob,
        p: &SuspendedProcess,
    ) -> Result<bool, String> {
        job.contains_process(p.process_handle())
    }

    fn install_confinement(&mut self, plan: &ConfinementPlan) -> Result<B::Net, String> {
        let sid = self.run_sid()?.clone();
        self.backend.install(plan, &sid)
    }

    fn verify_confinement(&mut self, net: &B::Net, plan: &ConfinementPlan) -> Result<bool, String> {
        let sid = self.run_sid()?.clone();
        self.backend.verify(net, plan, &sid)
    }

    fn resume(&mut self, p: &SuspendedProcess) -> Result<(), String> {
        p.resume()
    }

    fn terminate_and_confirm_empty(
        &mut self,
        job: &SandboxJob,
        p: &SuspendedProcess,
    ) -> Result<bool, String> {
        native::terminate_and_confirm_empty(job, p, self.confirm_timeout)
    }

    fn remove_confinement(&mut self, net: B::Net) -> Result<(), String> {
        self.backend.remove(net)
    }

    fn retain_confinement(&mut self, net: Option<B::Net>) {
        // Leak on purpose: dropping a dynamic WFP session removes its filters.
        if let Some(net) = net {
            std::mem::forget(net);
        }
        self.retained = true;
    }
}

/// Blocking: run from `spawn_blocking`. Applies the strict ordering and
/// returns the admitted service as a [`ManagedProcess`].
pub(crate) fn launch<B: ConfinementBackend>(
    spec: ManagedLaunchSpec,
    backend: B,
    req: &NetworkRequest,
    caps: &ConfinementCapabilities,
    confirm_timeout: Duration,
) -> Result<WindowsManagedProcess<B>, SandboxError> {
    let mut api = WindowsLaunchApi::new(spec, backend, confirm_timeout);
    match launch_admitted(&mut api, req, caps) {
        Ok(svc) => Ok(WindowsManagedProcess::new(api, svc)),
        Err(e) => {
            if api.retained {
                // Job not confirmed empty: keep the AppContainer profile
                // (and workspace ACEs) alive with the filters - fail closed.
                std::mem::forget(api);
            }
            Err(e)
        }
    }
}

/// Post-exit teardown state, moved into a blocking task by `wait`.
struct Teardown<B: ConfinementBackend> {
    api: WindowsLaunchApi<B>,
    process: SuspendedProcess,
    job: Arc<SandboxJob>,
    net: Option<B::Net>,
}

impl<B: ConfinementBackend> Teardown<B> {
    /// Wait for the service, confirm the job is empty, then remove the
    /// confinement and drop the AppContainer profile. Blocking.
    fn run(mut self) -> io::Result<ManagedExit> {
        loop {
            match self.process.wait_exit(Duration::from_secs(3600)) {
                Ok(true) => break,
                Ok(false) => continue,
                Err(e) => return Err(self.fail_closed(format!("wait failed: {e}"))),
            }
        }
        let code = match self.process.exit_code() {
            Ok(c) => c,
            Err(e) => return Err(self.fail_closed(e)),
        };
        match self
            .api
            .terminate_and_confirm_empty(&self.job, &self.process)
        {
            Ok(true) => {}
            Ok(false) => return Err(self.fail_closed("job not confirmed empty".into())),
            Err(e) => return Err(self.fail_closed(format!("job termination failed: {e}"))),
        }
        if let Some(net) = self.net.take() {
            self.api.remove_confinement(net).map_err(|e| {
                io::Error::other(format!(
                    "service reaped but confinement removal failed: {e}"
                ))
            })?;
        }
        // Profile (workspace ACEs + AppContainer) goes only after the job is empty.
        drop(self.api);
        Ok(ManagedExit {
            code: Some(code as i32),
        })
    }

    /// Leak the confinement on purpose: an unconfirmed job must keep its
    /// network filters (and the profile) for as long as this process lives.
    fn fail_closed(mut self, why: String) -> io::Error {
        if let Some(net) = self.net.take() {
            std::mem::forget(net);
        }
        let _ = self.job.terminate(1);
        std::mem::forget(self.api);
        io::Error::other(format!("{why}; confinement left installed (fail closed)"))
    }
}

/// An admitted Windows service. `service_pid` is read from the owned
/// process handle at admission.
pub(crate) struct WindowsManagedProcess<B: ConfinementBackend> {
    identity: ServiceIdentity,
    job: Arc<SandboxJob>,
    stdio: ParentPipes,
    teardown: Option<Teardown<B>>,
    pending: Option<tokio::task::JoinHandle<io::Result<ManagedExit>>>,
    exit: Option<ManagedExit>,
}

impl<B: ConfinementBackend> WindowsManagedProcess<B> {
    fn new(mut api: WindowsLaunchApi<B>, svc: AdmittedService<WindowsLaunchApi<B>>) -> Self {
        let AdmittedService {
            identity,
            process,
            job,
            net,
        } = svc;
        let stdio = std::mem::take(&mut api.pipes);
        let job = Arc::new(job);
        Self {
            identity,
            job: Arc::clone(&job),
            stdio,
            teardown: Some(Teardown {
                api,
                process,
                job,
                net: Some(net),
            }),
            pending: None,
            exit: None,
        }
    }

    /// Handle-derived identity (PID + creation time) of the service.
    #[cfg(test)]
    pub(crate) fn identity(&self) -> ServiceIdentity {
        self.identity
    }
}

#[async_trait]
impl<B: ConfinementBackend> ManagedProcess for WindowsManagedProcess<B> {
    fn service_pid(&self) -> u32 {
        self.identity.pid()
    }

    fn take_stdio(&mut self) -> ManagedStdio {
        let wrap = |f: File| tokio::fs::File::from_std(f);
        ManagedStdio {
            stdin: self.stdio.stdin.take().map(|f| Box::new(wrap(f)) as _),
            stdout: self.stdio.stdout.take().map(|f| Box::new(wrap(f)) as _),
            stderr: self.stdio.stderr.take().map(|f| Box::new(wrap(f)) as _),
        }
    }

    fn start_kill(&mut self) -> io::Result<()> {
        if self.exit.is_some() {
            return Ok(());
        }
        self.job.terminate(1).map_err(io::Error::other)
    }

    /// Cancel-safe: the teardown runs in a blocking task whose handle is kept
    /// across dropped `wait` futures.
    async fn wait(&mut self) -> io::Result<ManagedExit> {
        if let Some(exit) = self.exit {
            return Ok(exit);
        }
        if self.pending.is_none() {
            let td = self
                .teardown
                .take()
                .ok_or_else(|| io::Error::other("managed teardown already failed"))?;
            self.pending = Some(tokio::task::spawn_blocking(move || td.run()));
        }
        let handle = self.pending.as_mut().expect("pending set above");
        let joined = handle.await;
        self.pending = None;
        let exit = joined.map_err(|e| io::Error::other(format!("teardown task failed: {e}")))??;
        self.exit = Some(exit);
        Ok(exit)
    }
}

impl<B: ConfinementBackend> Drop for WindowsManagedProcess<B> {
    fn drop(&mut self) {
        // Never waited: kill, then reap + confirm + remove off-thread with the
        // same ordering. A pending task already owns the teardown.
        if let Some(td) = self.teardown.take() {
            let _ = self.job.terminate(1);
            std::thread::spawn(move || {
                let _ = td.run();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Records every backend call in order; installs nothing.
    #[derive(Clone, Default)]
    struct RecordingBackend {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl ConfinementBackend for RecordingBackend {
        type Net = String;
        fn install(&mut self, plan: &ConfinementPlan, sid: &RunSid) -> Result<String, String> {
            self.events
                .lock()
                .unwrap()
                .push(format!("install:{:?}", plan.ports()));
            Ok(sid.as_str().to_string())
        }
        fn verify(
            &mut self,
            net: &String,
            _plan: &ConfinementPlan,
            sid: &RunSid,
        ) -> Result<bool, String> {
            self.events.lock().unwrap().push("verify".into());
            Ok(net == sid.as_str())
        }
        fn remove(&mut self, net: String) -> Result<(), String> {
            self.events
                .lock()
                .unwrap()
                .push(format!("remove:{}", net.starts_with("S-1-15-2-")));
            Ok(())
        }
    }

    /// Test-only: every confinement capability on, so the ordering runs with
    /// real OS objects and a recording backend. Production code never builds this.
    const TEST_CAPS: ConfinementCapabilities = ConfinementCapabilities {
        per_run_wfp_confinement: true,
        wfp_arbitration_verified: true,
        elevated: true,
    };

    fn system32(exe: &str) -> String {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{root}\System32\{exe}")
    }

    fn spec(program: String, args: &[&str], cwd: &std::path::Path) -> ManagedLaunchSpec {
        ManagedLaunchSpec {
            program,
            args: args.iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            cwd: cwd.to_path_buf(),
            app_container_capabilities: Vec::new(),
            // No workspace grant: tests must not touch ACLs outside a temp dir.
            workspace: None,
            deny_read_globs: Vec::new(),
            max_memory_mb: None,
        }
    }

    fn req() -> NetworkRequest {
        NetworkRequest {
            loopback_ports: vec![5173],
            allow_hosts: Vec::new(),
        }
    }

    fn image_name(pid: u32) -> String {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: query-only handle, closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        assert!(!h.is_null(), "OpenProcess({pid}) failed");
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: valid handle and buffer.
        let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) };
        // SAFETY: owned handle.
        unsafe { CloseHandle(h) };
        assert!(ok != 0, "QueryFullProcessImageNameW failed");
        String::from_utf16_lossy(&buf[..len as usize])
    }

    #[test]
    fn environment_block_is_sorted_and_never_inherits() {
        assert_eq!(environment_block(&HashMap::new()).unwrap(), vec![0, 0]);
        let env = HashMap::from([
            ("b".to_string(), "2".to_string()),
            ("A".to_string(), "1".to_string()),
        ]);
        let block = environment_block(&env).unwrap();
        assert_eq!(String::from_utf16_lossy(&block), "A=1\0b=2\0\0");
        let bad = HashMap::from([("A=B".to_string(), "1".to_string())]);
        assert!(environment_block(&bad).is_err());
    }

    #[ignore = "creates a real OS AppContainer profile (HKCU mapping + %LOCALAPPDATA%/Packages); run only with explicit authorization"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn service_identity_is_the_real_appcontainer_child_and_teardown_is_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RecordingBackend::default();
        let events = Arc::clone(&backend.events);
        let ping = system32("PING.EXE");
        let s = spec(ping, &["-n", "30", "127.0.0.1"], dir.path());
        let mut proc = tokio::task::spawn_blocking(move || {
            launch(s, backend, &req(), &TEST_CAPS, Duration::from_secs(10))
        })
        .await
        .unwrap()
        .expect("launch");

        let pid = proc.service_pid();
        assert_ne!(pid, std::process::id());
        assert_eq!(pid, proc.identity().pid());
        // Identity is the service image itself, not a wrapper.
        assert!(image_name(pid).to_ascii_uppercase().ends_with(r"\PING.EXE"));
        assert_eq!(
            *events.lock().unwrap(),
            vec!["install:[5173]".to_string(), "verify".to_string()]
        );

        let mut io = proc.take_stdio();
        assert!(io.stdin.is_some() && io.stdout.is_some() && io.stderr.is_some());
        assert!(
            proc.take_stdio().stdout.is_none(),
            "pipes are handed out once"
        );

        proc.start_kill().unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(30), proc.wait())
            .await
            .expect("wait bounded")
            .expect("wait ok");
        assert_eq!(exit.code, Some(1), "job termination exit code");
        assert_eq!(events.lock().unwrap().last().unwrap(), "remove:true");
        // After reap the job is empty and stdout reaches EOF.
        assert_eq!(proc.job.active_process_count().unwrap(), 0);
        let mut rest = Vec::new();
        io.stdout
            .as_mut()
            .unwrap()
            .read_to_end(&mut rest)
            .await
            .unwrap();
        // A second wait returns the cached exit.
        assert_eq!(proc.wait().await.unwrap(), exit);
    }

    #[ignore = "creates a real OS AppContainer profile (HKCU mapping + %LOCALAPPDATA%/Packages); run only with explicit authorization"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stdin_eof_and_stdout_flow_through_the_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(system32("sort.exe"), &[], dir.path());
        let mut proc = tokio::task::spawn_blocking(move || {
            launch(
                s,
                RecordingBackend::default(),
                &req(),
                &TEST_CAPS,
                Duration::from_secs(10),
            )
        })
        .await
        .unwrap()
        .expect("launch");
        let ManagedStdio { stdin, stdout, .. } = proc.take_stdio();
        let mut stdin = stdin.unwrap();
        stdin.write_all(b"b\r\na\r\n").await.unwrap();
        stdin.shutdown().await.unwrap();
        drop(stdin);
        let mut out = String::new();
        let mut stdout = stdout.unwrap();
        tokio::time::timeout(Duration::from_secs(30), stdout.read_to_string(&mut out))
            .await
            .expect("stdout bounded")
            .unwrap();
        assert_eq!(out.split_whitespace().collect::<Vec<_>>(), vec!["a", "b"]);
        let exit = tokio::time::timeout(Duration::from_secs(30), proc.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(exit.success());
    }

    #[ignore = "creates a real OS AppContainer profile (HKCU mapping + %LOCALAPPDATA%/Packages); run only with explicit authorization"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_wait_keeps_the_teardown_and_a_retry_completes_it() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RecordingBackend::default();
        let events = Arc::clone(&backend.events);
        let s = spec(system32("PING.EXE"), &["-n", "30", "127.0.0.1"], dir.path());
        let mut proc = tokio::task::spawn_blocking(move || {
            launch(s, backend, &req(), &TEST_CAPS, Duration::from_secs(10))
        })
        .await
        .unwrap()
        .unwrap();
        let _io = proc.take_stdio();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), proc.wait())
                .await
                .is_err()
        );
        proc.start_kill().unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(30), proc.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(exit.code, Some(1));
        assert_eq!(events.lock().unwrap().last().unwrap(), "remove:true");
    }

    #[ignore = "creates a real OS AppContainer profile (HKCU mapping + %LOCALAPPDATA%/Packages); run only with explicit authorization"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_readback_never_resumes_and_removes_nothing_unconfirmed() {
        struct RejectingBackend;
        impl ConfinementBackend for RejectingBackend {
            type Net = ();
            fn install(&mut self, _: &ConfinementPlan, _: &RunSid) -> Result<(), String> {
                Ok(())
            }
            fn verify(&mut self, _: &(), _: &ConfinementPlan, _: &RunSid) -> Result<bool, String> {
                Ok(false)
            }
            fn remove(&mut self, _: ()) -> Result<(), String> {
                Ok(())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let s = spec(system32("PING.EXE"), &["-n", "30", "127.0.0.1"], dir.path());
        let err = tokio::task::spawn_blocking(move || {
            launch(
                s,
                RejectingBackend,
                &req(),
                &TEST_CAPS,
                Duration::from_secs(10),
            )
            .map(|_| ())
        })
        .await
        .unwrap()
        .expect_err("readback mismatch must refuse");
        assert!(err.to_string().contains("verify_confinement"), "{err}");
    }

    #[test]
    fn current_capabilities_refuse_before_any_os_object() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(system32("PING.EXE"), &["-n", "1", "127.0.0.1"], dir.path());
        let err = launch(
            s,
            RecordingBackend::default(),
            &req(),
            &super::super::managed_launch::CURRENT_CAPABILITIES,
            Duration::from_secs(10),
        )
        .map(|_| ())
        .expect_err("current build must refuse");
        assert!(
            matches!(err, SandboxError::UnsupportedPolicy { .. }),
            "{err}"
        );
    }
}
