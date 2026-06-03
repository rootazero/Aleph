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

    /// SP-6: try AppContainer first (strongest sandbox primitive).
    /// Soft-degrades to restricted-token (SP-3a) on failure.
    #[serde(default)]
    pub use_app_container: bool,

    /// SP-6: when `true`, refuse to spawn if AppContainer setup fails.
    /// Default `false` → soft-degrade to SP-3a's path.
    #[serde(default)]
    pub require_app_container: bool,

    /// SP-6: capability names (lowercase Win32 form like
    /// `internetClient`) to grant inside the AppContainer. Empty list
    /// = "no capabilities". Translated to SIDs via
    /// `DeriveCapabilitySidsFromName` at init time.
    #[serde(default)]
    pub app_container_capabilities: Vec<String>,

    /// SP-6: absolute path to the session workspace dir. SP-6 adds an
    /// Allow-Modify ACE for the per-execution AppContainer SID on this
    /// directory before spawn so the target can read/write its
    /// workspace. `None` → no DACL grant (target may fail on writes).
    #[serde(default)]
    pub workspace_path: Option<String>,

    /// Cycle 7: git-style globs (e.g. `**/.env`, `**/*.pem`, `**/.ssh`)
    /// identifying secret paths the sandboxed target must NOT be able to
    /// read, even though they live inside the otherwise-readable
    /// workspace. The init resolves each glob against `workspace_path`
    /// and stamps a `DENY_ACCESS` read ACE for the per-execution
    /// AppContainer SID on every match — the Windows analogue of the
    /// macOS seatbelt `deny_read_globs` floor (and codex's
    /// `deny_read_acl`). Empty list → no deny-read pass → byte-identical
    /// to the pre-Cycle-7 behaviour.
    ///
    /// Enforced only on the AppContainer path (the default): the
    /// restricted-token path shares the host user SID, so a per-SID deny
    /// would also lock out the parent. With `use_app_container = true`
    /// (the default) the common path is covered.
    #[serde(default)]
    pub deny_read_globs: Vec<String>,
}

/// Translate `NetworkPolicy` → AppContainer capability names. Lives at
/// crate top so it's testable cross-platform.
pub fn capability_names_for_network(
    net: &crate::sandbox::capabilities::NetworkPolicy,
) -> Vec<String> {
    use crate::sandbox::capabilities::NetworkPolicy;
    match net {
        NetworkPolicy::None => Vec::new(),
        NetworkPolicy::AllowAll => vec![
            "internetClient".to_string(),
            "privateNetworkClientServer".to_string(),
        ],
        // AllowHosts is rejected at WindowsSandboxDriver::profile_for time
        // (cycle 1 / SP-3b). If we ever get here we're conservative.
        NetworkPolicy::AllowHosts { .. } => Vec::new(),
    }
}

/// SP-6 v2: DACL inheritance flags applied to AppContainer workspace
/// grants. `CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE` so the ACE
/// propagates to existing children whose default DACL inheritance is
/// enabled (the NTFS default) plus all future children.
///
/// MSDN documents `OBJECT_INHERIT_ACE = 0x1`,
/// `CONTAINER_INHERIT_ACE = 0x2`. Hard-coded so this constant — and
/// the regression test for it — work on the macOS / Linux dev boxes
/// without dragging in Win32 headers.
// Intentionally kept non-cfg-gated so the regression test below
// compiles on all platforms.  The Win32 consumer is Windows-only, so
// this triggers dead_code on non-Windows dev boxes.
#[allow(dead_code)]
pub(crate) const DACL_INHERIT_FLAGS_FOR_APPCONTAINER: u32 = 0x2 | 0x1;

/// Cycle 5: one protected-metadata subpath under a workspace root,
/// tagged with whether it was absent on disk at classification time.
#[allow(dead_code)]
pub(crate) struct MetadataTarget {
    /// Absolute path of the protected subpath (`<ws>/.git`, …).
    pub path: std::path::PathBuf,
    /// `true` when the path did not exist. The Windows ACE stamper
    /// pre-creates an empty stub directory for every absent path before
    /// applying its deny ACE — otherwise the sandboxed process could
    /// `mkdir` the directory itself and inherit the workspace root's
    /// `GENERIC_ALL` grant.
    pub absent: bool,
}

/// Cycle 3 + Cycle 5: resolve the four protected-metadata subpaths
/// under `workspace_root`, each tagged with on-disk existence. Cross-
/// platform on purpose so the partition logic unit-tests on macOS /
/// Linux dev boxes; only the Windows ACE/stub stamper consumes it.
///
/// Cycle 3 only protected children that already existed, because
/// `SetNamedSecurityInfoW` fails with `ERROR_FILE_NOT_FOUND` on a
/// missing target. Cycle 5 keeps the absent ones too so the Windows
/// stamper can pre-create an empty stub directory for each — closing
/// the gap where a sandboxed process `mkdir`s `.git` and inherits the
/// workspace root's inherited `GENERIC_ALL`.
#[allow(dead_code)]
pub(crate) fn classify_protected_metadata(workspace_root: &std::path::Path) -> Vec<MetadataTarget> {
    crate::sandbox::protected_paths::PROTECTED_METADATA_SUBPATHS
        .iter()
        .map(|sub| {
            let path = workspace_root.join(sub);
            let absent = !path.exists();
            MetadataTarget { path, absent }
        })
        .collect()
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

    // SP-6: try AppContainer first when enabled. Soft-degrade to SP-3a
    // restricted-token path on any AppContainer setup failure (unless
    // require_app_container=true escalates it).
    if parsed.policy.use_app_container {
        match imp::launch_with_app_container(&parsed) {
            Ok(code) => std::process::exit(code),
            Err(imp::LaunchError::AppContainerSetupFailed(msg))
                if !parsed.policy.require_app_container =>
            {
                eprintln!(
                    "aleph sandbox-init-windows: AppContainer setup failed ({msg}); \
                     falling back to restricted-token path"
                );
                // fall through to restricted-token branch below
            }
            Err(imp::LaunchError::AppContainerSetupFailed(msg)) => {
                eprintln!(
                    "aleph sandbox-init-windows: AppContainer setup failed ({msg}) \
                     and require_app_container=true"
                );
                std::process::exit(64);
            }
            Err(imp::LaunchError::WaitFailed(msg)) => {
                eprintln!("aleph sandbox-init-windows: AppContainer wait failed: {msg}");
                std::process::exit(65);
            }
            Err(other) => {
                eprintln!("aleph sandbox-init-windows: unexpected AppContainer error: {other:?}");
                std::process::exit(65);
            }
        }
    }

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
        // Unreachable here: AppContainerSetupFailed is only produced by
        // launch_with_app_container and is fully handled above. Pattern
        // is included to keep the match exhaustive against future
        // changes to LaunchError.
        Err(imp::LaunchError::AppContainerSetupFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: unreachable AppContainer error: {msg}");
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
    let policy: WindowsInitPolicy =
        serde_json::from_str(policy_str).map_err(|e| format!("--policy JSON parse error: {e}"))?;

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
        CreateProcessAsUserW, CreateProcessW, GetCurrentProcess, GetExitCodeProcess,
        OpenProcessToken, WaitForSingleObject, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
        STARTUPINFOW,
    };

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
                unsafe { LocalFree(self.0) };
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

    /// SP-6: launch target inside a per-execution AppContainer profile.
    /// Capability SIDs are derived from the policy's
    /// `app_container_capabilities` name list. The AppContainer profile
    /// is deleted after the target exits.
    pub(super) fn launch_with_app_container(parsed: &ParsedInitArgs) -> Result<i32, LaunchError> {
        use std::iter::once;

        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::DeriveCapabilitySidsFromName;
        use windows_sys::Win32::Security::Isolation::{
            CreateAppContainerProfile, DeleteAppContainerProfile,
        };
        use windows_sys::Win32::Security::{FreeSid, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES};
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapAlloc, HeapFree};
        use windows_sys::Win32::System::SystemServices::SE_GROUP_ENABLED;
        use windows_sys::Win32::System::Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
            InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
            EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES,
            STARTUPINFOEXW,
        };

        // ---------- 1. Generate unique per-execution profile name ----------
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let profile_name = format!("aleph-sandbox-{pid}-{nonce}");
        let profile_name_w: Vec<u16> = profile_name.encode_utf16().chain(once(0)).collect();
        let display_name_w: Vec<u16> = "Aleph Sandbox".encode_utf16().chain(once(0)).collect();
        let description_w: Vec<u16> = "Per-execution AppContainer for aleph-server sandbox"
            .encode_utf16()
            .chain(once(0))
            .collect();

        // ---------- 2. Derive capability SIDs ----------
        let mut cap_sids: Vec<*mut core::ffi::c_void> = Vec::new();
        let mut group_sid_ptrs: Vec<*mut core::ffi::c_void> = Vec::new();
        for name in &parsed.policy.app_container_capabilities {
            let name_w: Vec<u16> = name.encode_utf16().chain(once(0)).collect();
            let mut sid: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut group_sids: *mut *mut core::ffi::c_void = std::ptr::null_mut();
            let mut group_count: u32 = 0;
            let mut sid_count: u32 = 0;
            let ok = unsafe {
                DeriveCapabilitySidsFromName(
                    name_w.as_ptr(),
                    &mut group_sids,
                    &mut group_count,
                    &mut sid as *mut _ as *mut _,
                    &mut sid_count,
                )
            };
            if ok == 0 || sid_count == 0 {
                // unknown capability name → skip silently
                continue;
            }
            cap_sids.push(sid);
            // Track group SIDs for cleanup even though we don't use them.
            if !group_sids.is_null() {
                group_sid_ptrs.push(group_sids as *mut core::ffi::c_void);
            }
        }

        let cap_attrs: Vec<SID_AND_ATTRIBUTES> = cap_sids
            .iter()
            .map(|s| SID_AND_ATTRIBUTES {
                Sid: *s,
                Attributes: SE_GROUP_ENABLED as u32,
            })
            .collect();

        // ---------- 3. Create the AppContainer profile ----------
        let mut ac_sid: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = unsafe {
            CreateAppContainerProfile(
                profile_name_w.as_ptr(),
                display_name_w.as_ptr(),
                description_w.as_ptr(),
                if cap_attrs.is_empty() {
                    std::ptr::null()
                } else {
                    cap_attrs.as_ptr()
                },
                cap_attrs.len() as u32,
                &mut ac_sid as *mut _ as *mut _,
            )
        };
        if hr != 0 {
            // Cleanup capability SIDs.
            for sid in &cap_sids {
                unsafe { LocalFree(*sid) };
            }
            for g in &group_sid_ptrs {
                unsafe { LocalFree(*g) };
            }
            return Err(LaunchError::AppContainerSetupFailed(format!(
                "CreateAppContainerProfile failed: hr={hr:#010x}"
            )));
        }

        // ---------- 3.5. SP-6 v2: grant workspace DACL ----------
        // Best-effort: if the grant fails, the target may fail on
        // workspace writes but the sandbox itself stays up. We never
        // hard-fail this step (per spec § 3, even when
        // require_app_container=true).
        //
        // Cycle 3 + Cycle 5: after the workspace grant, protect every
        // metadata subpath (`<ws>/.git`, `<ws>/.aleph`, …). The grant
        // gives the AppContainer SID `GENERIC_ALL` on the workspace
        // root, which inherits down to children; a `DENY_ACCESS` ACE on
        // each protected child pins it read-only (deny is evaluated
        // before allow in canonical ACL order). Cycle 3 only protected
        // children that already existed — Cycle 5 also pre-creates an
        // empty stub directory for each absent one, so the sandboxed
        // process cannot `mkdir` it and inherit `GENERIC_ALL`.
        //
        // `metadata_protection` records which deny ACEs were applied
        // and which stub directories were created, so the post-wait
        // cleanup revokes and removes the exact same sets.
        let mut metadata_protection = MetadataProtection::default();
        if let Some(ref ws) = parsed.policy.workspace_path {
            use windows_sys::Win32::Foundation::GENERIC_ALL;
            if let Err(e) = unsafe {
                set_workspace_dacl_entry(
                    ws,
                    ac_sid,
                    windows_sys::Win32::Security::Authorization::GRANT_ACCESS,
                    GENERIC_ALL,
                )
            } {
                eprintln!(
                    "aleph sandbox-init-windows: workspace DACL grant failed ({e}); \
                     target may fail on workspace writes"
                );
            }
            metadata_protection = ensure_protected_metadata_deny(ws, ac_sid);

            // Cycle 7: deny-read floor for configured secret globs. Each
            // resolved path gets a DENY read ACE for the AppContainer SID;
            // the paths fold into `metadata_protection.denied` so the
            // post-wait REVOKE_ACCESS loop (which ignores the mask) cleans
            // them up alongside the metadata deny-write ACEs.
            let deny_read = ensure_deny_read_globs(ws, ac_sid, &parsed.policy.deny_read_globs);
            metadata_protection.denied.extend(deny_read);
        }

        // ---------- 4. Build SECURITY_CAPABILITIES + attribute list ----------
        let mut attr_size: usize = 0;
        unsafe {
            // First call with NULL probes for required size; sets last
            // error to ERROR_INSUFFICIENT_BUFFER which we ignore.
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
        }
        let attr_buffer = unsafe { HeapAlloc(GetProcessHeap(), 0, attr_size) };
        if attr_buffer.is_null() {
            cleanup_sids(&cap_sids, &group_sid_ptrs, ac_sid);
            unsafe { DeleteAppContainerProfile(profile_name_w.as_ptr()) };
            return Err(LaunchError::AppContainerSetupFailed(
                "HeapAlloc for PROC_THREAD_ATTRIBUTE_LIST returned NULL".into(),
            ));
        }
        let attr_list = attr_buffer as LPPROC_THREAD_ATTRIBUTE_LIST;

        let ok = unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) };
        if ok == 0 {
            unsafe { HeapFree(GetProcessHeap(), 0, attr_buffer) };
            cleanup_sids(&cap_sids, &group_sid_ptrs, ac_sid);
            unsafe { DeleteAppContainerProfile(profile_name_w.as_ptr()) };
            return Err(LaunchError::AppContainerSetupFailed(
                "InitializeProcThreadAttributeList failed".into(),
            ));
        }

        let sec_caps = SECURITY_CAPABILITIES {
            AppContainerSid: ac_sid,
            Capabilities: if cap_attrs.is_empty() {
                std::ptr::null_mut()
            } else {
                cap_attrs.as_ptr() as *mut SID_AND_ATTRIBUTES
            },
            CapabilityCount: cap_attrs.len() as u32,
            Reserved: 0,
        };

        let ok = unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &sec_caps as *const _ as *const _,
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            unsafe {
                DeleteProcThreadAttributeList(attr_list);
                HeapFree(GetProcessHeap(), 0, attr_buffer);
                DeleteAppContainerProfile(profile_name_w.as_ptr());
            }
            cleanup_sids(&cap_sids, &group_sid_ptrs, ac_sid);
            return Err(LaunchError::AppContainerSetupFailed(
                "UpdateProcThreadAttribute(SECURITY_CAPABILITIES) failed".into(),
            ));
        }

        // ---------- 5. Build STARTUPINFOEXW + cmd line, CreateProcessW ----------
        let cmd_line_str = build_command_line(&parsed.target, &parsed.target_args);
        let mut cmd_line: Vec<u16> = cmd_line_str.encode_utf16().chain(once(0)).collect();

        let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        si.StartupInfo.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        si.StartupInfo.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        si.lpAttributeList = attr_list;

        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

        let spawn_ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmd_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // bInheritHandles = TRUE
                EXTENDED_STARTUPINFO_PRESENT,
                std::ptr::null_mut(),
                std::ptr::null(),
                &si.StartupInfo,
                &mut pi,
            )
        };

        if spawn_ok == 0 {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            unsafe {
                DeleteProcThreadAttributeList(attr_list);
                HeapFree(GetProcessHeap(), 0, attr_buffer);
                DeleteAppContainerProfile(profile_name_w.as_ptr());
            }
            cleanup_sids(&cap_sids, &group_sid_ptrs, ac_sid);
            return Err(LaunchError::AppContainerSetupFailed(format!(
                "CreateProcessW(AppContainer) failed: {err:#010x}"
            )));
        }

        // ---------- 6. Wait + GetExitCode ----------
        let wait_result = unsafe { WaitForSingleObject(pi.hProcess, INFINITE) };
        let wait_err = if wait_result != 0 {
            Some(wait_result)
        } else {
            None
        };

        let mut code: u32 = 0;
        let code_ok = unsafe { GetExitCodeProcess(pi.hProcess, &mut code) };

        // ---------- 6.5. SP-6 v2 + Cycle 3 + Cycle 5: undo DACL + stubs ----------
        // Best-effort. The SID is about to be invalidated by
        // DeleteAppContainerProfile, which makes any leftover ACE dead
        // weight (spec § 7 risk register), so revoke failure is logged
        // but ignored. REVOKE_ACCESS clears every ACE for the trustee
        // regardless of mask, so a single call per path covers both the
        // workspace grant and the metadata deny ACE. Cycle 5 also
        // removes the empty stub directories created before spawn so the
        // workspace is left exactly as we found it.
        if let Some(ref ws) = parsed.policy.workspace_path {
            use windows_sys::Win32::Foundation::GENERIC_ALL;
            if let Err(e) = unsafe {
                set_workspace_dacl_entry(
                    ws,
                    ac_sid,
                    windows_sys::Win32::Security::Authorization::REVOKE_ACCESS,
                    GENERIC_ALL,
                )
            } {
                eprintln!(
                    "aleph sandbox-init-windows: workspace DACL revoke failed ({e}); \
                     AppContainer SID is about to be invalidated, ACE will become dead weight"
                );
            }
            // Revoke each deny ACE we actually stamped earlier.
            for p in &metadata_protection.denied {
                if let Err(e) = unsafe {
                    set_workspace_dacl_entry(
                        p,
                        ac_sid,
                        windows_sys::Win32::Security::Authorization::REVOKE_ACCESS,
                        GENERIC_ALL,
                    )
                } {
                    eprintln!(
                        "aleph sandbox-init-windows: metadata DACL revoke failed on {p} ({e}); \
                         AppContainer SID is about to be invalidated, ACE will become dead weight"
                    );
                }
            }
            // Remove the empty stub directories we created before spawn.
            // `remove_dir` (not `remove_dir_all`) only succeeds on an
            // empty directory — if a deny ACE failed and the target
            // populated a stub, we leave it rather than destroy data.
            for p in &metadata_protection.created_stubs {
                if let Err(e) = std::fs::remove_dir(p) {
                    eprintln!(
                        "aleph sandbox-init-windows: could not remove protected metadata \
                         stub {p} ({e}); leaving it in place"
                    );
                }
            }
        }

        // ---------- 7. Cleanup (always runs) ----------
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(pi.hThread);
            windows_sys::Win32::Foundation::CloseHandle(pi.hProcess);
            DeleteProcThreadAttributeList(attr_list);
            HeapFree(GetProcessHeap(), 0, attr_buffer);
            DeleteAppContainerProfile(profile_name_w.as_ptr());
        }
        cleanup_sids(&cap_sids, &group_sid_ptrs, ac_sid);

        if let Some(w) = wait_err {
            return Err(LaunchError::WaitFailed(format!(
                "WaitForSingleObject returned {w:#010x}"
            )));
        }
        if code_ok == 0 {
            return Err(LaunchError::WaitFailed("GetExitCodeProcess failed".into()));
        }
        Ok(code as i32)
    }

    fn cleanup_sids(
        cap_sids: &[*mut core::ffi::c_void],
        group_sid_ptrs: &[*mut core::ffi::c_void],
        ac_sid: *mut core::ffi::c_void,
    ) {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::FreeSid;
        for sid in cap_sids {
            unsafe { LocalFree(*sid) };
        }
        for g in group_sid_ptrs {
            unsafe { LocalFree(*g) };
        }
        if !ac_sid.is_null() {
            unsafe { FreeSid(ac_sid) };
        }
    }

    /// Cycle 3 + Cycle 5: result of [`ensure_protected_metadata_deny`].
    /// Tells the post-wait cleanup which ACEs to revoke and which stub
    /// directories to remove.
    #[derive(Default)]
    pub(super) struct MetadataProtection {
        /// Paths carrying a deny ACE we must revoke after the target
        /// exits.
        pub denied: Vec<String>,
        /// Empty stub directories created before spawn; remove them
        /// after the target exits (best-effort, empty-only — never
        /// destroys agent data).
        pub created_stubs: Vec<String>,
    }

    /// Cycle 3 + Cycle 5: protect every
    /// `<ws>/{.git,.aleph,.codex,.agents}` metadata subpath against the
    /// per-execution AppContainer SID.
    ///
    /// For each of the four subpaths:
    /// - **existing** → stamp a `DENY_ACCESS` ACE (Cycle 3 behavior);
    /// - **absent** → create an empty stub directory first, then stamp
    ///   the ACE. Without the stub the sandboxed process could `mkdir`
    ///   the directory itself; the new directory would inherit the
    ///   workspace root's `GENERIC_ALL` grant and defeat the
    ///   protection. This is the Windows analogue of the Linux
    ///   synthetic-tmpfs fix (Cycle 5 Item 1).
    ///
    /// The mask is `GENERIC_WRITE | DELETE` — the AppContainer SID can
    /// still *read* `.git` (so the agent can run `git log` / `git
    /// status`) but cannot mutate the metadata in any way. `DELETE` is
    /// not part of `GENERIC_WRITE`, so we OR it in explicitly to block
    /// `rm -rf .git`-style attacks.
    ///
    /// Best-effort throughout: a failed stub creation or ACE stamp is
    /// logged and skipped; the affected path stays writable but the
    /// rest of the sandbox is unaffected.
    pub(super) fn ensure_protected_metadata_deny(
        workspace_path_str: &str,
        ac_sid: *mut core::ffi::c_void,
    ) -> MetadataProtection {
        // GENERIC_WRITE is in Foundation; DELETE is a standard right
        // (0x0001_0000) — hard-coding it avoids reaching for the
        // `windows_sys::Win32::Storage::FileSystem` re-export.
        const DELETE_RIGHT: u32 = 0x0001_0000;
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Security::Authorization::DENY_ACCESS;

        let mask = GENERIC_WRITE | DELETE_RIGHT;
        let root = std::path::Path::new(workspace_path_str);
        let mut out = MetadataProtection::default();

        for target in crate::sandbox::windows_init::classify_protected_metadata(root) {
            let Some(s) = target.path.to_str() else {
                eprintln!(
                    "aleph sandbox-init-windows: skipping protected metadata path with \
                     non-UTF-8 chars under {workspace_path_str}"
                );
                continue;
            };
            // Absent path → create an empty stub directory so the deny
            // ACE has a real object to bind to, and record it for
            // post-wait removal.
            if target.absent {
                if let Err(e) = std::fs::create_dir(&target.path) {
                    eprintln!(
                        "aleph sandbox-init-windows: could not create protected metadata \
                         stub {s} ({e}); path stays unprotected"
                    );
                    continue;
                }
                out.created_stubs.push(s.to_string());
            }
            match unsafe { set_workspace_dacl_entry(s, ac_sid, DENY_ACCESS, mask) } {
                Ok(()) => out.denied.push(s.to_string()),
                Err(e) => eprintln!(
                    "aleph sandbox-init-windows: deny ACE on protected metadata {s} \
                     failed ({e}); target may be able to modify it"
                ),
            }
        }
        out
    }

    /// Cycle 7: stamp a `DENY_ACCESS` read ACE for the per-execution
    /// AppContainer SID on every secret path under `workspace_path_str`
    /// that matches one of `deny_read_globs`. The Windows analogue of the
    /// macOS seatbelt deny-read floor: the workspace-root `GENERIC_ALL`
    /// grant inherits down to children, so without an explicit deny the
    /// sandboxed target could read `.env` / `*.pem` / `.ssh/**` sitting
    /// inside the workspace. The mask is `GENERIC_READ`; canonical ACL
    /// ordering (deny-before-allow) is handled by `SetEntriesInAclW`.
    ///
    /// Returns the paths that received a deny ACE so the caller can revoke
    /// them after the target exits (REVOKE_ACCESS ignores the mask, so the
    /// same cleanup path used for metadata deny-write ACEs applies).
    ///
    /// Best-effort throughout: a failed match-resolution or ACE stamp is
    /// logged and skipped; the affected path stays readable but the rest of
    /// the sandbox is unaffected. Empty globs → no walk, empty result.
    pub(super) fn ensure_deny_read_globs(
        workspace_path_str: &str,
        ac_sid: *mut core::ffi::c_void,
        deny_read_globs: &[String],
    ) -> Vec<String> {
        use windows_sys::Win32::Foundation::GENERIC_READ;
        use windows_sys::Win32::Security::Authorization::DENY_ACCESS;

        if deny_read_globs.is_empty() {
            return Vec::new();
        }

        let root = std::path::Path::new(workspace_path_str);
        let mut denied = Vec::new();
        for path in crate::sandbox::deny_globs::resolve_deny_read_paths_under(root, deny_read_globs)
        {
            let Some(s) = path.to_str() else {
                eprintln!(
                    "aleph sandbox-init-windows: skipping deny-read path with non-UTF-8 \
                     chars under {workspace_path_str}"
                );
                continue;
            };
            match unsafe { set_workspace_dacl_entry(s, ac_sid, DENY_ACCESS, GENERIC_READ) } {
                Ok(()) => denied.push(s.to_string()),
                Err(e) => eprintln!(
                    "aleph sandbox-init-windows: deny-read ACE on secret {s} failed ({e}); \
                     target may be able to read it"
                ),
            }
        }
        denied
    }

    /// SP-6 v2 / Cycle 3: Add or remove an inheritable ACE for `ac_sid`
    /// on `target_path` with the supplied access mask. Same code path
    /// for grant (`mode = GRANT_ACCESS`), deny (`mode = DENY_ACCESS`),
    /// and revoke (`mode = REVOKE_ACCESS`) — the only differences are
    /// `mode` and `permission_mask` inside `EXPLICIT_ACCESS_W`.
    ///
    /// Cycle 2 wired this only for the workspace root with `GENERIC_ALL`
    /// + `GRANT_ACCESS`/`REVOKE_ACCESS`. Cycle 3 generalises so the
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
        ea.grfInheritance = crate::sandbox::windows_init::DACL_INHERIT_FLAGS_FOR_APPCONTAINER;
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

        use windows_sys::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
        let label = TOKEN_MANDATORY_LABEL {
            Label: windows_sys::Win32::Security::SID_AND_ATTRIBUTES {
                Sid: sid as *mut _,
                Attributes: SE_GROUP_INTEGRITY as u32,
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

    fn spawn_and_wait(parsed: &ParsedInitArgs, token: Option<HANDLE>) -> Result<i32, LaunchError> {
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
            use_app_container: true,
            require_app_container: false,
            app_container_capabilities: vec!["internetClient".to_string()],
            workspace_path: Some("C:\\workspace\\session-abc".to_string()),
            deny_read_globs: vec!["**/.env".to_string(), "**/*.pem".to_string()],
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: WindowsInitPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn policy_default_disables_all_strict_flags() {
        let p = WindowsInitPolicy::default();
        assert!(!p.require_restricted_token);
        assert!(!p.use_app_container);
        assert!(!p.require_app_container);
        assert!(p.app_container_capabilities.is_empty());
        assert!(p.workspace_path.is_none());
        assert!(p.deny_read_globs.is_empty());
    }

    #[test]
    fn policy_accepts_missing_deny_read_globs_via_serde_default() {
        // Forward-compat: an older driver that omits the field still
        // deserializes — deny_read_globs defaults to empty (no floor).
        let parsed: WindowsInitPolicy =
            serde_json::from_str(r#"{"workspace_path":"C:\\ws"}"#).unwrap();
        assert!(parsed.deny_read_globs.is_empty());
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
            ..Default::default()
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
        let argv = vec!["--policy".to_string(), "{}".to_string(), "--".to_string()];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("missing target"), "got: {err}");
    }

    #[test]
    fn capability_names_for_allow_all() {
        let names =
            capability_names_for_network(&crate::sandbox::capabilities::NetworkPolicy::AllowAll);
        assert_eq!(
            names,
            vec![
                "internetClient".to_string(),
                "privateNetworkClientServer".to_string(),
            ]
        );
    }

    #[test]
    fn capability_names_for_none_returns_empty() {
        let names =
            capability_names_for_network(&crate::sandbox::capabilities::NetworkPolicy::None);
        assert!(names.is_empty());
    }

    #[test]
    fn capability_names_for_allow_hosts_returns_empty() {
        // AllowHosts is rejected at profile_for; if we somehow reach here
        // we're conservative and grant nothing.
        let names = capability_names_for_network(
            &crate::sandbox::capabilities::NetworkPolicy::AllowHosts {
                hosts: vec!["github.com".to_string()],
            },
        );
        assert!(names.is_empty());
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

    #[test]
    fn dacl_inherit_flags_matches_msdn_documented_bits() {
        // OBJECT_INHERIT_ACE = 0x1, CONTAINER_INHERIT_ACE = 0x2 per
        // Microsoft Windows SDK winnt.h. If this fires, the constant
        // drifted and SP-6 v2 workspace DACL grant is no longer
        // inheritable, which means AppContainer targets cannot read
        // or write subdirectories of their workspace.
        assert_eq!(DACL_INHERIT_FLAGS_FOR_APPCONTAINER, 0x3);
    }

    #[test]
    fn classify_marks_existing_and_absent_metadata() {
        // The Windows stamper stamps a deny ACE on every entry and
        // pre-creates a stub for each `absent` one. If the `absent`
        // flag ever drifts, an absent `.git` would silently lose
        // protection. File-system semantics are identical on macOS /
        // Linux dev boxes, so the test runs everywhere.
        let tmp = tempfile::tempdir().expect("tempdir");
        let ws = tmp.path();
        // Create two of the four protected subpaths.
        std::fs::create_dir(ws.join(".git")).unwrap();
        std::fs::create_dir(ws.join(".aleph")).unwrap();
        // .codex and .agents intentionally absent.

        let targets = classify_protected_metadata(ws);
        assert_eq!(
            targets.len(),
            crate::sandbox::protected_paths::PROTECTED_METADATA_SUBPATHS.len(),
            "one entry per protected subpath"
        );
        for t in &targets {
            let name = t.path.file_name().unwrap().to_str().unwrap();
            let expect_absent = name == ".codex" || name == ".agents";
            assert_eq!(t.absent, expect_absent, "wrong absent flag for {name}");
        }
    }

    #[test]
    fn classify_marks_all_absent_when_workspace_missing() {
        // Non-existent workspace root → every subpath is absent. The
        // Windows stamper's `create_dir` then fails (missing parent)
        // and logs — no panic. Confirms classification never walks a
        // missing directory.
        let bogus = std::path::PathBuf::from("/this/does/not/exist/aleph/test/abcdef");
        let targets = classify_protected_metadata(&bogus);
        assert_eq!(
            targets.len(),
            crate::sandbox::protected_paths::PROTECTED_METADATA_SUBPATHS.len()
        );
        assert!(
            targets.iter().all(|t| t.absent),
            "every entry should be absent for a missing workspace"
        );
    }

    #[test]
    fn classify_treats_file_named_dot_git_as_existing() {
        // `.exists()` is true for files too, so a stray *file* named
        // `.git` counts as existing — the stamper DACLs the file in
        // place rather than trying to create a stub directory over it.
        // Pin the behavior so a future "directory-only" refactor is
        // intentional.
        let tmp = tempfile::tempdir().expect("tempdir");
        let ws = tmp.path();
        std::fs::write(ws.join(".git"), b"weird but legal\n").unwrap();
        let targets = classify_protected_metadata(ws);
        let git = targets
            .iter()
            .find(|t| t.path == ws.join(".git"))
            .expect(".git entry present");
        assert!(!git.absent, "a file named .git counts as existing");
    }
}
