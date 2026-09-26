use std::ffi::c_void;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
use windows_sys::Win32::System::JobObjects::JobObjectBasicUIRestrictions;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::SetInformationJobObject;
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
/// There is one job per process. It is created on first use with only
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and its handle is never closed on
/// purpose. The OS closes the last handle when this process ends, whether by
/// a normal return, `process::exit`, or an abort, and closing the job kills
/// every process assigned to it. That covers more than the unix arm's
/// `atexit`, which an abort skips. Processes that `pid` spawned before it was
/// assigned are not in the job and are not killed.
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
