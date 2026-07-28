//! Windows-only Win32 launch implementation for `sandbox-init-windows`.
//!
//! This module owns the shared launch types ([`LaunchError`],
//! [`LocalFreeGuard`]), the SP-3a restricted-token + Low-integrity path,
//! the workspace DACL primitives ([`DaclMutex`], [`set_workspace_dacl_entry`]),
//! and the command-line builder. The SP-6 AppContainer path lives in the
//! [`app_container`] submodule.

use super::args::ParsedInitArgs;
use super::policy::{DACL_INHERIT_FLAGS_FOR_APPCONTAINER, DACL_SERIALIZATION_MUTEX_NAME};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

// LocalFree lives in Foundation in windows-sys 0.61+ (was Memory in 0.59).
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{ConvertStringSidToSidW, ACCESS_MODE};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, SetTokenInformation, TokenIntegrityLevel, DISABLE_MAX_PRIVILEGE,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, CreateProcessW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    WaitForSingleObject, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
};

mod app_container;
pub(super) use app_container::launch_with_app_container;

/// SP-6 v2: RAII guard that calls `LocalFree` on `Drop`. Used for
/// `PSECURITY_DESCRIPTOR` (returned by `GetNamedSecurityInfoW`'s
/// `ppSecurityDescriptor` out-param) and `PACL` (returned by
/// `SetEntriesInAclW`'s `NewAcl` out-param) — both system-
/// allocated and documented to be freed with `LocalFree`.
///
/// IMPORTANT: do NOT wrap the `ppDacl` output of
/// `GetNamedSecurityInfoW` in this guard — that pointer is interior
/// to the security descriptor buffer and is freed transitively when
/// the surrounding `PSECURITY_DESCRIPTOR` is freed. Wrapping it
/// would double-free.
///
/// Holds a raw `*mut c_void`. Caller is responsible for not double-
/// freeing — we don't take ownership beyond running `LocalFree` on
/// drop.
struct LocalFreeGuard(*mut core::ffi::c_void);

impl Drop for LocalFreeGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe {
                // SAFETY: `self.0` is a non-null pointer returned by a Windows
                // API that must be freed with `LocalFree`.
                LocalFree(self.0)
            };
        }
    }
}

/// Per-error-class outcome of a launch attempt.
#[derive(Debug)]
pub(super) enum LaunchError {
    PrivilegeNotHeld,
    SetupFailed(String),
    SpawnFailed(String),
    WaitFailed(String),
    /// SP-6: AppContainer-specific setup failure (CreateAppContainerProfile,
    /// SECURITY_CAPABILITIES wiring, etc.). Distinguished from
    /// `SetupFailed` so the caller can decide whether to soft-degrade
    /// to the SP-3a restricted-token path.
    AppContainerSetupFailed(String),
}

pub(super) fn launch_with_restricted_token(parsed: &ParsedInitArgs) -> Result<i32, LaunchError> {
    // 1. Open self token.
    let host_token = open_self_token()?;

    // 2. Derive restricted token (no privs except SeChangeNotify).
    // rust-doctor-disable-next-line unsafe-block-audit
    let restricted = create_restricted(host_token).inspect_err(|_| unsafe {
        CloseHandle(host_token);
    })?;

    // 3. Drop integrity to Low.
    if let Err(e) = set_integrity_low(restricted) {
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe {
            CloseHandle(restricted);
            CloseHandle(host_token);
        }
        return Err(e);
    }

    // 4. Spawn target with the restricted token.
    let result = spawn_and_wait(parsed, Some(restricted));

    // SAFETY: `restricted` and `host_token` are valid owned token handles
    // obtained above; closing them exactly once here is required cleanup.
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        CloseHandle(restricted);
        CloseHandle(host_token);
    }
    result
}

pub(super) fn launch_with_host_token(parsed: &ParsedInitArgs) -> Result<i32, LaunchError> {
    spawn_and_wait(parsed, None)
}

/// Cycle 8: RAII handle on the session-local DACL serialization mutex
/// (see [`super::super::policy::DACL_SERIALIZATION_MUTEX_NAME`]). [`acquire`]
/// blocks until the mutex is free — bounded by `WAIT_TIMEOUT_MS` so a wedged
/// peer cannot deadlock a spawn — and `Drop` releases + closes it.
///
/// Best-effort by construction: if `CreateMutexW` or the wait fails (or
/// times out), the guard holds a NULL handle and is a no-op, so the
/// caller's read-modify-write proceeds unserialized — identical to the
/// pre-Cycle-8 behaviour. A hardening primitive must never turn a
/// working spawn into a failed one.
///
/// [`acquire`]: DaclMutex::acquire
struct DaclMutex {
    handle: HANDLE,
    acquired: bool,
}

impl DaclMutex {
    fn acquire() -> Self {
        use std::iter::once;
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name_w: Vec<u16> = DACL_SERIALIZATION_MUTEX_NAME
            .encode_utf16()
            .chain(once(0))
            .collect();
        // bInitialOwner = FALSE: every acquirer goes through the wait
        // below, so the abandoned-owner case is handled uniformly.
        // rust-doctor-disable-next-line unsafe-block-audit
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name_w.as_ptr()) };
        if handle.is_null() {
            return Self {
                handle: std::ptr::null_mut(),
                acquired: false,
            };
        }

        // WAIT_OBJECT_0 → acquired cleanly. WAIT_ABANDONED → a prior
        // holder died without releasing; we still own the mutex now and
        // re-read the DACL under it anyway, so the path is identical.
        const WAIT_OBJECT_0: u32 = 0x0000_0000;
        const WAIT_ABANDONED: u32 = 0x0000_0080;
        const WAIT_TIMEOUT_MS: u32 = 10_000;
        // rust-doctor-disable-next-line unsafe-block-audit
        let rc = unsafe { WaitForSingleObject(handle, WAIT_TIMEOUT_MS) };
        if rc == WAIT_OBJECT_0 || rc == WAIT_ABANDONED {
            Self {
                handle,
                acquired: true,
            }
        } else {
            // Timed out or wait failed — drop the lock and proceed
            // lock-free rather than block the spawn indefinitely.
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe { CloseHandle(handle) };
            Self {
                handle: std::ptr::null_mut(),
                acquired: false,
            }
        }
    }
}

impl Drop for DaclMutex {
    fn drop(&mut self) {
        if self.handle.is_null() {
            return;
        }
        use windows_sys::Win32::System::Threading::ReleaseMutex;
        if self.acquired {
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe { ReleaseMutex(self.handle) };
        }
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { CloseHandle(self.handle) };
    }
}

/// SP-6 v2 / Cycle 3: Add or remove an inheritable ACE for `ac_sid`
/// on `target_path` with the supplied access mask. Same code path
/// for grant (`mode = GRANT_ACCESS`), deny (`mode = DENY_ACCESS`),
/// and revoke (`mode = REVOKE_ACCESS`) — the only differences are
/// `mode` and `permission_mask` inside `EXPLICIT_ACCESS_W`.
///
/// Cycle 2 wired this only for the workspace root with `GENERIC_ALL`
/// plus `GRANT_ACCESS`/`REVOKE_ACCESS`. Cycle 3 generalises so the
/// caller can also stamp `DENY_ACCESS` ACEs on protected metadata
/// subpaths (`.git`, `.aleph`, …) with a narrower mask. Canonical
/// ACL ordering is handled by `SetEntriesInAclW`, which automatically
/// places deny ACEs before allow ACEs in the merged DACL.
///
/// Best-effort: any failure returns `Err(String)` and the caller
/// logs + continues. Never panics.
///
/// Caveat: assumes `target_path` is already canonical (the driver
/// populates the policy with the resolved session workspace dir;
/// we do not resolve symlinks here). On `REVOKE_ACCESS`, the
/// `permission_mask` is ignored by Windows — every ACE for the
/// trustee is removed regardless. We still pass it for clarity.
// rust-doctor-disable-next-line unsafe-block-audit
unsafe fn set_workspace_dacl_entry(
    target_path: &str,
    ac_sid: *mut core::ffi::c_void,
    mode: ACCESS_MODE,
    permission_mask: u32,
) -> Result<(), String> {
    use std::iter::once;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
        SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
    };
    use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION};

    // Cycle 8: serialize the read-modify-write below against every other
    // concurrent `sandbox-init-windows` process touching the same path's
    // DACL. Held only for this single path's RMW — released on return —
    // so concurrent inits still run their targets in parallel; only the
    // DACL mutations are serialized. Fail-soft: a no-op guard (mutex
    // unavailable) just reproduces the pre-Cycle-8 unserialized path.
    let _dacl_guard = DaclMutex::acquire();

    let path_w: Vec<u16> = target_path.encode_utf16().chain(once(0)).collect();

    // 1. Read existing DACL so we merge (not replace).
    let mut old_dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
    let status = GetNamedSecurityInfoW(
        path_w.as_ptr(),
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut old_dacl,
        std::ptr::null_mut(),
        &mut sd,
    );
    if status != ERROR_SUCCESS {
        return Err(format!(
            "GetNamedSecurityInfoW({target_path}) failed: {status:#010x}"
        ));
    }
    let _sd_guard = LocalFreeGuard(sd);

    // 2. Build EXPLICIT_ACCESS_W for the AppContainer SID.
    let mut ea: EXPLICIT_ACCESS_W = std::mem::zeroed();
    ea.grfAccessPermissions = permission_mask;
    ea.grfAccessMode = mode;
    ea.grfInheritance = DACL_INHERIT_FLAGS_FOR_APPCONTAINER;
    ea.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    ea.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN;
    ea.Trustee.ptstrName = ac_sid as *mut u16;
    // pMultipleTrustee / MultipleTrusteeOperation already zeroed
    // (= NO_MULTIPLE_TRUSTEE).

    // 3. Merge ACE into existing DACL.
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    let status = SetEntriesInAclW(1, &ea, old_dacl, &mut new_dacl);
    if status != ERROR_SUCCESS {
        return Err(format!(
            "SetEntriesInAclW({target_path}) failed: {status:#010x}"
        ));
    }
    let _dacl_guard = LocalFreeGuard(new_dacl as *mut core::ffi::c_void);

    // 4. Write the merged DACL back.
    let status = SetNamedSecurityInfoW(
        path_w.as_ptr() as *mut u16,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        new_dacl,
        std::ptr::null_mut(),
    );
    if status != ERROR_SUCCESS {
        return Err(format!(
            "SetNamedSecurityInfoW({target_path}) failed: {status:#010x}"
        ));
    }
    Ok(())
}

fn open_self_token() -> Result<HANDLE, LaunchError> {
    let mut token: HANDLE = std::ptr::null_mut();
    // rust-doctor-disable-next-line unsafe-block-audit
    let ok = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut token,
        )
    };
    if ok == 0 {
        return Err(LaunchError::SetupFailed(format!(
            "OpenProcessToken failed: {:#010x}",
            unsafe { GetLastError() }
        )));
    }
    Ok(token)
}

fn create_restricted(host_token: HANDLE) -> Result<HANDLE, LaunchError> {
    let mut restricted: HANDLE = std::ptr::null_mut();
    // rust-doctor-disable-next-line unsafe-block-audit
    let ok = unsafe {
        CreateRestrictedToken(
            host_token,
            DISABLE_MAX_PRIVILEGE,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            &mut restricted,
        )
    };
    if ok == 0 {
        return Err(LaunchError::SetupFailed(format!(
            "CreateRestrictedToken failed: {:#010x}",
            unsafe { GetLastError() }
        )));
    }
    Ok(restricted)
}

fn set_integrity_low(token: HANDLE) -> Result<(), LaunchError> {
    let mut sid: *mut core::ffi::c_void = std::ptr::null_mut();
    let low_sid_str: Vec<u16> = OsStr::new("S-1-16-4096")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // rust-doctor-disable-next-line unsafe-block-audit
    let ok = unsafe { ConvertStringSidToSidW(low_sid_str.as_ptr(), &mut sid) };
    if ok == 0 {
        return Err(LaunchError::SetupFailed(format!(
            "ConvertStringSidToSidW failed: {:#010x}",
            unsafe { GetLastError() }
        )));
    }

    use windows_sys::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
    let label = TOKEN_MANDATORY_LABEL {
        Label: windows_sys::Win32::Security::SID_AND_ATTRIBUTES {
            Sid: sid as *mut _,
            Attributes: SE_GROUP_INTEGRITY as u32,
        },
    };
    let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32
        // rust-doctor-disable-next-line unsafe-block-audit
        + unsafe { windows_sys::Win32::Security::GetLengthSid(sid as *mut _) };
    // rust-doctor-disable-next-line unsafe-block-audit
    let ok = unsafe {
        SetTokenInformation(
            token,
            TokenIntegrityLevel,
            &label as *const _ as *const _,
            size,
        )
    };
    // rust-doctor-disable-next-line unsafe-block-audit
    let last = unsafe { GetLastError() };
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        LocalFree(sid);
    }
    if ok == 0 {
        return Err(LaunchError::SetupFailed(format!(
            "SetTokenInformation(IL=Low) failed: {last:#010x}"
        )));
    }
    Ok(())
}

fn spawn_and_wait(parsed: &ParsedInitArgs, token: Option<HANDLE>) -> Result<i32, LaunchError> {
    // Build a mutable wide-char command line. CreateProcess writes
    // into the buffer, so we must own it.
    let cmd_line_str = build_command_line(&parsed.target, &parsed.target_args);
    let mut cmd_line: Vec<u16> = cmd_line_str
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // rust-doctor-disable-next-line unsafe-block-audit
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES;
    // rust-doctor-disable-next-line unsafe-block-audit
    si.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    // rust-doctor-disable-next-line unsafe-block-audit
    si.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    // rust-doctor-disable-next-line unsafe-block-audit
    si.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

    // rust-doctor-disable-next-line unsafe-block-audit
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // rust-doctor-disable-next-line unsafe-block-audit
    let ok = unsafe {
        match token {
            Some(t) => CreateProcessAsUserW(
                t,
                std::ptr::null(),
                cmd_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // bInheritHandles = TRUE
                0,
                std::ptr::null_mut(),
                std::ptr::null(),
                &si,
                &mut pi,
            ),
            None => CreateProcessW(
                std::ptr::null(),
                cmd_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // bInheritHandles = TRUE
                0,
                std::ptr::null_mut(),
                std::ptr::null(),
                &si,
                &mut pi,
            ),
        }
    };
    if ok == 0 {
        // rust-doctor-disable-next-line unsafe-block-audit
        let err = unsafe { GetLastError() };
        const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
        if token.is_some() && err == ERROR_PRIVILEGE_NOT_HELD {
            return Err(LaunchError::PrivilegeNotHeld);
        }
        return Err(LaunchError::SpawnFailed(format!(
            "CreateProcess{} failed: {err:#010x}",
            if token.is_some() { "AsUserW" } else { "W" }
        )));
    }

    if pi.hProcess.is_null() || pi.hProcess == INVALID_HANDLE_VALUE {
        return Err(LaunchError::SpawnFailed(
            "CreateProcess succeeded but returned NULL process HANDLE".into(),
        ));
    }

    // Wait for target.
    // rust-doctor-disable-next-line unsafe-block-audit
    let wait_result = unsafe { WaitForSingleObject(pi.hProcess, INFINITE) };
    if wait_result != 0 {
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe {
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        }
        return Err(LaunchError::WaitFailed(format!(
            "WaitForSingleObject returned {wait_result:#010x}"
        )));
    }

    let mut code: u32 = 0;
    // rust-doctor-disable-next-line unsafe-block-audit
    let ok = unsafe { GetExitCodeProcess(pi.hProcess, &mut code) };
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    if ok == 0 {
        return Err(LaunchError::WaitFailed(format!(
            "GetExitCodeProcess failed: {:#010x}",
            unsafe { GetLastError() }
        )));
    }
    Ok(code as i32)
}

/// Build a Windows-conforming command line from program + args. The
/// program goes first (quoted if it contains a space); each arg is
/// quoted using the CommandLineToArgvW-compatible escape rules.
fn build_command_line(program: &str, args: &[String]) -> String {
    let mut line = String::new();
    line.push_str(&quote_arg(program));
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(a));
    }
    line
}

fn quote_arg(arg: &str) -> String {
    // Per Microsoft docs ("Parsing C++ Command-Line Arguments"):
    // - If no special chars (space, tab, ", \), no quoting needed.
    // - Otherwise wrap in quotes and escape internal `\` runs that
    //   are followed by a `"` (or end of string), plus internal `"`.
    if !arg.is_empty()
        && !arg
            .chars()
            .any(|c| c == ' ' || c == '\t' || c == '"' || c == '\\')
    {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                for _ in 0..(backslashes * 2 + 1) {
                    out.push('\\');
                }
                backslashes = 0;
                out.push('"');
            }
            _ => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(c);
            }
        }
    }
    // Trailing backslashes before the closing quote must be doubled.
    for _ in 0..(backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}
