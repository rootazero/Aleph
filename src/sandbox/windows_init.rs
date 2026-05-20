//! SP-3a — Windows restricted-token + Low-IL init binary logic.
//!
//! Invoked as `aleph-server sandbox-init-windows --policy <json> --
//! <target> <target-args...>` by `WindowsSandboxDriver::run`. Lives in
//! a hidden CLI subcommand on the existing aleph-server binary (no
//! separate helper artifact — R3 core minimalism).
//!
//! The init prelude runs inside the JobObject that the driver already
//! assigned, *before* the untrusted target gets to execute. That's the
//! correct security hook point: the process container is already in
//! place, but the primary token is still the host's (full privileges,
//! Medium integrity). We derive a Chrome-pattern restricted token from
//! our own and use `CreateProcessAsUserW` to launch the target under it,
//! at Low integrity.
//!
//! Cross-platform parts (policy struct, JSON shape, argv parser) are not
//! gated, so they compile + unit-test on macOS / Linux dev boxes. The
//! actual `apply_*` and `run_init` Win32 entry point is
//! `#[cfg(target_os = "windows")]`-gated.

use serde::{Deserialize, Serialize};

/// Policy passed from `WindowsSandboxDriver::run` to `sandbox-init-windows`
/// via JSON on argv. Bounded by capability count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WindowsInitPolicy {
    /// When `true`, the init exits non-zero if `CreateProcessAsUserW`
    /// fails with `ERROR_PRIVILEGE_NOT_HELD` (host lacks
    /// `SE_INCREASE_QUOTA`). Default `false` → soft-degrade to
    /// `CreateProcessW` with the host token (cycle 1 behavior).
    /// JobObject containment continues to apply either way.
    #[serde(default)]
    pub require_restricted_token: bool,
}

/// Top-level entry point for the `sandbox-init-windows` subcommand. Never
/// returns: either calls `ExitProcess` with the target's exit code, or
/// `ExitProcess`es with a diagnostic code on init-side failure.
///
/// Exit codes (per spec §5):
/// - 64 → restricted token required but unavailable (`require_restricted_token=true`)
/// - 65 → unrecoverable Win32 setup error (OpenProcessToken / CreateRestrictedToken / SetTokenInformation / WaitForSingleObject / GetExitCodeProcess)
/// - 66 → argv parse failure
/// - 67 → all spawn paths failed (neither `CreateProcessAsUserW` nor `CreateProcessW`)
/// - 78 (`EX_CONFIG`) → invoked on a non-Windows host
#[cfg(target_os = "windows")]
pub fn run_init(args: Vec<String>) -> ! {
    let parsed = match parse_init_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("aleph sandbox-init-windows: argv parse failed: {e}");
            std::process::exit(66);
        }
    };

    let exit_code = match imp::launch_with_restricted_token(&parsed) {
        Ok(code) => code,
        Err(imp::LaunchError::PrivilegeNotHeld) if !parsed.policy.require_restricted_token => {
            eprintln!(
                "aleph sandbox-init-windows: restricted token unavailable \
                 (ERROR_PRIVILEGE_NOT_HELD); falling back to plain CreateProcessW \
                 (JobObject containment still applies)"
            );
            match imp::launch_with_host_token(&parsed) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("aleph sandbox-init-windows: fallback CreateProcessW failed: {e:?}");
                    std::process::exit(67);
                }
            }
        }
        Err(imp::LaunchError::PrivilegeNotHeld) => {
            eprintln!(
                "aleph sandbox-init-windows: restricted token required \
                 (ERROR_PRIVILEGE_NOT_HELD) and require_restricted_token=true"
            );
            std::process::exit(64);
        }
        Err(imp::LaunchError::SetupFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: setup failed: {msg}");
            std::process::exit(65);
        }
        Err(imp::LaunchError::SpawnFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: spawn failed: {msg}");
            std::process::exit(67);
        }
        Err(imp::LaunchError::WaitFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: wait failed: {msg}");
            std::process::exit(65);
        }
    };

    std::process::exit(exit_code);
}

#[cfg(not(target_os = "windows"))]
pub fn run_init(_args: Vec<String>) -> ! {
    eprintln!("aleph sandbox-init-windows: only supported on Windows");
    std::process::exit(78); // EX_CONFIG
}

/// Output of `parse_init_args`. `target_args` is the slice after `--`.
/// `dead_code` allow: only the Windows `run_init` consumes it; unit tests
/// reference it cross-platform.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct ParsedInitArgs {
    pub(crate) policy: WindowsInitPolicy,
    pub(crate) target: String,
    pub(crate) target_args: Vec<String>,
}

/// argv layout: `[--policy <json> -- <target> <target-args...>]`.
/// The leading `sandbox-init-windows` subcommand name is stripped by
/// the CLI dispatcher before calling `run_init`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_init_args(args: &[String]) -> Result<ParsedInitArgs, String> {
    let mut policy_json: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--policy" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--policy requires a value".to_string())?;
                policy_json = Some(v.as_str());
                i += 2;
            }
            "--" => {
                i += 1;
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let policy_str = policy_json.ok_or_else(|| "missing --policy".to_string())?;
    let policy: WindowsInitPolicy = serde_json::from_str(policy_str)
        .map_err(|e| format!("--policy JSON parse error: {e}"))?;

    let target = args
        .get(i)
        .ok_or_else(|| "missing target program after `--`".to_string())?
        .clone();
    let target_args = args[i + 1..].to_vec();

    Ok(ParsedInitArgs {
        policy,
        target,
        target_args,
    })
}

// ---------------------------------------------------------------------------
// Windows-only implementation.
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod imp {
    use super::ParsedInitArgs;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    // LocalFree lives in Foundation in windows-sys 0.61+ (was Memory in 0.59).
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, INVALID_HANDLE_VALUE, HANDLE,
    };
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, SetTokenInformation, TokenIntegrityLevel,
        DISABLE_MAX_PRIVILEGE, SE_GROUP_INTEGRITY, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY,
        TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessAsUserW, CreateProcessW, GetCurrentProcess, GetExitCodeProcess,
        OpenProcessToken, WaitForSingleObject, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
        STARTUPINFOW,
    };

    /// Per-error-class outcome of a launch attempt.
    #[derive(Debug)]
    pub(super) enum LaunchError {
        PrivilegeNotHeld,
        SetupFailed(String),
        SpawnFailed(String),
        WaitFailed(String),
    }

    pub(super) fn launch_with_restricted_token(
        parsed: &ParsedInitArgs,
    ) -> Result<i32, LaunchError> {
        // 1. Open self token.
        let host_token = open_self_token()?;

        // 2. Derive restricted token (no privs except SeChangeNotify).
        let restricted = create_restricted(host_token).inspect_err(|_| unsafe {
            CloseHandle(host_token);
        })?;

        // 3. Drop integrity to Low.
        if let Err(e) = set_integrity_low(restricted) {
            unsafe {
                CloseHandle(restricted);
                CloseHandle(host_token);
            }
            return Err(e);
        }

        // 4. Spawn target with the restricted token.
        let result = spawn_and_wait(parsed, Some(restricted));

        unsafe {
            CloseHandle(restricted);
            CloseHandle(host_token);
        }
        result
    }

    pub(super) fn launch_with_host_token(parsed: &ParsedInitArgs) -> Result<i32, LaunchError> {
        spawn_and_wait(parsed, None)
    }

    fn open_self_token() -> Result<HANDLE, LaunchError> {
        let mut token: HANDLE = std::ptr::null_mut();
        let ok = unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE
                    | TOKEN_QUERY
                    | TOKEN_ASSIGN_PRIMARY
                    | TOKEN_ADJUST_DEFAULT,
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
        let ok = unsafe { ConvertStringSidToSidW(low_sid_str.as_ptr(), &mut sid) };
        if ok == 0 {
            return Err(LaunchError::SetupFailed(format!(
                "ConvertStringSidToSidW failed: {:#010x}",
                unsafe { GetLastError() }
            )));
        }

        let label = TOKEN_MANDATORY_LABEL {
            Label: windows_sys::Win32::Security::SID_AND_ATTRIBUTES {
                Sid: sid as *mut _,
                Attributes: SE_GROUP_INTEGRITY,
            },
        };
        let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32
            + unsafe { windows_sys::Win32::Security::GetLengthSid(sid as *mut _) };
        let ok = unsafe {
            SetTokenInformation(
                token,
                TokenIntegrityLevel,
                &label as *const _ as *const _,
                size,
            )
        };
        let last = unsafe { GetLastError() };
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

    fn spawn_and_wait(
        parsed: &ParsedInitArgs,
        token: Option<HANDLE>,
    ) -> Result<i32, LaunchError> {
        // Build a mutable wide-char command line. CreateProcess writes
        // into the buffer, so we must own it.
        let cmd_line_str = build_command_line(&parsed.target, &parsed.target_args);
        let mut cmd_line: Vec<u16> = cmd_line_str
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        si.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        si.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

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

        if pi.hProcess == std::ptr::null_mut() || pi.hProcess == INVALID_HANDLE_VALUE {
            return Err(LaunchError::SpawnFailed(
                "CreateProcess succeeded but returned NULL process HANDLE".into(),
            ));
        }

        // Wait for target.
        let wait_result = unsafe { WaitForSingleObject(pi.hProcess, INFINITE) };
        if wait_result != 0 {
            unsafe {
                CloseHandle(pi.hThread);
                CloseHandle(pi.hProcess);
            }
            return Err(LaunchError::WaitFailed(format!(
                "WaitForSingleObject returned {wait_result:#010x}"
            )));
        }

        let mut code: u32 = 0;
        let ok = unsafe { GetExitCodeProcess(pi.hProcess, &mut code) };
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_round_trips_through_json() {
        let original = WindowsInitPolicy {
            require_restricted_token: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: WindowsInitPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn policy_default_does_not_require_restricted_token() {
        let p = WindowsInitPolicy::default();
        assert!(!p.require_restricted_token);
        // Round-trip through JSON via default also.
        let json = serde_json::to_string(&p).unwrap();
        let parsed: WindowsInitPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, p);
    }

    #[test]
    fn policy_accepts_missing_require_flag_via_serde_default() {
        // Spec § 2.5 promises the policy struct is forward-compatible:
        // a JSON like `{}` deserializes with default values.
        let parsed: WindowsInitPolicy = serde_json::from_str("{}").unwrap();
        assert!(!parsed.require_restricted_token);
    }

    #[test]
    fn parse_init_args_extracts_policy_and_target() {
        let policy = WindowsInitPolicy {
            require_restricted_token: true,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let argv = vec![
            "--policy".to_string(),
            json,
            "--".to_string(),
            "C:\\Windows\\System32\\cmd.exe".to_string(),
            "/c".to_string(),
            "echo hi".to_string(),
        ];
        let parsed = parse_init_args(&argv).unwrap();
        assert_eq!(parsed.policy, policy);
        assert_eq!(parsed.target, "C:\\Windows\\System32\\cmd.exe");
        assert_eq!(parsed.target_args, vec!["/c", "echo hi"]);
    }

    #[test]
    fn parse_init_args_rejects_missing_policy() {
        let argv = vec!["--".to_string(), "cmd.exe".to_string()];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("missing --policy"), "got: {err}");
    }

    #[test]
    fn parse_init_args_rejects_missing_target() {
        let argv = vec![
            "--policy".to_string(),
            "{}".to_string(),
            "--".to_string(),
        ];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("missing target"), "got: {err}");
    }

    #[test]
    fn parse_init_args_rejects_bad_json() {
        let argv = vec![
            "--policy".to_string(),
            "not json".to_string(),
            "--".to_string(),
            "cmd.exe".to_string(),
        ];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("JSON parse error"), "got: {err}");
    }
}
