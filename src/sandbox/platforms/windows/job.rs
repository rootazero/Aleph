use std::ffi::c_void;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::JobObjects::JobObjectBasicAccountingInformation;
use windows_sys::Win32::System::JobObjects::JobObjectBasicUIRestrictions;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::QueryInformationJobObject;
use windows_sys::Win32::System::JobObjects::SetInformationJobObject;
use windows_sys::Win32::System::JobObjects::TerminateJobObject;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_ACCOUNTING_INFORMATION;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_UI_RESTRICTIONS;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_PROCESS_MEMORY;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_DESKTOP;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_DISPLAYSETTINGS;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_EXITWINDOWS;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_GLOBALATOMS;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_HANDLES;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_READCLIPBOARD;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_UILIMIT_WRITECLIPBOARD;

/// Job object wrapper for sandbox resource limits.
///
/// Creates a Windows job object with:
/// - Active process limit (prevents fork bombs)
/// - Kill on job close (cleanup when parent exits)
/// - UI restrictions (limits desktop access)
/// - Optional virtual-memory ceiling per process
pub struct SandboxJob {
    handle: HANDLE,
}

unsafe impl Send for SandboxJob {}
// SAFETY: every `&self` method is a single thread-safe kernel call on the
// job handle (query / assign / terminate); the handle is closed only in Drop.
unsafe impl Sync for SandboxJob {}

impl SandboxJob {
    /// Create a new sandbox job with default restrictions.
    ///
    /// `max_memory_mb`, when `Some`, sets `JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    /// .ProcessMemoryLimit` so each process in the job is killed if it tries
    /// to commit more than that many megabytes.
    ///
    /// # Safety
    /// Caller must ensure the job object is properly closed.
    pub unsafe fn new(
        max_active_processes: u32,
        max_memory_mb: Option<u64>,
    ) -> Result<Self, String> {
        let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
        if handle.is_null() {
            return Err(format!("CreateJobObjectW failed: {}", GetLastError()));
        }

        // Set extended limits
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        let mut flags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        if let Some(mb) = max_memory_mb {
            flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            // SIZE_T is usize in windows-sys bindings. saturating_mul guards
            // against 32-bit-host overflow when mb is large enough to overflow
            // usize::MAX bytes.
            limits.ProcessMemoryLimit = (mb as usize).saturating_mul(1024 * 1024);
        }
        limits.BasicLimitInformation.LimitFlags = flags;
        limits.BasicLimitInformation.ActiveProcessLimit = max_active_processes;

        let ok = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );

        if ok == 0 {
            let _ = CloseHandle(handle);
            return Err(format!(
                "SetInformationJobObject(ExtendedLimit) failed: {}",
                GetLastError()
            ));
        }

        // Set UI restrictions
        let mut ui: JOBOBJECT_BASIC_UI_RESTRICTIONS = std::mem::zeroed();
        ui.UIRestrictionsClass = JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_EXITWINDOWS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD;

        let ok = SetInformationJobObject(
            handle,
            JobObjectBasicUIRestrictions,
            &ui as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        );

        if ok == 0 {
            let _ = CloseHandle(handle);
            return Err(format!(
                "SetInformationJobObject(UIRestrictions) failed: {}",
                GetLastError()
            ));
        }

        Ok(Self { handle })
    }

    /// Assign a process to this job object.
    ///
    /// # Safety
    /// The process handle must be valid and have PROCESS_SET_QUOTA access.
    pub unsafe fn assign_process(&self, process_handle: HANDLE) -> Result<(), String> {
        let ok = AssignProcessToJobObject(self.handle, process_handle);
        if ok == 0 {
            return Err(format!(
                "AssignProcessToJobObject failed: {}",
                GetLastError()
            ));
        }
        Ok(())
    }

    /// Managed single-process job: `ActiveProcessLimit == 1` so the service
    /// cannot fork around the kill/wait contract (`managed_spawn_fork`).
    /// Kill-on-close, UI limits and the optional memory ceiling are the same
    /// as [`SandboxJob::new`].
    #[allow(dead_code)] // consumed by the managed launcher once its production API exists
    pub(crate) fn new_single_process(max_memory_mb: Option<u64>) -> Result<Self, String> {
        // SAFETY: the returned job owns its handle and closes it on Drop.
        unsafe { Self::new(1, max_memory_mb) }
    }

    /// Kernel readback: is `process` a member of THIS job? (`IsProcessInJob`.)
    /// An invalid handle is an `Err`, never `Ok(false)`.
    #[allow(dead_code)]
    pub(crate) fn contains_process(&self, process: HANDLE) -> Result<bool, String> {
        let mut result: windows_sys::core::BOOL = 0;
        // SAFETY: `result` is a valid out-pointer; handles are only read by the kernel.
        let ok = unsafe { IsProcessInJob(process, self.handle, &mut result) };
        if ok == 0 {
            // SAFETY: thread-local error read.
            return Err(format!("IsProcessInJob failed: {}", unsafe {
                GetLastError()
            }));
        }
        Ok(result != 0)
    }

    /// Kernel readback of `ActiveProcesses` for this job.
    #[allow(dead_code)]
    pub(crate) fn active_process_count(&self) -> Result<u32, String> {
        // SAFETY: zeroed POD out-struct, correctly sized.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a valid buffer of the stated length.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // SAFETY: thread-local error read.
            return Err(format!(
                "QueryInformationJobObject(BasicAccounting) failed: {}",
                unsafe { GetLastError() }
            ));
        }
        Ok(info.ActiveProcesses)
    }

    /// Terminate every process in the job. Success only means the request was
    /// accepted; callers must still wait and read [`Self::active_process_count`].
    #[allow(dead_code)]
    pub(crate) fn terminate(&self, exit_code: u32) -> Result<(), String> {
        // SAFETY: `self.handle` is a valid job handle owned by `self`.
        if unsafe { TerminateJobObject(self.handle, exit_code) } == 0 {
            // SAFETY: thread-local error read.
            return Err(format!("TerminateJobObject failed: {}", unsafe {
                GetLastError()
            }));
        }
        Ok(())
    }
}

impl Drop for SandboxJob {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: `self.handle` is a valid, non-null job object handle.
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

/// Tie `pid` to this process: the kernel kills it when this process exits,
/// however that happens. This is the Windows arm of
/// `utils::scratch::reap_on_exit`, and it lives here because its FFI is the
/// same job-object primitive [`SandboxJob`] is built on (R1's
/// process-isolation exception).
///
/// `#[allow(dead_code)]` — only the integration-test harnesses under
/// `tests/*/harness.rs` call this through `scratch::reap_on_exit`, and those
/// binaries are not compiled by the lib-level `cargo clippy -p alephcore`
/// the workflow uses (line 351 of aleph-core-ci.yml). Under `--all-targets`
/// the call sites light up and the lint clears; the lib-only path is a
/// false positive that we suppress here so a pre-commit hook matching CI
/// plus `--all-targets` stays green on every platform.
///
/// There is one job per process. It is created on first use with only
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and its handle is never closed on
/// purpose. The OS closes the last handle when this process ends, whether by
/// a normal return, `process::exit`, or an abort, and closing the job kills
/// every process assigned to it. That covers more than the unix arm's
/// `atexit`, which an abort skips. Processes that `pid` spawned before it was
/// assigned are not in the job and are not killed.
#[allow(dead_code)]
pub(crate) fn kill_on_this_process_exit(pid: u32) -> Result<(), String> {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    // The handle as an integer: `HANDLE` is a raw pointer, which a `static`
    // cannot hold. It is only ever passed back to the kernel.
    static JOB: OnceLock<Result<usize, String>> = OnceLock::new();
    let job = JOB
        .get_or_init(|| {
            // SAFETY: plain job-object creation. On failure the half-made
            // handle is closed before the error is returned.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if handle.is_null() {
                    return Err(format!("CreateJobObjectW failed: {}", GetLastError()));
                }
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    let err = GetLastError();
                    let _ = CloseHandle(handle);
                    return Err(format!(
                        "SetInformationJobObject(KillOnClose) failed: {err}"
                    ));
                }
                Ok(handle as usize)
            }
        })
        .clone()?;

    // SAFETY: `OpenProcess` on a pid this process spawned. The process handle
    // is closed on every path. Assigning does not transfer it: the job keeps
    // its own reference to the process.
    unsafe {
        let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return Err(format!("OpenProcess({pid}) failed: {}", GetLastError()));
        }
        let ok = AssignProcessToJobObject(job as HANDLE, process);
        let err = GetLastError();
        let _ = CloseHandle(process);
        if ok == 0 {
            return Err(format!("AssignProcessToJobObject({pid}) failed: {err}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;

    /// Kernel readback primitives used by the managed launcher: membership,
    /// active-process count, terminate. Spawns `ping` (loopback only; no HOME,
    /// keyring or network state is touched).
    #[test]
    fn single_process_job_readback_and_terminate() {
        let job = SandboxJob::new_single_process(None).expect("job");
        assert_eq!(job.active_process_count().unwrap(), 0);

        let mut child = std::process::Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn ping");
        let handle = child.as_raw_handle() as HANDLE;
        assert!(!job.contains_process(handle).unwrap(), "not yet assigned");
        // SAFETY: `handle` is the live child handle owned by `child`.
        unsafe { job.assign_process(handle) }.expect("assign");
        assert!(job.contains_process(handle).unwrap());
        assert_eq!(job.active_process_count().unwrap(), 1);

        job.terminate(1).expect("terminate");
        let _ = child.wait();
        for _ in 0..100 {
            if job.active_process_count().unwrap() == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("job did not become empty after terminate");
    }
}
