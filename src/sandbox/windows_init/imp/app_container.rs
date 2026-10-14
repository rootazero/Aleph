//! SP-6 AppContainer launch path plus the Cycle 3/5/7 workspace
//! protection passes (metadata deny-write ACEs, absent-stub pre-creation,
//! and secret deny-read ACEs).
//!
//! Soft-degrades to the SP-3a restricted-token path in [`super`] on any
//! AppContainer setup failure (unless `require_app_container = true`).
//!
//! The per-execution profile and the `PROC_THREAD_ATTRIBUTE_LIST` are RAII
//! types ([`AppContainerProfile`], [`ProcAttributeList`]) so the same SID /
//! capability / workspace-ACE logic serves both this launch-and-wait init
//! path and the parent-side managed launcher
//! (`platforms::windows::launcher`), which creates the service suspended.

use std::iter::once;

use windows_sys::Win32::Security::{SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES};
use windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST;

use super::{build_command_line, set_workspace_dacl_entry, LaunchError};
use crate::sandbox::windows_init::args::ParsedInitArgs;

type RawSid = *mut core::ffi::c_void;

const INIT_LOG: &str = "aleph sandbox-init-windows";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(once(0)).collect()
}

/// Workspace ACEs stamped for the profile SID; revoked on profile drop.
struct WorkspaceGrant {
    path: String,
    protection: MetadataProtection,
}

/// A per-execution AppContainer profile: derived capability SIDs, the
/// container SID, and (optionally) the workspace grant + metadata/secret
/// deny ACEs. Drop revokes the ACEs, deletes the profile and frees SIDs,
/// in that order. The SID pointers stay valid for the lifetime of `self`.
pub(crate) struct AppContainerProfile {
    name_w: Vec<u16>,
    sid: RawSid,
    cap_sids: Vec<RawSid>,
    group_sid_ptrs: Vec<RawSid>,
    cap_attrs: Vec<SID_AND_ATTRIBUTES>,
    workspace: Option<WorkspaceGrant>,
    log_context: &'static str,
}

// SAFETY: every raw pointer is a SID allocated for and owned exclusively by
// this value; none is shared with another owner, and the OS calls made
// through them are not thread-affine.
unsafe impl Send for AppContainerProfile {}

impl AppContainerProfile {
    /// Create `aleph-sandbox-{pid}-{nonce}` with the named capabilities.
    /// Unknown capability names are skipped (same as the init path always did).
    pub(crate) fn create(
        capability_names: &[String],
        log_context: &'static str,
    ) -> Result<Self, String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::DeriveCapabilitySidsFromName;
        use windows_sys::Win32::Security::Isolation::CreateAppContainerProfile;
        use windows_sys::Win32::System::SystemServices::SE_GROUP_ENABLED;

        // ---------- 1. Generate unique per-execution profile name ----------
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let profile_name = format!("aleph-sandbox-{pid}-{nonce}");
        let name_w = wide(&profile_name);
        let display_name_w = wide("Aleph Sandbox");
        let description_w = wide("Per-execution AppContainer for aleph-server sandbox");

        // ---------- 2. Derive capability SIDs ----------
        let mut cap_sids: Vec<RawSid> = Vec::new();
        let mut group_sid_ptrs: Vec<RawSid> = Vec::new();
        for name in capability_names {
            let name_w = wide(name);
            let mut sid: RawSid = std::ptr::null_mut();
            let mut group_sids: *mut RawSid = std::ptr::null_mut();
            let mut group_count: u32 = 0;
            let mut sid_count: u32 = 0;
            // SAFETY: `name_w` is NUL-terminated; output pointers are valid out-params.
            // rust-doctor-disable-next-line unsafe-block-audit
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
                // unknown capability name -> skip silently
                continue;
            }
            cap_sids.push(sid);
            // Track group SIDs for cleanup even though we don't use them.
            if !group_sids.is_null() {
                group_sid_ptrs.push(group_sids as RawSid);
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
        let mut ac_sid: RawSid = std::ptr::null_mut();
        // SAFETY: All input strings are NUL-terminated; `cap_attrs` is a valid array.
        // rust-doctor-disable-next-line unsafe-block-audit
        let hr = unsafe {
            CreateAppContainerProfile(
                name_w.as_ptr(),
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
            for sid in cap_sids.iter().chain(group_sid_ptrs.iter()) {
                // SAFETY: `*sid` is a non-null pointer returned by `DeriveCapabilitySidsFromName`.
                // rust-doctor-disable-next-line unsafe-block-audit
                unsafe { LocalFree(*sid) };
            }
            return Err(format!("CreateAppContainerProfile failed: hr={hr:#010x}"));
        }
        Ok(Self {
            name_w,
            sid: ac_sid,
            cap_sids,
            group_sid_ptrs,
            cap_attrs,
            workspace: None,
            log_context,
        })
    }

    /// `S-1-15-2-...` string form of the container SID.
    pub(crate) fn sid_string(&self) -> Result<String, String> {
        use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

        let mut out: *mut u16 = std::ptr::null_mut();
        // SAFETY: `self.sid` is a valid SID; `out` is a valid out-pointer.
        // rust-doctor-disable-next-line unsafe-block-audit
        if unsafe { ConvertSidToStringSidW(self.sid, &mut out) } == 0 || out.is_null() {
            // SAFETY: thread-local error read.
            return Err(format!("ConvertSidToStringSidW failed: {}", unsafe {
                GetLastError()
            }));
        }
        let mut len = 0usize;
        // SAFETY: `out` is a NUL-terminated LocalAlloc'd wide string.
        // rust-doctor-disable-next-line unsafe-block-audit
        let text = unsafe {
            while *out.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(out, len))
        };
        // SAFETY: `out` was allocated by ConvertSidToStringSidW with LocalAlloc.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { LocalFree(out as *mut core::ffi::c_void) };
        Ok(text)
    }

    /// `SECURITY_CAPABILITIES` pointing into `self`; only valid while
    /// `self` lives and is not mutated.
    pub(crate) fn security_capabilities(&self) -> SECURITY_CAPABILITIES {
        SECURITY_CAPABILITIES {
            AppContainerSid: self.sid,
            Capabilities: if self.cap_attrs.is_empty() {
                std::ptr::null_mut()
            } else {
                self.cap_attrs.as_ptr() as *mut SID_AND_ATTRIBUTES
            },
            CapabilityCount: self.cap_attrs.len() as u32,
            Reserved: 0,
        }
    }

    /// SP-6 v2 + Cycle 3/5/7: grant the workspace to the container SID,
    /// then pin metadata subpaths read-only and deny-read secret globs.
    ///
    /// Best-effort: if the grant fails, the target may fail on workspace
    /// writes but the sandbox itself stays up (spec section 3, even when
    /// `require_app_container = true`). The applied ACEs and created
    /// stubs are recorded so Drop revokes / removes exactly the same sets.
    /// Calling it twice is a no-op.
    pub(crate) fn grant_workspace(&mut self, ws: &str, deny_read_globs: &[String]) {
        use windows_sys::Win32::Foundation::GENERIC_ALL;
        if self.workspace.is_some() {
            return;
        }
        // SAFETY: `ws` is a canonical path and `self.sid` is a valid AppContainer SID.
        // rust-doctor-disable-next-line unsafe-block-audit
        if let Err(e) = unsafe {
            set_workspace_dacl_entry(
                ws,
                self.sid,
                windows_sys::Win32::Security::Authorization::GRANT_ACCESS,
                GENERIC_ALL,
            )
        } {
            eprintln!(
                "{}: workspace DACL grant failed ({e}); \
                 target may fail on workspace writes",
                self.log_context
            );
        }
        let mut protection = ensure_protected_metadata_deny(ws, self.sid);
        let deny_read = ensure_deny_read_globs(ws, self.sid, deny_read_globs);
        protection.denied.extend(deny_read);
        self.workspace = Some(WorkspaceGrant {
            path: ws.to_owned(),
            protection,
        });
    }

    /// Undo the workspace grant. Best-effort: the SID is about to be
    /// invalidated by `DeleteAppContainerProfile`, so a leftover ACE is dead
    /// weight (spec section 7). REVOKE_ACCESS clears every ACE for the
    /// trustee regardless of mask. Stubs are removed with `remove_dir`
    /// (empty-only), never `remove_dir_all`.
    fn revoke_workspace(&mut self) {
        use windows_sys::Win32::Foundation::GENERIC_ALL;
        use windows_sys::Win32::Security::Authorization::REVOKE_ACCESS;
        let Some(grant) = self.workspace.take() else {
            return;
        };
        let ctx = self.log_context;
        // SAFETY: path is canonical and `self.sid` is still a valid SID.
        // rust-doctor-disable-next-line unsafe-block-audit
        if let Err(e) =
            unsafe { set_workspace_dacl_entry(&grant.path, self.sid, REVOKE_ACCESS, GENERIC_ALL) }
        {
            eprintln!(
                "{ctx}: workspace DACL revoke failed ({e}); \
                 AppContainer SID is about to be invalidated, ACE will become dead weight"
            );
        }
        for p in &grant.protection.denied {
            // SAFETY: `p` is a canonical path and `self.sid` is a valid SID.
            // rust-doctor-disable-next-line unsafe-block-audit
            if let Err(e) =
                unsafe { set_workspace_dacl_entry(p, self.sid, REVOKE_ACCESS, GENERIC_ALL) }
            {
                eprintln!(
                    "{ctx}: metadata DACL revoke failed on {p} ({e}); \
                     AppContainer SID is about to be invalidated, ACE will become dead weight"
                );
            }
        }
        for p in &grant.protection.created_stubs {
            if let Err(e) = std::fs::remove_dir(p) {
                eprintln!(
                    "{ctx}: could not remove protected metadata \
                     stub {p} ({e}); leaving it in place"
                );
            }
        }
    }
}

impl Drop for AppContainerProfile {
    fn drop(&mut self) {
        use windows_sys::Win32::Security::Isolation::DeleteAppContainerProfile;
        self.revoke_workspace();
        // SAFETY: `name_w` is the NUL-terminated name this profile was created with.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { DeleteAppContainerProfile(self.name_w.as_ptr()) };
        cleanup_sids(&self.cap_sids, &self.group_sid_ptrs, self.sid);
    }
}

/// Heap-backed, initialized `PROC_THREAD_ATTRIBUTE_LIST`. Drop deletes and
/// frees it.
pub(crate) struct ProcAttributeList {
    buffer: *mut core::ffi::c_void,
}

// SAFETY: the buffer is owned exclusively by this value.
unsafe impl Send for ProcAttributeList {}

impl ProcAttributeList {
    pub(crate) fn new(count: u32) -> Result<Self, String> {
        use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapAlloc, HeapFree};
        use windows_sys::Win32::System::Threading::InitializeProcThreadAttributeList;

        let mut attr_size: usize = 0;
        // SAFETY: Calling with NULL list is the documented size-probing pattern.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &mut attr_size);
        }
        // SAFETY: `GetProcessHeap()` returns a valid heap; size was probed above.
        // rust-doctor-disable-next-line unsafe-block-audit
        let buffer = unsafe { HeapAlloc(GetProcessHeap(), 0, attr_size) };
        if buffer.is_null() {
            return Err("HeapAlloc for PROC_THREAD_ATTRIBUTE_LIST returned NULL".into());
        }
        // SAFETY: `buffer` and `attr_size` match the probed allocation.
        // rust-doctor-disable-next-line unsafe-block-audit
        let ok = unsafe {
            InitializeProcThreadAttributeList(
                buffer as LPPROC_THREAD_ATTRIBUTE_LIST,
                count,
                0,
                &mut attr_size,
            )
        };
        if ok == 0 {
            // SAFETY: `buffer` is a non-null allocation from the process heap.
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe { HeapFree(GetProcessHeap(), 0, buffer) };
            return Err("InitializeProcThreadAttributeList failed".into());
        }
        Ok(Self { buffer })
    }

    pub(crate) fn as_raw(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer as LPPROC_THREAD_ATTRIBUTE_LIST
    }

    /// # Safety
    /// `value` must point to `size` readable bytes that stay valid and
    /// unmoved until every `CreateProcess*` call using this list returns.
    pub(crate) unsafe fn update(
        &mut self,
        attribute: usize,
        value: *const core::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        use windows_sys::Win32::System::Threading::UpdateProcThreadAttribute;
        // SAFETY: list is initialized; caller guarantees `value`/`size`.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.as_raw(),
                0,
                attribute,
                value,
                size,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(format!("UpdateProcThreadAttribute({attribute:#x}) failed"));
        }
        Ok(())
    }
}

impl Drop for ProcAttributeList {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapFree};
        use windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList;
        // SAFETY: the list was initialized in `new` and is deleted exactly once.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe {
            DeleteProcThreadAttributeList(self.as_raw());
            HeapFree(GetProcessHeap(), 0, self.buffer);
        }
    }
}

/// SP-6: launch target inside a per-execution AppContainer profile.
/// Capability SIDs are derived from the policy's
/// `app_container_capabilities` name list. The AppContainer profile
/// is deleted after the target exits.
pub(in crate::sandbox::windows_init) fn launch_with_app_container(
    parsed: &ParsedInitArgs,
) -> Result<i32, LaunchError> {
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, WaitForSingleObject, EXTENDED_STARTUPINFO_PRESENT,
        INFINITE, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    // ---------- 1-3. Profile + capability SIDs ----------
    let mut profile =
        AppContainerProfile::create(&parsed.policy.app_container_capabilities, INIT_LOG)
            .map_err(LaunchError::AppContainerSetupFailed)?;

    // ---------- 3.5. Workspace grant + metadata / secret deny ACEs ----------
    if let Some(ref ws) = parsed.policy.workspace_path {
        profile.grant_workspace(ws, &parsed.policy.deny_read_globs);
    }

    // ---------- 4. SECURITY_CAPABILITIES + attribute list ----------
    let mut attrs = ProcAttributeList::new(1).map_err(LaunchError::AppContainerSetupFailed)?;
    let sec_caps = profile.security_capabilities();
    // SAFETY: `sec_caps` (and the SIDs it points into, owned by `profile`)
    // outlive the CreateProcessW call below.
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        attrs.update(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            &sec_caps as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<SECURITY_CAPABILITIES>(),
        )
    }
    .map_err(|_| {
        LaunchError::AppContainerSetupFailed(
            "UpdateProcThreadAttribute(SECURITY_CAPABILITIES) failed".into(),
        )
    })?;

    // ---------- 5. Build STARTUPINFOEXW + cmd line, CreateProcessW ----------
    let cmd_line_str = build_command_line(&parsed.target, &parsed.target_args);
    let mut cmd_line: Vec<u16> = wide(&cmd_line_str);

    // SAFETY: `STARTUPINFOEXW` is a plain struct and may be zero-initialized.
    // rust-doctor-disable-next-line unsafe-block-audit
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    // SAFETY: Standard handle constants request inherited console handles.
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        si.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
        si.StartupInfo.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
        si.StartupInfo.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    }
    si.lpAttributeList = attrs.as_raw();

    // SAFETY: `PROCESS_INFORMATION` is a plain struct and may be zero-initialized.
    // rust-doctor-disable-next-line unsafe-block-audit
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // SAFETY: `cmd_line` is a valid mutable NUL-terminated command line; `si` is initialized.
    // rust-doctor-disable-next-line unsafe-block-audit
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
        // SAFETY: `GetLastError()` reads the calling thread's last-error code.
        // rust-doctor-disable-next-line unsafe-block-audit
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        // `attrs` then `profile` drop here (reverse declaration order).
        return Err(LaunchError::AppContainerSetupFailed(format!(
            "CreateProcessW(AppContainer) failed: {err:#010x}"
        )));
    }

    // ---------- 6. Wait + GetExitCode ----------
    // SAFETY: `pi.hProcess` is a valid non-null process handle.
    // rust-doctor-disable-next-line unsafe-block-audit
    let wait_result = unsafe { WaitForSingleObject(pi.hProcess, INFINITE) };
    let wait_err = if wait_result != 0 {
        Some(wait_result)
    } else {
        None
    };

    let mut code: u32 = 0;
    // SAFETY: `pi.hProcess` is a valid non-null process handle.
    // rust-doctor-disable-next-line unsafe-block-audit
    let code_ok = unsafe { GetExitCodeProcess(pi.hProcess, &mut code) };

    // ---------- 7. Cleanup (always runs) ----------
    // Handles first, then the attribute list, then the profile (which
    // revokes the workspace ACEs, removes stubs, deletes the profile and
    // frees the SIDs).
    // SAFETY: both handles are owned and closed exactly once.
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(pi.hThread);
        windows_sys::Win32::Foundation::CloseHandle(pi.hProcess);
    }
    drop(attrs);
    drop(profile);

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

fn cleanup_sids(cap_sids: &[RawSid], group_sid_ptrs: &[RawSid], ac_sid: RawSid) {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::FreeSid;
    for sid in cap_sids {
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { LocalFree(*sid) };
    }
    for g in group_sid_ptrs {
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { LocalFree(*g) };
    }
    if !ac_sid.is_null() {
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { FreeSid(ac_sid) };
    }
}

/// Cycle 3 + Cycle 5: result of [`ensure_protected_metadata_deny`].
/// Tells the post-wait cleanup which ACEs to revoke and which stub
/// directories to remove.
#[derive(Default)]
struct MetadataProtection {
    /// Paths carrying a deny ACE we must revoke after the target
    /// exits.
    denied: Vec<String>,
    /// Empty stub directories created before spawn; remove them
    /// after the target exits (best-effort, empty-only — never
    /// destroys agent data).
    created_stubs: Vec<String>,
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
fn ensure_protected_metadata_deny(
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

    for target in crate::sandbox::windows_init::policy::classify_protected_metadata(root) {
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
        // SAFETY: `s` is a canonical path and `ac_sid` is a valid AppContainer SID.
        // rust-doctor-disable-next-line unsafe-block-audit
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
fn ensure_deny_read_globs(
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
    for path in crate::sandbox::deny_globs::resolve_deny_read_paths_under(root, deny_read_globs) {
        let Some(s) = path.to_str() else {
            eprintln!(
                "aleph sandbox-init-windows: skipping deny-read path with non-UTF-8 \
                 chars under {workspace_path_str}"
            );
            continue;
        };
        // SAFETY: `s` is a canonical path and `ac_sid` is a valid AppContainer SID.
        // rust-doctor-disable-next-line unsafe-block-audit
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
