//! SP-6 AppContainer launch path plus the Cycle 3/5/7 workspace
//! protection passes (metadata deny-write ACEs, absent-stub pre-creation,
//! and secret deny-read ACEs).
//!
//! Soft-degrades to the SP-3a restricted-token path in [`super`] on any
//! AppContainer setup failure (unless `require_app_container = true`).

use super::{build_command_line, set_workspace_dacl_entry, LaunchError};
use crate::sandbox::windows_init::args::ParsedInitArgs;

/// SP-6: launch target inside a per-execution AppContainer profile.
/// Capability SIDs are derived from the policy's
/// `app_container_capabilities` name list. The AppContainer profile
/// is deleted after the target exits.
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub(in crate::sandbox::windows_init) fn launch_with_app_container(
    parsed: &ParsedInitArgs,
) -> Result<i32, LaunchError> {
    use std::iter::once;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::DeriveCapabilitySidsFromName;
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeleteAppContainerProfile,
    };
    use windows_sys::Win32::Security::{SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapAlloc, HeapFree};
    use windows_sys::Win32::System::SystemServices::SE_GROUP_ENABLED;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
        EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
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
    // SAFETY: All input strings are NUL-terminated; `cap_attrs` is a valid array.
    // rust-doctor-disable-next-line unsafe-block-audit
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
            // SAFETY: `*sid` is a non-null SID returned by `DeriveCapabilitySidsFromName`.
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe { LocalFree(*sid) };
        }
        for g in &group_sid_ptrs {
            // SAFETY: `*g` is a non-null SID returned by `DeriveCapabilitySidsFromName`.
            // rust-doctor-disable-next-line unsafe-block-audit
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
        // SAFETY: `ws` is a canonical path and `ac_sid` is a valid AppContainer SID.
        // rust-doctor-disable-next-line unsafe-block-audit
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
    // SAFETY: Calling with NULL list is the documented size-probing pattern.
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
    }
    // SAFETY: `GetProcessHeap()` returns a valid heap; size was probed above.
    // rust-doctor-disable-next-line unsafe-block-audit
    let attr_buffer = unsafe { HeapAlloc(GetProcessHeap(), 0, attr_size) };
    if attr_buffer.is_null() {
        cleanup_sids(&cap_sids, &group_sid_ptrs, ac_sid);
        // SAFETY: `profile_name_w` is a valid NUL-terminated profile name.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { DeleteAppContainerProfile(profile_name_w.as_ptr()) };
        return Err(LaunchError::AppContainerSetupFailed(
            "HeapAlloc for PROC_THREAD_ATTRIBUTE_LIST returned NULL".into(),
        ));
    }
    let attr_list = attr_buffer as LPPROC_THREAD_ATTRIBUTE_LIST;

    // SAFETY: `attr_list` and `attr_size` match the probed allocation.
    // rust-doctor-disable-next-line unsafe-block-audit
    let ok = unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) };
    if ok == 0 {
        // SAFETY: `attr_buffer` is a non-null allocation from the process heap.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { HeapFree(GetProcessHeap(), 0, attr_buffer) };
        cleanup_sids(&cap_sids, &group_sid_ptrs, ac_sid);
        // SAFETY: `profile_name_w` is a valid NUL-terminated profile name.
        // rust-doctor-disable-next-line unsafe-block-audit
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

    // SAFETY: `attr_list` is initialized and `sec_caps` outlives the update.
    // rust-doctor-disable-next-line unsafe-block-audit
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
        // SAFETY: All handles/allocs are valid owned resources from above.
        // rust-doctor-disable-next-line unsafe-block-audit
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

    // SAFETY: `STARTUPINFOEXW` is a plain struct and may be zero-initialized.
    // rust-doctor-disable-next-line unsafe-block-audit
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    // SAFETY: Standard handle constants request inherited console handles.
    // rust-doctor-disable-next-line unsafe-block-audit
    si.StartupInfo.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    // SAFETY: Standard handle constants request inherited console handles.
    // rust-doctor-disable-next-line unsafe-block-audit
    si.StartupInfo.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    // SAFETY: Standard handle constants request inherited console handles.
    // rust-doctor-disable-next-line unsafe-block-audit
    si.StartupInfo.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    si.lpAttributeList = attr_list;

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
        // SAFETY: All handles/allocs are valid owned resources from above.
        // rust-doctor-disable-next-line unsafe-block-audit
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
        // SAFETY: `ws` is a canonical path and `ac_sid` is a valid AppContainer SID.
        // rust-doctor-disable-next-line unsafe-block-audit
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
            // SAFETY: `p` is a canonical path and `ac_sid` is a valid AppContainer SID.
            // rust-doctor-disable-next-line unsafe-block-audit
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
    // rust-doctor-disable-next-line unsafe-block-audit
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
