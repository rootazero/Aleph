# SP-6 v2 — Workspace DACL Grant for AppContainer SID

**Date**: 2026-05-20
**Status**: Design
**Branch**: `feat/sandbox-sp6-workspace-dacl`
**Predecessor**: `2026-05-20-sandbox-sp6-windows-appcontainer-design.md` § 2.4

## 1. Goal & scope

Close the gap left by SP-6 v1: the AppContainer launch path in
`launch_with_app_container` accepts a `workspace_path: Option<String>`
field in `WindowsInitPolicy` and the driver populates it correctly, but
the init function never reads it and never grants the AppContainer SID
access to its workspace. Result: every sandboxed target that needs to
read or write its own workspace currently fails under SP-6's strictest
tier with `ACCESS_DENIED`, silently degrading the useful workload
surface of AppContainer sandboxing.

This spec wires the DACL grant + revoke lifecycle promised by SP-6 v1
§ 2.4, with no public API changes.

**In scope**:
- Inheritable `GENERIC_ALL` allow ACE for the per-execution AppContainer
  SID on the workspace root, applied before `CreateProcessW`.
- Active revoke of the same ACE after `WaitForSingleObject` succeeds.
- Best-effort failure handling at every Win32 call site (per SP-6 v1
  § 5 table — confirmed unchanged).
- One cross-platform unit test for the constants/helpers we add.

**Out of scope** (no change from SP-6 v1):
- Recursive walk of pre-existing workspace children (we rely on
  inheritance flags so children with default inheritance pick up the
  ACE automatically; explicit-DACL children remain explicit by design).
- DACL grant on `%TEMP%`, `~/.gitconfig`, or other system-level paths
  AppContainer targets might need — documented as an accepted
  AppContainer limitation in SP-6 v1 § 7.
- WFP per-host network filtering (deferred indefinitely; SP-6's
  capability-based model is a deliberate alternative).
- Persistent AppContainer profile reuse (still one profile per
  execution, deleted on exit).
- Any change to `require_app_container` semantics — DACL failure stays
  best-effort regardless of the `require_*` flag (rationale: DACL is an
  *enabler*, not a sandbox *enforcement*; failing the whole sandbox
  because an enabler failed would punish computation-only targets that
  don't need workspace writes).

**Success criteria**:
1. On Windows, when AppContainer launch succeeds and `workspace_path`
   is set, the target can read and write inside its workspace via
   ordinary file APIs.
2. When the workspace path is `None`, missing, or DACL grant fails for
   any reason, the launch still proceeds and the target runs (it may
   subsequently fail when it tries to write workspace — that's a
   target-level concern, not a sandbox-level one).
3. After the target exits, the workspace DACL is restored: the ACE we
   added is removed via `SetEntriesInAclW(REVOKE_ACCESS)`. If the
   revoke fails, the ACE remains but is harmless (the SID is invalidated
   when `DeleteAppContainerProfile` runs immediately after).
4. macOS / Linux code paths untouched.
5. All existing sandbox lib tests continue to pass.

## 2. Architecture

### 2.1 Where the change lands

Single file: `src/sandbox/windows_init.rs`. Two new helpers + two new
call sites inside the existing `imp::launch_with_app_container`
function. No driver, config, or platforms/mod.rs changes — the
`workspace_path` field is already wired end-to-end through the policy
JSON.

### 2.2 Lifecycle (additions to existing flow)

Existing steps unchanged; two new bracketing steps inserted:

```
1. Generate per-execution profile name              (existing)
2. Derive capability SIDs                           (existing)
3. CreateAppContainerProfile → ac_sid               (existing)
3.5 [NEW] grant_workspace_dacl(workspace_path, ac_sid)
       Behavior:
         - workspace_path = None              → skip silently
         - any Win32 step fails               → log to stderr + continue
         - success                            → continue
4. Build SECURITY_CAPABILITIES + attribute list     (existing)
5. CreateProcessW (EXTENDED_STARTUPINFO_PRESENT)    (existing)
6. WaitForSingleObject + GetExitCodeProcess         (existing)
6.5 [NEW] revoke_workspace_dacl(workspace_path, ac_sid)
       Behavior:
         - workspace_path = None              → skip silently
         - grant step earlier failed          → still attempt revoke (idempotent — REVOKE_ACCESS removes any matching ACE)
         - revoke step fails                  → log + continue; SID is about to be invalidated, ACE becomes dead weight
7. DeleteProcThreadAttributeList + HeapFree         (existing)
8. DeleteAppContainerProfile + FreeSid              (existing — also performs the implicit SID invalidation that makes 6.5 a hygiene-only step)
```

### 2.3 grant_workspace_dacl pseudocode

```rust
// Returns Ok(()) on success or skip; Err on hard failure (which the caller logs + ignores).
unsafe fn grant_workspace_dacl(workspace_path: &str, ac_sid: PSID) -> Result<(), String> {
    // 1. Convert path → wide.
    let path_w: Vec<u16> = OsStr::new(workspace_path).encode_wide().chain(once(0)).collect();

    // 2. Read existing DACL.
    let mut old_dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    let status = GetNamedSecurityInfoW(
        path_w.as_ptr(),
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        null_mut(), null_mut(),
        &mut old_dacl,
        null_mut(),
        &mut sd,
    );
    if status != ERROR_SUCCESS { return Err(format!("GetNamedSecurityInfoW failed: {status:#010x}")); }
    let _sd_guard = LocalFreeGuard(sd as *mut c_void);

    // 3. Build EXPLICIT_ACCESS_W { GENERIC_ALL, GRANT_ACCESS, INHERIT, ac_sid }.
    let mut ea: EXPLICIT_ACCESS_W = zeroed();
    ea.grfAccessPermissions = GENERIC_ALL;
    ea.grfAccessMode = GRANT_ACCESS;
    ea.grfInheritance = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
    ea.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    ea.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN; // AppContainer SIDs aren't a standard trustee type
    ea.Trustee.ptstrName = ac_sid as *mut u16;

    // 4. Merge into existing DACL.
    let mut new_dacl: *mut ACL = null_mut();
    let status = SetEntriesInAclW(1, &ea, old_dacl, &mut new_dacl);
    if status != ERROR_SUCCESS { return Err(format!("SetEntriesInAclW failed: {status:#010x}")); }
    let _dacl_guard = LocalFreeGuard(new_dacl as *mut c_void);

    // 5. Write back.
    let status = SetNamedSecurityInfoW(
        path_w.as_ptr() as *mut u16,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        null_mut(), null_mut(),
        new_dacl,
        null_mut(),
    );
    if status != ERROR_SUCCESS { return Err(format!("SetNamedSecurityInfoW failed: {status:#010x}")); }
    Ok(())
}
```

### 2.4 revoke_workspace_dacl pseudocode

Mirror of grant, but `grfAccessMode = REVOKE_ACCESS`. SetEntriesInAclW
with REVOKE_ACCESS removes all ACEs matching the trustee, so this is
idempotent (works whether grant succeeded, partially succeeded, or
never ran).

### 2.5 RAII discipline

A small `LocalFreeGuard` wrapper provides RAII for `PSECURITY_DESCRIPTOR`
and `PACL` pointers returned by `GetNamedSecurityInfoW` and
`SetEntriesInAclW`. Without this, any early-return on Win32 failure
between allocation and `LocalFree` leaks memory in this short-lived
init binary — not a correctness issue (the process exits immediately
after `ExitProcess`) but a leak the static analyzer correctly flags.

Implementation: zero-sized struct holding a raw pointer + `Drop` calling
`LocalFree`. Cross-platform-safe because the guard type only exists
inside the `#[cfg(target_os = "windows")] mod imp`.

### 2.6 Inheritance rationale

`CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE` means the ACE propagates
to:
- All existing subdirectories whose own DACLs have inheritance enabled
  (the NTFS default for files/dirs created without explicit protection).
- All future children created after the grant.

Workspace setup happens *before* `sandbox-init-windows` launches, so
files placed there by the sandbox owner exist at grant time but inherit
from the workspace root by default. Setting an inheritable ACE on root
therefore covers them.

Files with explicit-protected DACLs (rare in fresh per-session
workspaces) won't inherit and may still fail — accepted limitation; if
a real workload hits this we'll revisit with a recursive walk.

## 3. Error handling (Best-effort always — per user decision 2026-05-20)

Every Win32 step in grant + revoke is wrapped to log to stderr and
continue. The caller (`launch_with_app_container`) does NOT treat DACL
failure as fatal under any flag combination, including `require_app_container=true`.

Rationale: DACL is an enabler for workload functionality, not a sandbox
enforcement primitive. The AppContainer SID still isolates the target;
the target just can't write workspace. A computation-only target works
fine. A workspace-writing target fails at first write — diagnosable by
the user, recoverable by re-running outside AppContainer (or by fixing
whatever path/permission issue prevented the grant).

This matches the SP-6 v1 § 5 table exactly. No spec contract change.

## 4. File-level changes

| File | Change | Approx LOC |
|---|---|---|
| `src/sandbox/windows_init.rs` | Extend `imp::launch_with_app_container`: two new helper functions (`grant_workspace_dacl`, `revoke_workspace_dacl`), one `LocalFreeGuard` helper, two call sites inside the lifecycle. Plus 2 cross-platform unit tests for any pure-logic helpers. | +130 |
| `docs/reference/SANDBOX.md` | Update "Current Windows defense surface" → SP-6 entry: mention DACL grant is now wired (one paragraph). Update deferred-features table if needed. | +5 |

Net: ~135 LOC; one commit.

## 5. windows-sys feature audit

All required APIs are in features already enabled by SP-6 v1's upgrade
to `windows-sys 0.61`:

| API | Module | Feature |
|---|---|---|
| `GetNamedSecurityInfoW` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `SetNamedSecurityInfoW` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `SetEntriesInAclW` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `EXPLICIT_ACCESS_W` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `TRUSTEE_W` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `SE_FILE_OBJECT` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `GRANT_ACCESS` / `REVOKE_ACCESS` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `TRUSTEE_IS_SID` / `TRUSTEE_IS_UNKNOWN` | `Win32::Security::Authorization` | `Win32_Security_Authorization` ✓ |
| `DACL_SECURITY_INFORMATION` | `Win32::Security` | `Win32_Security` ✓ |
| `CONTAINER_INHERIT_ACE` / `OBJECT_INHERIT_ACE` | `Win32::Security` | `Win32_Security` ✓ |
| `GENERIC_ALL` | `Win32::Storage::FileSystem` (or `Win32::System::SystemServices`) | one of those features already on ✓ |
| `LocalFree` | `Win32::Foundation` (0.61+ location) | `Win32_Foundation` ✓ |
| `ACL` / `PSECURITY_DESCRIPTOR` types | `Win32::Security` | `Win32_Security` ✓ |

If `GENERIC_ALL` resolves elsewhere in 0.61, the implementation will
import the correct path; no new feature flag needed.

**Zero version bumps, zero new features, zero new dependencies.**

## 6. Testing

### 6.1 Cross-platform unit tests (compile + run on macOS dev box)

- `inherit_flags_constant_value` — sanity check the constants we
  compose are the documented values (catches accidental redefinition).
- `revoke_is_idempotent_in_concept` — placeholder test documenting that
  REVOKE_ACCESS removes all matching ACEs; pure documentation, no
  Win32 call.

These don't exercise actual DACL behavior — they're regression nets
for the constants/imports, same posture as SP-3a and SP-6 v1 cross-
platform tests.

### 6.2 Windows CI integration tests (`#[cfg(target_os = "windows")] + #[ignore]`)

- `app_container_target_can_write_workspace` — launch a no-op target
  that touches `<workspace>/test-file`; assert the file exists.
- `app_container_target_cannot_write_outside_workspace` — launch target
  that tries to write `C:\Windows\Temp\foo`; assert `ACCESS_DENIED`.
- `dacl_revoked_after_run` — launch + exit + inspect workspace DACL;
  assert the AppContainer SID's ACE was removed.
- `dacl_grant_failure_does_not_block_launch` — point workspace_path
  at a non-existent path; assert target still spawns + exits cleanly.

### 6.3 What we don't test from macOS dev box

- Actual `GetNamedSecurityInfoW` / `SetNamedSecurityInfoW` /
  `SetEntriesInAclW` behavior.
- ACE inheritance propagation under NTFS.
- Behavior under workspace paths with explicit-protected DACLs.

Same posture as SP-3a / SP-6 v1: ship the code, lean on Windows CI.

## 7. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| `EXPLICIT_ACCESS_W.Trustee.ptstrName` is overloaded (string OR sid pointer based on `TrusteeForm`); compile-time type mismatch if windows-sys field type is `*mut PWSTR` instead of `*mut c_void` | Med | Cast explicitly in the implementation; verified by trying the actual import. If field is rigidly typed, use `BuildTrusteeWithSidW` instead — adds 1 Win32 call but is the documented "type-safe" path. |
| Author can't test locally → DACL bugs only surface in CI | High | Best-effort error handling means worst-case is the target failing on workspace write, not a process hang or privilege escalation. Soft-degrade behavior already characterized by SP-6 v1. |
| `SetNamedSecurityInfoW` is slow on large workspace trees due to inheritance propagation | Low | Fresh per-session workspaces are typically small or empty at grant time. If a regression appears, switch to `PROTECTED_DACL_SECURITY_INFORMATION` + recursive walk. |
| `LocalFree` of an `ACL*` cast — pointer comes from `SetEntriesInAclW` which docs say to free with `LocalFree`; the cast back through `*mut c_void` is technically UB-adjacent | Low | windows-sys' `LocalFree` takes `HLOCAL = *mut c_void` since 0.61, so the cast is the documented call shape. |
| Workspace path is a symlink/junction → ACL applies to the link, not the target | Med | Out of scope — caller (sandbox driver) is responsible for resolving the workspace_path to a canonical path before stuffing it into the policy. Add a `// caveat:` note in the helper docstring. |
| ACE leaks if `revoke_workspace_dacl` is skipped due to a panic between grant and revoke | Low | The init binary doesn't use panics; all error paths are explicit `Err(...)`. SID is invalidated by `DeleteAppContainerProfile` which runs in `cleanup_sids` regardless. |
| AppContainer profile + workspace DACL accumulate dead-weight ACEs in user's workspace dir over many runs if revoke keeps failing | Low | The SID is per-execution unique → each accumulated ACE is for a different invalid SID. Windows truncates `aclui.dll` displays of >1000 ACEs but does NOT enforce a hard ACL size limit until ~64KB. Document as "manual cleanup via `icacls /reset` is available if it becomes a problem". |

## 8. Alignment with redlines & principles

- **R3 (core minimalism)**: one file, ~130 LOC, zero new deps. ✓
- **R10 (thin harness)**: not in `src/harness/`; lives in OS-bridge code. ✓
- **P5 (least knowledge)**: helper takes `workspace_path: &str` + `ac_sid: PSID`; doesn't know about WindowsInitPolicy structure. ✓
- **P7 (defensive design)**: every unsafe Win32 call has explicit error check + log; best-effort posture; RAII guards prevent leaks even on early return. ✓

## 9. Implementation sequence

Single commit (no preliminary upgrade or refactor needed):

1. Add `LocalFreeGuard` helper to `imp` mod.
2. Add `grant_workspace_dacl` + `revoke_workspace_dacl` to `imp` mod.
3. Insert call sites in `launch_with_app_container` (step 3.5 + 6.5
   in the lifecycle).
4. Add cross-platform unit tests (constants + documentation tests).
5. `cargo check -p alephcore` on macOS — verify no regressions on
   cross-platform code.
6. `cargo test -p alephcore --lib sandbox` — verify all existing
   sandbox tests still pass.
7. `cargo clippy -p alephcore --lib --tests -- -D warnings` (touched
   files only, per memory: project main is NOT clippy-clean overall).
8. Update `docs/reference/SANDBOX.md` Windows section.
9. Commit: `sandbox: SP-6 v2 — wire workspace DACL grant for AppContainer SID`.

## 10. Out-of-scope follow-up specs

After SP-6 v2 ships, the residual Windows sandbox gaps are:
- **WFP per-host network filtering** (SP-3b) — admin-only; capability
  model in SP-6 covers most use cases; defer until concrete need
  emerges.
- **System-path access (`~/.gitconfig`, `%TEMP%`)** — accepted
  AppContainer limitation; users wanting these workflows can run
  outside AppContainer (sandbox degrades to SP-3a tier).
- **Recursive walk for explicit-protected DACL children** — not
  needed for fresh per-session workspaces; revisit if real workload
  hits the inheritance limitation.
