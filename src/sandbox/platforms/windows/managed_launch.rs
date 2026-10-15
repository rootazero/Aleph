//! Strict managed-service launch contract for Windows (#53 / #48).
//!
//! STATUS: contract + ordering state machine + typed blocker report + fake-API tests, plus
//! OS primitives ([`native`]: suspended-process handle identity/resume, job
//! kill-and-confirm-empty) that were compiled and exercised by host-token tests against a
//! system `ping.exe` (see `.analysis/browser-live-runtime/windows-native-suspended-test.log`),
//! and a pure per-run WFP filter spec builder/readback verifier ([`build_filter_specs`]).
//! There is still NO production [`ManagedLaunchApi`] implementation: suspended creation needs a
//! parent-side token/AppContainer + stdio pipes, `ManagedChild` wraps a `tokio::process::Child`
//! that such a process cannot be, and no WFP FFI / admin proof exists. The Windows driver still
//! refuses every managed spawn (`WindowsSandboxDriver::managed_spawn_preflight`), and
//! `CURRENT_CAPABILITIES` is all false. Exact blockers:
//! `docs/reports/verify-20261007/windows-managed-seam-decision.md`.
//!
//! What this module pins down, so a later real implementation cannot silently
//! weaken it:
//!
//! 1. **Admission before execution.** The service is created suspended, its
//!    identity (real process handle + PID + creation FILETIME) is read from
//!    the handle we own, it is placed in the Job Object and membership is
//!    verified, and the network confinement is installed and read back -
//!    all BEFORE `resume`. A failure at any step terminates the suspended
//!    process; it is never resumed.
//! 2. **Identity.** [`ServiceIdentity`] is only constructible from a non-zero
//!    PID and non-zero creation time; a wrapper PID, or a PID-only identity,
//!    cannot be expressed.
//! 3. **Network.** [`plan_network_confinement`] never degrades. Without a
//!    verified per-run confinement capability the answer is a typed
//!    `UnsupportedPolicy`; `AllowHosts` is always unsupported; there is no
//!    "permit all loopback" outcome.
//! 4. **Teardown order.** Network confinement is removed only after the job
//!    is confirmed empty. If termination cannot be confirmed, the filters are
//!    deliberately left in place (fail closed) and the error says so.

#![allow(dead_code)] // contract only; the production API impl is intentionally absent

use crate::sandbox::command::SandboxError;

pub(crate) const PLATFORM: &str = "windows/token";

/// Process identity: PID alone is reusable, so the creation FILETIME (100ns
/// ticks since 1601, as returned by `GetProcessTimes`) is part of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ServiceIdentity {
    pid: u32,
    creation_filetime: u64,
}

impl ServiceIdentity {
    pub(crate) fn new(pid: u32, creation_filetime: u64) -> Result<Self, String> {
        if pid == 0 {
            return Err("service PID is 0".into());
        }
        if creation_filetime == 0 {
            return Err(
                "service creation time is 0 (identity not read from a process handle)".into(),
            );
        }
        Ok(Self {
            pid,
            creation_filetime,
        })
    }
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }
    pub(crate) fn creation_filetime(&self) -> u64 {
        self.creation_filetime
    }
}

/// Exact network the managed service may use. Loopback only; never a host list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkRequest {
    /// Exact TCP ports on 127.0.0.1 / ::1 the service may CONNECT to or
    /// LISTEN/ACCEPT on (companion control port, gateway port).
    pub(crate) loopback_ports: Vec<u16>,
    /// Non-empty means the caller asked for `NetworkPolicy::AllowHosts`.
    pub(crate) allow_hosts: Vec<String>,
}

/// What the host can PROVE it can enforce. Every field defaults to `false`
/// and is flipped only by a verified implementation + admin experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ConfinementCapabilities {
    /// Persistent per-run WFP provider/sublayer/filters keyed by a random
    /// per-run SID, installed before resume, verified by readback.
    pub(crate) per_run_wfp_confinement: bool,
    /// WFP permit for loopback is proven to win against the AppContainer
    /// loopback block (cross-sublayer arbitration) on this host.
    pub(crate) wfp_arbitration_verified: bool,
    /// The process running this code is elevated (WFP writes need admin).
    pub(crate) elevated: bool,
}

/// Capabilities of THIS build: nothing is verified.
pub(crate) const CURRENT_CAPABILITIES: ConfinementCapabilities = ConfinementCapabilities {
    per_run_wfp_confinement: false,
    wfp_arbitration_verified: false,
    elevated: false,
};

/// Validated, exact confinement. Only [`plan_network_confinement`] makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfinementPlan {
    ports: Vec<u16>,
}

impl ConfinementPlan {
    pub(crate) fn ports(&self) -> &[u16] {
        &self.ports
    }
}

fn unsupported(feature: &str, reason: impl Into<String>) -> SandboxError {
    SandboxError::UnsupportedPolicy {
        platform: PLATFORM,
        feature: feature.into(),
        reason: reason.into(),
    }
}

/// Decide whether the requested network can be enforced EXACTLY. Pure.
pub(crate) fn plan_network_confinement(
    req: &NetworkRequest,
    caps: &ConfinementCapabilities,
) -> Result<ConfinementPlan, SandboxError> {
    if !req.allow_hosts.is_empty() {
        return Err(unsupported(
            "managed_network_allow_hosts",
            "NetworkPolicy::AllowHosts is not enforceable for a managed Windows service: \
             hosts are not mapped to WFP filters and the managed proxy is blocked by AppContainer loopback isolation",
        ));
    }
    if req.loopback_ports.is_empty() || req.loopback_ports.contains(&0) {
        return Err(unsupported(
            "managed_network_ports",
            "managed confinement needs one or more exact non-zero loopback ports",
        ));
    }
    if !caps.per_run_wfp_confinement {
        return Err(unsupported(
            "managed_network_confinement",
            "no per-run (random SID + provider GUID) WFP confinement is implemented in this build; \
             loopback is neither blanket-exempted nor silently ignored",
        ));
    }
    if !caps.wfp_arbitration_verified {
        return Err(unsupported(
            "managed_network_arbitration",
            "WFP permit vs AppContainer loopback block arbitration has not been verified on this host \
             (admin experiment pending); not assuming permit wins",
        ));
    }
    if !caps.elevated {
        return Err(unsupported(
            "managed_network_elevation",
            "installing persistent WFP filters needs an elevated process; this process is not elevated and will not auto-elevate",
        ));
    }
    let mut ports = req.loopback_ports.clone();
    ports.sort_unstable();
    ports.dedup();
    Ok(ConfinementPlan { ports })
}

/// Facts about this build/host that decide whether a strict managed launch is
/// possible. `observe` fills the host-dependent part from read-only probes;
/// the build-dependent part is `const` and flips only with a verified impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostFacts {
    /// `Some(bool)` from a read-only token probe; `None` = probe failed.
    pub(crate) elevated: Option<bool>,
    /// The launch ordering is backed by a real `ManagedLaunchApi` impl
    /// (`launcher::WindowsLaunchApi`, wired into `driver::spawn_managed`).
    pub(crate) production_launcher: bool,
    /// The parent holds the service's own process handle and reads its
    /// PID + creation time from it (managed launch creates the service
    /// parent-side, so no `sandbox-init-windows` wrapper PID is involved).
    pub(crate) wrapper_reports_service_identity: bool,
    pub(crate) caps: ConfinementCapabilities,
}

impl HostFacts {
    /// This build, with elevation answered by `elevated`. The AppContainer
    /// launcher and its handle-derived identity are implemented and wired,
    /// but stay `false` until the real-AppContainer `launcher::tests` (which
    /// create an OS AppContainer profile under the user's profile and are
    /// therefore `#[ignore]`d) have been executed and recorded as evidence.
    /// The confinement capabilities are not verified and stay false.
    pub(crate) const fn current_build(elevated: Option<bool>) -> Self {
        Self {
            elevated,
            production_launcher: false,
            wrapper_reports_service_identity: false,
            caps: CURRENT_CAPABILITIES,
        }
    }

    /// Read-only: queries the current process token, nothing else.
    pub(crate) fn observe() -> Self {
        Self::current_build(super::elevation::process_is_elevated().ok())
    }
}

/// One reason a strict managed launch cannot be offered. Typed so callers and
/// reports can assert on it instead of matching message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedLaunchBlocker {
    WrapperPidIsNotServicePid,
    NoProductionLauncher,
    NoPerRunWfpConfinement,
    WfpArbitrationUnverified,
    NotElevated,
    ElevationUnknown,
}

impl ManagedLaunchBlocker {
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::WrapperPidIsNotServicePid => {
                "the parent-side service identity (PID + creation time read from the AppContainer child's own handle) is implemented but has not been proven by an executed real-AppContainer launch on this host"
            }
            Self::NoProductionLauncher => {
                "the AppContainer suspended-create + job-admission launcher (launcher::WindowsLaunchApi) is implemented and wired but has not been proven by an executed real-process test on this host"
            }
            Self::NoPerRunWfpConfinement => {
                "per-run (random SID + provider GUID) WFP confinement is implemented but not enabled: install/readback/removal have not been proven on this host by the admin-only probe"
            }
            Self::WfpArbitrationUnverified => {
                "WFP permit vs AppContainer loopback block arbitration is unverified on this host (no admin evidence)"
            }
            Self::NotElevated => {
                "this process token is not elevated; installing WFP filters needs Administrator and the process never auto-elevates"
            }
            Self::ElevationUnknown => "the process-token elevation probe failed, so elevation is treated as absent",
        }
    }
}

/// Every blocker that currently applies, in stable order. Empty means a
/// strict managed launch is possible. Pure: no OS access.
pub(crate) fn assess_managed_launch(facts: &HostFacts) -> Vec<ManagedLaunchBlocker> {
    use ManagedLaunchBlocker as B;
    let mut out = Vec::new();
    if !facts.wrapper_reports_service_identity {
        out.push(B::WrapperPidIsNotServicePid);
    }
    if !facts.production_launcher {
        out.push(B::NoProductionLauncher);
    }
    if !facts.caps.per_run_wfp_confinement {
        out.push(B::NoPerRunWfpConfinement);
    }
    if !facts.caps.wfp_arbitration_verified {
        out.push(B::WfpArbitrationUnverified);
    }
    match facts.elevated {
        Some(true) => {}
        Some(false) => out.push(B::NotElevated),
        None => out.push(B::ElevationUnknown),
    }
    out
}

/// Typed refusal for `WindowsSandboxDriver::managed_spawn_preflight`.
/// `Ok(())` only when [`assess_managed_launch`] finds no blocker.
pub(crate) fn managed_spawn_preflight_with(facts: &HostFacts) -> Result<(), SandboxError> {
    let blockers = assess_managed_launch(facts);
    if blockers.is_empty() {
        return Ok(());
    }
    let list = blockers
        .iter()
        .map(|b| b.describe())
        .collect::<Vec<_>>()
        .join("; ");
    Err(unsupported(
        "managed_spawn",
        format!(
            "{} blocker(s): {list}. A restricted token is not disabled and no global loopback exemption is added to work around this; AllowHosts is unsupported",
            blockers.len()
        ),
    ))
}

/// Everything the launch needs from the OS, as an ordered, owned-handle API.
/// A production impl would wrap `CreateProcessAsUserW(CREATE_SUSPENDED)`,
/// `GetProcessTimes`, `AssignProcessToJobObject`/`IsProcessInJob`, WFP
/// `Fwpm*` calls and `ResumeThread`. Each handle type owns and closes itself.
pub(crate) trait ManagedLaunchApi {
    type Proc;
    type Job;
    type Net;

    fn create_job(&mut self) -> Result<Self::Job, String>;
    /// Create the service SUSPENDED under the sandbox token. Returns the real
    /// process handle (not a wrapper).
    fn create_suspended(&mut self) -> Result<Self::Proc, String>;
    /// Read PID + creation FILETIME from the process handle itself.
    fn read_identity(&mut self, p: &Self::Proc) -> Result<(u32, u64), String>;
    fn assign_to_job(&mut self, job: &Self::Job, p: &Self::Proc) -> Result<(), String>;
    fn verify_job_membership(&mut self, job: &Self::Job, p: &Self::Proc) -> Result<bool, String>;
    fn install_confinement(&mut self, plan: &ConfinementPlan) -> Result<Self::Net, String>;
    /// Read the installed filters back and compare with `plan` exactly.
    fn verify_confinement(
        &mut self,
        net: &Self::Net,
        plan: &ConfinementPlan,
    ) -> Result<bool, String>;
    fn resume(&mut self, p: &Self::Proc) -> Result<(), String>;
    /// Bounded: terminate the job, wait for the process; `Ok(true)` only when
    /// the job is confirmed to have zero active processes.
    fn terminate_and_confirm_empty(
        &mut self,
        job: &Self::Job,
        p: &Self::Proc,
    ) -> Result<bool, String>;
    fn remove_confinement(&mut self, net: Self::Net) -> Result<(), String>;
    /// The job was NOT confirmed empty: keep the confinement (and whatever
    /// else the impl owns, e.g. the AppContainer profile) installed for the
    /// rest of this process. Dropping `net` is NOT an option - for a dynamic
    /// WFP session that would remove the filters (fail open).
    fn retain_confinement(&mut self, net: Option<Self::Net>);
}

/// A launched, admitted service. Owns every handle; teardown goes through
/// [`ManagedLaunchApi::terminate_and_confirm_empty`] then
/// [`ManagedLaunchApi::remove_confinement`].
pub(crate) struct AdmittedService<A: ManagedLaunchApi> {
    pub(crate) identity: ServiceIdentity,
    pub(crate) process: A::Proc,
    pub(crate) job: A::Job,
    pub(crate) net: A::Net,
}

fn launch_error(step: &str, detail: &str) -> SandboxError {
    SandboxError::Other(format!("managed launch refused at {step}: {detail}"))
}

/// Run the strict ordering. On error after the process exists, it is
/// terminated (never resumed) and confinement is removed only if the job was
/// confirmed empty.
pub(crate) fn launch_admitted<A: ManagedLaunchApi>(
    api: &mut A,
    req: &NetworkRequest,
    caps: &ConfinementCapabilities,
) -> Result<AdmittedService<A>, SandboxError> {
    // 1. Network decision BEFORE any OS object exists.
    let plan = plan_network_confinement(req, caps)?;

    let job = api
        .create_job()
        .map_err(|e| launch_error("create_job", &e))?;
    let process = api
        .create_suspended()
        .map_err(|e| launch_error("create_suspended", &e))?;

    // From here the suspended process exists: any failure must tear down.
    macro_rules! bail {
        ($net:expr, $step:expr, $detail:expr) => {{
            let detail: String = $detail;
            return Err(abort(api, &job, &process, $net, $step, &detail));
        }};
    }

    let (pid, created) = match api.read_identity(&process) {
        Ok(v) => v,
        Err(e) => bail!(None, "read_identity", e),
    };
    let identity = match ServiceIdentity::new(pid, created) {
        Ok(i) => i,
        Err(e) => bail!(None, "identity", e),
    };
    if let Err(e) = api.assign_to_job(&job, &process) {
        bail!(None, "assign_to_job", e);
    }
    match api.verify_job_membership(&job, &process) {
        Ok(true) => {}
        Ok(false) => bail!(
            None,
            "verify_job_membership",
            "process is not in the job".into()
        ),
        Err(e) => bail!(None, "verify_job_membership", e),
    }
    let net = match api.install_confinement(&plan) {
        Ok(n) => n,
        Err(e) => bail!(None, "install_confinement", e),
    };
    match api.verify_confinement(&net, &plan) {
        Ok(true) => {}
        Ok(false) => bail!(
            Some(net),
            "verify_confinement",
            "readback differs from plan".into()
        ),
        Err(e) => bail!(Some(net), "verify_confinement", e),
    }
    if let Err(e) = api.resume(&process) {
        bail!(Some(net), "resume", e);
    }
    Ok(AdmittedService {
        identity,
        process,
        job,
        net,
    })
}

fn abort<A: ManagedLaunchApi>(
    api: &mut A,
    job: &A::Job,
    process: &A::Proc,
    net: Option<A::Net>,
    step: &str,
    detail: &str,
) -> SandboxError {
    match api.terminate_and_confirm_empty(job, process) {
        Ok(true) => {
            if let Some(net) = net {
                if let Err(e) = api.remove_confinement(net) {
                    return launch_error(
                        step,
                        &format!("{detail}; process reaped but confinement removal failed: {e}"),
                    );
                }
            }
            launch_error(step, detail)
        }
        // Unconfirmed: leave the filters (and profile) in place - fail closed.
        Ok(false) => {
            api.retain_confinement(net);
            launch_error(
                step,
                &format!("{detail}; job NOT confirmed empty, confinement left installed"),
            )
        }
        Err(e) => {
            api.retain_confinement(net);
            launch_error(
                step,
                &format!("{detail}; termination failed ({e}), confinement left installed"),
            )
        }
    }
}

/// Normal teardown with the same ordering rule.
pub(crate) fn teardown<A: ManagedLaunchApi>(
    api: &mut A,
    svc: AdmittedService<A>,
) -> Result<(), SandboxError> {
    let AdmittedService {
        process, job, net, ..
    } = svc;
    match api.terminate_and_confirm_empty(&job, &process) {
        Ok(true) => api
            .remove_confinement(net)
            .map_err(|e| launch_error("teardown", &format!("confinement removal failed: {e}"))),
        Ok(false) => {
            api.retain_confinement(Some(net));
            Err(launch_error(
                "teardown",
                "job not confirmed empty; confinement left installed",
            ))
        }
        Err(e) => {
            api.retain_confinement(Some(net));
            Err(launch_error(
                "teardown",
                &format!("{e}; confinement left installed"),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Native building blocks (compile-unverified on the authoring host; see STATUS).
// These are the OS primitives a production `ManagedLaunchApi` will compose.
// They do NOT make a launcher: nothing creates the suspended process yet.
// ---------------------------------------------------------------------------

/// Parent-owned handles of a process created with `CREATE_SUSPENDED`.
/// Identity is read from these handles, never from a wrapper.
pub(crate) mod native {
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, FILETIME, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessId, GetProcessTimes, ResumeThread, WaitForSingleObject,
    };

    use super::ServiceIdentity;
    use crate::sandbox::platforms::windows::job::SandboxJob;

    pub(crate) struct SuspendedProcess {
        process: HANDLE,
        thread: HANDLE,
    }

    // SAFETY: the handles are plain kernel object handles owned exclusively by this value.
    unsafe impl Send for SuspendedProcess {}

    impl SuspendedProcess {
        /// # Safety
        /// `process` and `thread` must be valid handles from a `CREATE_SUSPENDED`
        /// `CreateProcess*` call; ownership transfers to the returned value.
        pub(crate) unsafe fn from_raw(process: HANDLE, thread: HANDLE) -> Self {
            Self { process, thread }
        }

        pub(crate) fn process_handle(&self) -> HANDLE {
            self.process
        }

        /// Real service PID + creation FILETIME, read from the owned handle.
        pub(crate) fn read_identity(&self) -> Result<ServiceIdentity, String> {
            // SAFETY: `self.process` is a valid handle for the lifetime of `self`.
            let pid = unsafe { GetProcessId(self.process) };
            let zero = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
            // SAFETY: all four out-pointers are valid FILETIMEs.
            let ok = unsafe {
                GetProcessTimes(
                    self.process,
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            };
            if ok == 0 {
                // SAFETY: thread-local error read.
                return Err(format!("GetProcessTimes failed: {}", unsafe {
                    GetLastError()
                }));
            }
            let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
            ServiceIdentity::new(pid, ticks)
        }

        /// Resume the primary thread. Requires the previous suspend count to be
        /// exactly 1 (created suspended, never resumed before); anything else
        /// is an error because the admission ordering was violated.
        pub(crate) fn resume(&self) -> Result<(), String> {
            // SAFETY: `self.thread` is a valid thread handle.
            let prev = unsafe { ResumeThread(self.thread) };
            if prev == u32::MAX {
                // SAFETY: thread-local error read.
                return Err(format!("ResumeThread failed: {}", unsafe {
                    GetLastError()
                }));
            }
            if prev != 1 {
                return Err(format!(
                    "ResumeThread previous suspend count was {prev}, expected 1"
                ));
            }
            Ok(())
        }

        /// Exit code read from the owned handle. `STILL_ACTIVE` (259) is an
        /// error: call only after [`Self::wait_exit`] returned `Ok(true)`.
        pub(crate) fn exit_code(&self) -> Result<u32, String> {
            let mut code: u32 = 0;
            // SAFETY: valid process handle and out-pointer.
            if unsafe {
                windows_sys::Win32::System::Threading::GetExitCodeProcess(self.process, &mut code)
            } == 0
            {
                // SAFETY: thread-local error read.
                return Err(format!("GetExitCodeProcess failed: {}", unsafe {
                    GetLastError()
                }));
            }
            if code == windows_sys::Win32::Foundation::STILL_ACTIVE as u32 {
                return Err("process has not exited (STILL_ACTIVE)".into());
            }
            Ok(code)
        }

        /// `Ok(true)` when the process has exited within `timeout`.
        pub(crate) fn wait_exit(&self, timeout: Duration) -> Result<bool, String> {
            let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
            // SAFETY: valid process handle.
            let r = unsafe { WaitForSingleObject(self.process, ms) };
            if r == WAIT_OBJECT_0 {
                Ok(true)
            } else if r == windows_sys::Win32::Foundation::WAIT_TIMEOUT {
                Ok(false)
            } else {
                // SAFETY: thread-local error read.
                Err(format!("WaitForSingleObject returned {r}: {}", unsafe {
                    GetLastError()
                }))
            }
        }
    }

    impl Drop for SuspendedProcess {
        fn drop(&mut self) {
            // SAFETY: handles are owned; closing twice is prevented by Drop running once.
            unsafe {
                if !self.thread.is_null() {
                    CloseHandle(self.thread);
                }
                if !self.process.is_null() {
                    CloseHandle(self.process);
                }
            }
        }
    }

    /// Kill the whole job, wait for the service process to exit, then poll the
    /// kernel `ActiveProcesses` counter to zero. `Ok(true)` ONLY when both the
    /// process exited and the job is empty within `timeout`; a timeout is
    /// `Ok(false)` so callers keep the network confinement installed.
    /// Blocking: call from `spawn_blocking`.
    pub(crate) fn terminate_and_confirm_empty(
        job: &SandboxJob,
        process: &SuspendedProcess,
        timeout: Duration,
    ) -> Result<bool, String> {
        let deadline = Instant::now() + timeout;
        job.terminate(1)?;
        if !process.wait_exit(timeout)? {
            return Ok(false);
        }
        loop {
            if job.active_process_count()? == 0 {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

// ---------------------------------------------------------------------------
// Pure per-run WFP filter specification (no OS access, no FFI).
// ---------------------------------------------------------------------------

/// Per-run AppContainer / restricted-token package SID (`S-1-...`). Every
/// filter is conditioned on it, so nothing outside this run is affected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunSid(String);

impl RunSid {
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        let ok = s.len() >= 8
            && s.starts_with("S-1-")
            && s.bytes()
                .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
            && s[1..]
                .split('-')
                .skip(1)
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            && s.matches('S').count() == 1;
        if ok {
            Ok(Self(s.to_string()))
        } else {
            Err(format!("invalid run SID {s:?}"))
        }
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum WfpLayer {
    ConnectV4,
    ConnectV6,
    ListenV4,
    ListenV6,
    RecvAcceptV4,
    RecvAcceptV6,
}

impl WfpLayer {
    const ALL: [WfpLayer; 6] = [
        Self::ConnectV4,
        Self::ConnectV6,
        Self::ListenV4,
        Self::ListenV6,
        Self::RecvAcceptV4,
        Self::RecvAcceptV6,
    ];
    /// Outbound connect filters match the REMOTE port; listen/accept match the LOCAL port.
    fn port_is_remote(self) -> bool {
        matches!(self, Self::ConnectV4 | Self::ConnectV6)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum WfpAction {
    Permit,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum WfpMatch {
    /// Loopback address + TCP + exactly this port.
    LoopbackTcpPort { port: u16, remote: bool },
    /// Catch-all for the run SID (used only with `Block`).
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct FilterSpec {
    pub(crate) layer: WfpLayer,
    pub(crate) action: WfpAction,
    pub(crate) weight: u8,
    pub(crate) run_sid: String,
    pub(crate) matcher: WfpMatch,
}

pub(crate) const PERMIT_WEIGHT: u8 = 15;
pub(crate) const BLOCK_WEIGHT: u8 = 1;

/// Exact default-deny spec: per layer one `Block` catch-all plus one `Permit`
/// per planned loopback port (loopback + TCP + that port only), all scoped to
/// `sid`. There is no loopback-wide permit and no host permit. Pure.
pub(crate) fn build_filter_specs(plan: &ConfinementPlan, sid: &RunSid) -> Vec<FilterSpec> {
    let mut out = Vec::new();
    for layer in WfpLayer::ALL {
        out.push(FilterSpec {
            layer,
            action: WfpAction::Block,
            weight: BLOCK_WEIGHT,
            run_sid: sid.as_str().to_string(),
            matcher: WfpMatch::Any,
        });
        for &port in plan.ports() {
            out.push(FilterSpec {
                layer,
                action: WfpAction::Permit,
                weight: PERMIT_WEIGHT,
                run_sid: sid.as_str().to_string(),
                matcher: WfpMatch::LoopbackTcpPort {
                    port,
                    remote: layer.port_is_remote(),
                },
            });
        }
    }
    out
}

/// Order-independent exact multiset comparison of expected vs. read-back
/// filters. Any missing, extra or altered filter is an error.
pub(crate) fn verify_filter_readback(
    expected: &[FilterSpec],
    actual: &[FilterSpec],
) -> Result<(), String> {
    let mut e = expected.to_vec();
    let mut a = actual.to_vec();
    e.sort();
    a.sort();
    if e == a {
        return Ok(());
    }
    let missing = e.iter().filter(|x| !a.contains(x)).count();
    let extra = a.iter().filter(|x| !e.contains(x)).count();
    Err(format!(
        "filter readback mismatch: expected {} got {} ({missing} missing, {extra} unexpected)",
        e.len(),
        a.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_CAPS: ConfinementCapabilities = ConfinementCapabilities {
        per_run_wfp_confinement: true,
        wfp_arbitration_verified: true,
        elevated: true,
    };

    fn req() -> NetworkRequest {
        NetworkRequest {
            loopback_ports: vec![9223, 18790, 9223],
            allow_hosts: vec![],
        }
    }

    #[derive(Default)]
    struct Fake {
        calls: Vec<&'static str>,
        fail_at: Option<&'static str>,
        identity: (u32, u64),
        member: bool,
        readback_ok: bool,
        empty_confirmed: bool,
    }

    impl Fake {
        fn ok() -> Self {
            Self {
                identity: (4242, 1337),
                member: true,
                readback_ok: true,
                empty_confirmed: true,
                ..Default::default()
            }
        }
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.calls.push(name);
            if self.fail_at == Some(name) {
                Err(format!("{name} failed"))
            } else {
                Ok(())
            }
        }
    }

    impl ManagedLaunchApi for Fake {
        type Proc = ();
        type Job = ();
        type Net = ();
        fn create_job(&mut self) -> Result<(), String> {
            self.step("create_job")
        }
        fn create_suspended(&mut self) -> Result<(), String> {
            self.step("create_suspended")
        }
        fn read_identity(&mut self, _: &()) -> Result<(u32, u64), String> {
            self.step("read_identity")?;
            Ok(self.identity)
        }
        fn assign_to_job(&mut self, _: &(), _: &()) -> Result<(), String> {
            self.step("assign_to_job")
        }
        fn verify_job_membership(&mut self, _: &(), _: &()) -> Result<bool, String> {
            self.step("verify_job_membership")?;
            Ok(self.member)
        }
        fn install_confinement(&mut self, plan: &ConfinementPlan) -> Result<(), String> {
            assert_eq!(plan.ports(), &[9223, 18790], "ports sorted + deduped");
            self.step("install_confinement")
        }
        fn verify_confinement(&mut self, _: &(), _: &ConfinementPlan) -> Result<bool, String> {
            self.step("verify_confinement")?;
            Ok(self.readback_ok)
        }
        fn resume(&mut self, _: &()) -> Result<(), String> {
            self.step("resume")
        }
        fn terminate_and_confirm_empty(&mut self, _: &(), _: &()) -> Result<bool, String> {
            self.step("terminate")?;
            Ok(self.empty_confirmed)
        }
        fn remove_confinement(&mut self, _: ()) -> Result<(), String> {
            self.step("remove_confinement")
        }
        fn retain_confinement(&mut self, net: Option<()>) {
            self.calls.push(if net.is_some() {
                "retain_confinement"
            } else {
                "retain_nothing"
            });
        }
    }

    #[test]
    fn happy_path_resumes_last_after_admission_and_readback() {
        let mut f = Fake::ok();
        let svc = launch_admitted(&mut f, &req(), &GOOD_CAPS).expect("launch");
        assert_eq!(svc.identity.pid(), 4242);
        assert_eq!(
            f.calls,
            [
                "create_job",
                "create_suspended",
                "read_identity",
                "assign_to_job",
                "verify_job_membership",
                "install_confinement",
                "verify_confinement",
                "resume"
            ]
        );
    }

    #[test]
    fn current_build_refuses_before_any_os_object_is_created() {
        let mut f = Fake::ok();
        let err = launch_admitted(&mut f, &req(), &CURRENT_CAPABILITIES)
            .err()
            .expect("refused");
        assert!(
            matches!(err, SandboxError::UnsupportedPolicy { .. }),
            "{err:?}"
        );
        assert!(
            f.calls.is_empty(),
            "no spawn/job/filter call may happen: {:?}",
            f.calls
        );
    }

    #[test]
    fn allow_hosts_is_always_unsupported_even_with_full_capabilities() {
        let r = NetworkRequest {
            loopback_ports: vec![9223],
            allow_hosts: vec!["example.com".into()],
        };
        assert!(matches!(
            plan_network_confinement(&r, &GOOD_CAPS),
            Err(SandboxError::UnsupportedPolicy { .. })
        ));
    }

    #[test]
    fn each_missing_capability_and_bad_ports_are_refused() {
        for caps in [
            ConfinementCapabilities {
                per_run_wfp_confinement: false,
                ..GOOD_CAPS
            },
            ConfinementCapabilities {
                wfp_arbitration_verified: false,
                ..GOOD_CAPS
            },
            ConfinementCapabilities {
                elevated: false,
                ..GOOD_CAPS
            },
        ] {
            assert!(plan_network_confinement(&req(), &caps).is_err());
        }
        for ports in [vec![], vec![0], vec![80, 0]] {
            let r = NetworkRequest {
                loopback_ports: ports,
                allow_hosts: vec![],
            };
            assert!(plan_network_confinement(&r, &GOOD_CAPS).is_err());
        }
    }

    #[test]
    fn identity_rejects_pid_only_and_zero() {
        assert!(ServiceIdentity::new(0, 5).is_err());
        assert!(ServiceIdentity::new(5, 0).is_err());
        assert!(ServiceIdentity::new(5, 6).is_ok());
    }

    #[test]
    fn failure_before_resume_never_resumes_and_terminates() {
        for at in [
            "read_identity",
            "assign_to_job",
            "verify_job_membership",
            "install_confinement",
            "verify_confinement",
        ] {
            let mut f = Fake::ok();
            f.fail_at = Some(at);
            assert!(launch_admitted(&mut f, &req(), &GOOD_CAPS).is_err(), "{at}");
            assert!(!f.calls.contains(&"resume"), "{at}: resumed after failure");
            assert!(f.calls.contains(&"terminate"), "{at}: not terminated");
        }
    }

    #[test]
    fn not_a_member_or_bad_readback_or_zero_identity_never_resumes() {
        let mut f = Fake::ok();
        f.member = false;
        assert!(launch_admitted(&mut f, &req(), &GOOD_CAPS).is_err());
        assert!(!f.calls.contains(&"resume"));

        let mut f = Fake::ok();
        f.readback_ok = false;
        assert!(launch_admitted(&mut f, &req(), &GOOD_CAPS).is_err());
        assert!(!f.calls.contains(&"resume"));
        assert_eq!(
            f.calls.last(),
            Some(&"remove_confinement"),
            "removed after confirmed empty"
        );

        let mut f = Fake::ok();
        f.identity = (4242, 0);
        assert!(launch_admitted(&mut f, &req(), &GOOD_CAPS).is_err());
        assert!(!f.calls.contains(&"resume"));
    }

    #[test]
    fn unconfirmed_empty_job_leaves_confinement_installed() {
        let mut f = Fake::ok();
        f.readback_ok = false;
        f.empty_confirmed = false;
        let err = launch_admitted(&mut f, &req(), &GOOD_CAPS)
            .err()
            .expect("err");
        assert!(!f.calls.contains(&"remove_confinement"));
        assert_eq!(
            f.calls.last(),
            Some(&"retain_confinement"),
            "installed net must be retained"
        );

        // Before install there is no net, but the impl is still told to retain.
        let mut f = Fake::ok();
        f.fail_at = Some("assign_to_job");
        f.empty_confirmed = false;
        assert!(launch_admitted(&mut f, &req(), &GOOD_CAPS).is_err());
        assert_eq!(f.calls.last(), Some(&"retain_nothing"));
        assert!(format!("{err}").contains("left installed"), "{err}");
    }

    #[test]
    fn current_build_reports_every_blocker_in_stable_order() {
        use ManagedLaunchBlocker as B;
        assert_eq!(
            assess_managed_launch(&HostFacts::current_build(Some(false))),
            [
                B::WrapperPidIsNotServicePid,
                B::NoProductionLauncher,
                B::NoPerRunWfpConfinement,
                B::WfpArbitrationUnverified,
                B::NotElevated
            ]
        );
        // Elevation never removes the unverified launcher/confinement blockers.
        let elevated = assess_managed_launch(&HostFacts::current_build(Some(true)));
        assert_eq!(
            elevated,
            [
                B::WrapperPidIsNotServicePid,
                B::NoProductionLauncher,
                B::NoPerRunWfpConfinement,
                B::WfpArbitrationUnverified
            ]
        );
        assert!(!elevated.contains(&B::NotElevated));
        // A failed probe is a blocker of its own, not "assume elevated".
        assert!(
            assess_managed_launch(&HostFacts::current_build(None)).contains(&B::ElevationUnknown)
        );
    }

    #[test]
    fn preflight_only_passes_when_every_blocker_is_cleared() {
        let all_clear = HostFacts {
            elevated: Some(true),
            production_launcher: true,
            wrapper_reports_service_identity: true,
            caps: GOOD_CAPS,
        };
        assert!(managed_spawn_preflight_with(&all_clear).is_ok());
        for broken in [
            HostFacts {
                elevated: Some(false),
                ..all_clear
            },
            HostFacts {
                elevated: None,
                ..all_clear
            },
            HostFacts {
                production_launcher: false,
                ..all_clear
            },
            HostFacts {
                wrapper_reports_service_identity: false,
                ..all_clear
            },
            HostFacts {
                caps: ConfinementCapabilities {
                    per_run_wfp_confinement: false,
                    ..GOOD_CAPS
                },
                ..all_clear
            },
            HostFacts {
                caps: ConfinementCapabilities {
                    wfp_arbitration_verified: false,
                    ..GOOD_CAPS
                },
                ..all_clear
            },
        ] {
            assert!(matches!(
                managed_spawn_preflight_with(&broken),
                Err(SandboxError::UnsupportedPolicy { .. })
            ));
        }
    }

    #[test]
    fn current_build_refusal_is_typed_and_names_only_external_blockers() {
        match managed_spawn_preflight_with(&HostFacts::current_build(Some(false))) {
            Err(SandboxError::UnsupportedPolicy {
                feature, reason, ..
            }) => {
                assert_eq!(feature, "managed_spawn");
                assert!(!reason.contains("wrapper PID"), "{reason}");
                assert!(reason.contains("5 blocker(s)"), "{reason}");
                assert!(reason.contains("arbitration"), "{reason}");
            }
            other => panic!("expected UnsupportedPolicy, got {other:?}"),
        }
    }

    #[test]
    fn current_capabilities_stay_all_false_until_a_verified_impl_exists() {
        assert_eq!(CURRENT_CAPABILITIES, ConfinementCapabilities::default());
    }

    #[test]
    fn teardown_removes_confinement_only_after_empty_confirmation() {
        let mut f = Fake::ok();
        let svc = launch_admitted(&mut f, &req(), &GOOD_CAPS).unwrap();
        f.calls.clear();
        teardown(&mut f, svc).unwrap();
        assert_eq!(f.calls, ["terminate", "remove_confinement"]);

        let mut f = Fake::ok();
        let svc = launch_admitted(&mut f, &req(), &GOOD_CAPS).unwrap();
        f.calls.clear();
        f.empty_confirmed = false;
        assert!(teardown(&mut f, svc).is_err());
        assert_eq!(f.calls, ["terminate", "retain_confinement"]);
    }

    fn sid() -> RunSid {
        RunSid::parse("S-1-15-2-1234-5678-9012").unwrap()
    }

    fn plan() -> ConfinementPlan {
        plan_network_confinement(&req(), &GOOD_CAPS).unwrap()
    }

    #[test]
    fn run_sid_rejects_malformed_values() {
        for bad in [
            "",
            "S-1-",
            "S-2-15-2-1",
            "S-1-15-2-x",
            "S-1-15--2",
            "S-1-15-2-1 ",
            "SS-1-15-2-1",
        ] {
            assert!(RunSid::parse(bad).is_err(), "{bad:?}");
        }
        assert!(RunSid::parse("S-1-15-2-1-2-3").is_ok());
    }

    #[test]
    fn filter_specs_are_exact_default_deny_scoped_to_run_sid() {
        let specs = build_filter_specs(&plan(), &sid());
        // 6 layers x (1 block + 2 permits for ports 9223, 18790)
        assert_eq!(specs.len(), 18);
        assert!(specs.iter().all(|s| s.run_sid == sid().as_str()));
        for s in &specs {
            match s.action {
                WfpAction::Permit => {
                    assert!(
                        matches!(s.matcher, WfpMatch::LoopbackTcpPort { .. }),
                        "{s:?}"
                    );
                    assert_eq!(s.weight, PERMIT_WEIGHT);
                }
                WfpAction::Block => {
                    assert_eq!(s.matcher, WfpMatch::Any);
                    assert!(s.weight < PERMIT_WEIGHT);
                }
            }
        }
        for layer in WfpLayer::ALL {
            let blocks = specs
                .iter()
                .filter(|s| s.layer == layer && s.action == WfpAction::Block)
                .count();
            assert_eq!(blocks, 1, "{layer:?}");
        }
        let ports: std::collections::BTreeSet<u16> = specs
            .iter()
            .filter_map(|s| match s.matcher {
                WfpMatch::LoopbackTcpPort { port, .. } => Some(port),
                WfpMatch::Any => None,
            })
            .collect();
        assert_eq!(ports.into_iter().collect::<Vec<_>>(), vec![9223, 18790]);
    }

    #[test]
    fn filter_port_side_follows_layer_direction() {
        for s in build_filter_specs(&plan(), &sid()) {
            if let WfpMatch::LoopbackTcpPort { remote, .. } = s.matcher {
                assert_eq!(
                    remote,
                    matches!(s.layer, WfpLayer::ConnectV4 | WfpLayer::ConnectV6),
                    "{s:?}"
                );
            }
        }
    }

    #[test]
    fn filter_readback_detects_missing_extra_and_changed() {
        let expected = build_filter_specs(&plan(), &sid());
        let mut shuffled = expected.clone();
        shuffled.reverse();
        assert!(verify_filter_readback(&expected, &shuffled).is_ok());

        let mut missing = expected.clone();
        missing.pop();
        assert!(verify_filter_readback(&expected, &missing).is_err());

        let mut extra = expected.clone();
        extra.push(FilterSpec {
            layer: WfpLayer::ConnectV4,
            action: WfpAction::Permit,
            weight: PERMIT_WEIGHT,
            run_sid: sid().as_str().to_string(),
            matcher: WfpMatch::Any,
        });
        assert!(verify_filter_readback(&expected, &extra).is_err());

        let mut changed = expected.clone();
        changed[1].weight = 14;
        assert!(verify_filter_readback(&expected, &changed).is_err());
    }
}

/// Real-OS evidence for the [`native`] building blocks (suspended handle identity,
/// Job admission, resume-count check, bounded kill-and-confirm-empty). The target is the
/// system `ping.exe` against loopback: no HOME / keyring / `.aleph` / network-policy state is
/// touched, no token or WFP object is created, and the process is created SUSPENDED and
/// killed through its own job handle (never by an unverified PID). This does NOT prove
/// sandboxing: the process runs under the host token (test-only creation helper).
#[cfg(all(test, target_os = "windows"))]
mod native_tests {
    use super::native::{terminate_and_confirm_empty, SuspendedProcess};
    use crate::sandbox::platforms::windows::job::SandboxJob;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
        CREATE_SUSPENDED, PROCESS_INFORMATION, STARTUPINFOW,
    };

    /// Test-only: host-token suspended creation. Production creation (token/AppContainer +
    /// stdio pipes) intentionally does not exist yet.
    fn spawn_suspended_ping() -> SuspendedProcess {
        let root = std::env::var("SystemRoot").expect("SystemRoot");
        let exe = format!("{root}\\System32\\ping.exe");
        assert!(std::path::Path::new(&exe).is_file(), "{exe} missing");
        let mut cmd: Vec<u16> = format!("\"{exe}\" -n 30 127.0.0.1")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: zeroed POD structs; `cb` set below.
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `cmd` is a mutable NUL-terminated buffer; no handles are inherited.
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            )
        };
        assert_ne!(ok, 0, "CreateProcessW failed: {}", unsafe {
            GetLastError()
        });
        // SAFETY: handles come from the successful CREATE_SUSPENDED call above.
        unsafe { SuspendedProcess::from_raw(pi.hProcess, pi.hThread) }
    }

    fn exit_code(p: &SuspendedProcess) -> u32 {
        let mut code = 0u32;
        // SAFETY: valid process handle and out-pointer.
        assert_ne!(
            unsafe { GetExitCodeProcess(p.process_handle(), &mut code) },
            0
        );
        code
    }

    #[test]
    fn suspended_identity_comes_from_the_owned_handle_and_never_runs_before_admission() {
        let job = SandboxJob::new_single_process(None).expect("job");
        let proc = spawn_suspended_ping();

        let id = proc.read_identity().expect("identity from handle");
        assert_ne!(id.pid(), 0);
        assert_ne!(
            id.pid(),
            std::process::id(),
            "service identity is not this (parent) process"
        );
        assert_ne!(id.creation_filetime(), 0);
        // Reading twice yields the same (pid, creation) pair: it is a stable handle identity.
        assert_eq!(proc.read_identity().unwrap(), id);

        // Admission: not a member before assign, member after (kernel readback), still suspended.
        assert!(!job.contains_process(proc.process_handle()).unwrap());
        // SAFETY: valid live process handle owned by `proc`.
        unsafe { job.assign_process(proc.process_handle()) }.expect("assign");
        assert!(job.contains_process(proc.process_handle()).unwrap());
        assert_eq!(job.active_process_count().unwrap(), 1);
        assert!(
            !proc.wait_exit(Duration::from_millis(100)).unwrap(),
            "must not run while suspended"
        );

        // First resume is accepted (suspend count 1); a second one violates the ordering.
        proc.resume().expect("first resume");
        let again = proc.resume().expect_err("second resume must be refused");
        assert!(again.contains("expected 1"), "{again}");

        assert!(terminate_and_confirm_empty(&job, &proc, Duration::from_secs(10)).unwrap());
        assert_eq!(job.active_process_count().unwrap(), 0);
        assert_eq!(
            exit_code(&proc),
            1,
            "terminated by the job (exit code 1), not by itself"
        );
    }

    #[test]
    fn unresumed_suspended_process_is_reaped_through_the_job_handle() {
        let job = SandboxJob::new_single_process(None).expect("job");
        let proc = spawn_suspended_ping();
        // SAFETY: valid live process handle owned by `proc`.
        unsafe { job.assign_process(proc.process_handle()) }.expect("assign");
        assert!(job.contains_process(proc.process_handle()).unwrap());

        assert!(terminate_and_confirm_empty(&job, &proc, Duration::from_secs(10)).unwrap());
        assert_eq!(job.active_process_count().unwrap(), 0);
        assert_eq!(exit_code(&proc), 1);
        // Reaped and confirmed empty; a late resume must fail rather than touch a dead thread.
        assert!(proc.resume().is_err());
    }

    #[test]
    fn terminate_on_a_process_outside_the_job_is_not_confirmed_empty() {
        // The job never received the process: killing "the job" cannot reap it, so the
        // bounded wait must report Ok(false) (confinement stays) instead of a false success.
        let job = SandboxJob::new_single_process(None).expect("job");
        let proc = spawn_suspended_ping();
        let confirmed =
            terminate_and_confirm_empty(&job, &proc, Duration::from_millis(300)).unwrap();
        assert!(
            !confirmed,
            "process outside the job must not be reported reaped"
        );
        // Cleanup of this test's own child via its own handle (not an unverified PID).
        // SAFETY: valid process handle owned by `proc`.
        unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(proc.process_handle(), 1)
        };
        assert!(proc.wait_exit(Duration::from_secs(10)).unwrap());
    }
}
