# SP-6 v2 Workspace DACL Grant — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire SP-6 v1's promised-but-unimplemented workspace DACL grant: give the per-execution AppContainer SID inheritable GENERIC_ALL access to its workspace before launch, then revoke after wait. Single file change in `src/sandbox/windows_init.rs`, no public API changes.

**Architecture:** Two new unsafe Win32 helpers inside the existing `imp` module (`grant_workspace_dacl` + `revoke_workspace_dacl` sharing one inner implementation); one RAII guard for `LocalFree` cleanup; two call sites inserted into the existing `launch_with_app_container` lifecycle at step 3.5 (after `CreateAppContainerProfile` succeeds) and step 6.5 (after `WaitForSingleObject` returns). One cross-platform constant (`DACL_INHERIT_FLAGS_FOR_APPCONTAINER`) with unit-test regression net. Best-effort failure handling throughout — DACL failure logs to stderr and continues; never blocks launch.

**Tech Stack:** Rust, `windows-sys 0.61` (already on; no upgrade), unsafe Win32 (`GetNamedSecurityInfoW`, `SetEntriesInAclW`, `SetNamedSecurityInfoW`, `EXPLICIT_ACCESS_W`).

**Working directory:** `/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/` (git worktree on branch `feat/sandbox-sp6-workspace-dacl`).

**Spec:** `docs/superpowers/specs/2026-05-20-sandbox-sp6v2-workspace-dacl-design.md` (committed at `fdecfb1c5`).

---

## File Structure

Single file modified:

- `src/sandbox/windows_init.rs` — top-of-module constant + `imp` mod additions + 2 call sites in `launch_with_app_container` + 1 unit test
- `docs/reference/SANDBOX.md` — one-paragraph update to Windows defense surface section

No new files; no driver, config, or platforms/mod.rs changes (the `workspace_path` field is already plumbed through policy JSON end-to-end by SP-6 v1).

---

## Task 1: Add inherit-flags constant + unit test (TDD)

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/src/sandbox/windows_init.rs` (insert after `capability_names_for_network` at line ~74; add test in existing `mod tests` near line ~944)

The constant is cross-platform so the test runs on macOS dev box. We hard-code the MSDN-documented bit values rather than referencing `windows_sys` constants (which only exist behind `cfg(target_os = "windows")`); the test acts as a regression net if anyone refactors the constant.

- [ ] **Step 1.1: Write the failing test in `mod tests`**

In `/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/src/sandbox/windows_init.rs`, locate the existing `#[cfg(test)] mod tests { ... }` block (starts at line ~825, closes at line ~945). Add this test as the last test inside the module, just before the closing `}`:

```rust
    #[test]
    fn dacl_inherit_flags_matches_msdn_documented_bits() {
        // OBJECT_INHERIT_ACE = 0x1, CONTAINER_INHERIT_ACE = 0x2 per
        // Microsoft Windows SDK winnt.h. If this fires, the constant
        // drifted and SP-6 v2 workspace DACL grant is no longer
        // inheritable, which means AppContainer targets cannot read
        // or write subdirectories of their workspace.
        assert_eq!(DACL_INHERIT_FLAGS_FOR_APPCONTAINER, 0x3);
    }
```

- [ ] **Step 1.2: Run test to verify it fails (constant doesn't exist yet)**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo test -p alephcore --lib sandbox::windows_init::tests::dacl_inherit_flags_matches_msdn_documented_bits 2>&1 | tail -15
```

Expected: compile error similar to `cannot find value DACL_INHERIT_FLAGS_FOR_APPCONTAINER in this scope`.

- [ ] **Step 1.3: Add the constant**

Locate the existing `pub fn capability_names_for_network` (line ~62-74). Immediately after its closing `}` (line ~74), insert:

```rust

/// SP-6 v2: DACL inheritance flags applied to AppContainer workspace
/// grants. `CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE` so the ACE
/// propagates to existing children whose default DACL inheritance is
/// enabled (the NTFS default) plus all future children.
///
/// MSDN documents `OBJECT_INHERIT_ACE = 0x1`,
/// `CONTAINER_INHERIT_ACE = 0x2`. Hard-coded so this constant — and
/// the regression test for it — work on the macOS / Linux dev boxes
/// without dragging in Win32 headers.
pub(crate) const DACL_INHERIT_FLAGS_FOR_APPCONTAINER: u32 = 0x2 | 0x1;
```

- [ ] **Step 1.4: Run test to verify it passes**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo test -p alephcore --lib sandbox::windows_init::tests::dacl_inherit_flags_matches_msdn_documented_bits 2>&1 | tail -5
```

Expected: `test result: ok. 1 passed; 0 failed`.

- [ ] **Step 1.5: Run the full sandbox::windows_init test module to confirm no regression**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo test -p alephcore --lib sandbox::windows_init 2>&1 | tail -5
```

Expected: all existing windows_init tests still pass (10 from SP-6 v1 + 1 new = 11 passed).

(No commit yet — accumulate changes; single commit at end per spec § 9.)

---

## Task 2: Add `LocalFreeGuard` RAII helper + `set_workspace_dacl_entry` Win32 helper

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/src/sandbox/windows_init.rs` inside `#[cfg(target_os = "windows")] mod imp { ... }` — add the guard after the `use` statements (around line 263, before the `LaunchError` enum at line ~266) and add the helper after `cleanup_sids` (ends line ~583, before `open_self_token` at line ~585).

This is pure unsafe Win32 plumbing. There's no cross-platform behavior we can TDD here; correctness is verified via `cargo check -p alephcore` on macOS (catches signature mismatches and import errors via windows-sys' cfg-gated types) and via Windows CI for runtime behavior (per spec § 6.3 — same posture as SP-6 v1).

- [ ] **Step 2.1: Add `LocalFreeGuard` struct inside `mod imp`**

In `/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/src/sandbox/windows_init.rs`, locate the `mod imp {` block. After the existing `use` statements (last `use` is `STARTUPINFOW,` at line 262, closing `};` at line 263), and before the `LaunchError` enum (`#[derive(Debug)] pub(super) enum LaunchError {` at line ~266), insert:

```rust

    /// SP-6 v2: RAII guard that calls `LocalFree` on `Drop`. Used for
    /// `PSECURITY_DESCRIPTOR` (returned by `GetNamedSecurityInfoW`)
    /// and `PACL` (returned by `SetEntriesInAclW`) — both system-
    /// allocated and documented to be freed with `LocalFree`.
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
```

- [ ] **Step 2.2: Add `set_workspace_dacl_entry` helper inside `mod imp`**

Locate `fn cleanup_sids(` (starts line ~567). Find its closing `}` (around line 583). Find the next function `fn open_self_token(` (starts line ~585). Insert between them:

```rust

    /// SP-6 v2: Add or remove an inheritable allow ACE for `ac_sid` on
    /// `workspace_path`. Same code path for both grant (`mode =
    /// GRANT_ACCESS`) and revoke (`mode = REVOKE_ACCESS`) since the
    /// only difference is one field in `EXPLICIT_ACCESS_W`.
    ///
    /// Best-effort: any failure returns `Err(String)` and the caller
    /// logs + continues. Never panics.
    ///
    /// Caveat: assumes `workspace_path` is already canonical (the
    /// driver populates the policy with the resolved session workspace
    /// dir; we do not resolve symlinks here).
    unsafe fn set_workspace_dacl_entry(
        workspace_path: &str,
        ac_sid: *mut core::ffi::c_void,
        mode: ACCESS_MODE,
    ) -> Result<(), String> {
        use std::iter::once;
        use windows_sys::Win32::Foundation::ERROR_SUCCESS;
        use windows_sys::Win32::Security::Authorization::{
            GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
            EXPLICIT_ACCESS_W, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
        };
        use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION};
        use windows_sys::Win32::System::SystemServices::GENERIC_ALL;

        let path_w: Vec<u16> = workspace_path.encode_utf16().chain(once(0)).collect();

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
                "GetNamedSecurityInfoW({workspace_path}) failed: {status:#010x}"
            ));
        }
        let _sd_guard = LocalFreeGuard(sd);

        // 2. Build EXPLICIT_ACCESS_W for the AppContainer SID.
        let mut ea: EXPLICIT_ACCESS_W = std::mem::zeroed();
        ea.grfAccessPermissions = GENERIC_ALL;
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
            return Err(format!("SetEntriesInAclW failed: {status:#010x}"));
        }
        let _dacl_guard = LocalFreeGuard(new_dacl as *mut core::ffi::c_void);

        // 4. Write the merged DACL back to the workspace.
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
                "SetNamedSecurityInfoW({workspace_path}) failed: {status:#010x}"
            ));
        }
        Ok(())
    }
```

- [ ] **Step 2.3: Add `LocalFree` to the existing `use` imports**

The `use` block at the top of `mod imp` already imports `LocalFree` (line 247-249: `use windows_sys::Win32::Foundation::{ CloseHandle, GetLastError, LocalFree, INVALID_HANDLE_VALUE, HANDLE, };`). No change needed — `LocalFree` is already in scope.

Also `ACCESS_MODE` is the type of `EXPLICIT_ACCESS_W.grfAccessMode`. In windows-sys 0.61 it lives in `Win32::Security::Authorization::ACCESS_MODE`. The helper's signature parameter `mode: ACCESS_MODE` requires this type to be in scope where the function is *defined*. Add to the existing `use` block at top of `mod imp` (line 250-254, currently importing types from `Win32::Security`):

Find this block in `mod imp`:

```rust
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, SetTokenInformation, TokenIntegrityLevel,
        DISABLE_MAX_PRIVILEGE, SE_GROUP_INTEGRITY, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY,
        TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
```

Add a new `use` line directly after it:

```rust
    use windows_sys::Win32::Security::Authorization::ACCESS_MODE;
```

- [ ] **Step 2.4: Verify cargo check compiles on macOS dev box**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo check -p alephcore 2>&1 | tail -10
```

Expected: clean compile. Since the `mod imp` block is `#[cfg(target_os = "windows")]`-gated, macOS doesn't compile the new helper directly — but the surrounding cross-platform code (including the new `pub(crate) const DACL_INHERIT_FLAGS_FOR_APPCONTAINER` from Task 1) must compile.

If you see a compile error citing `DACL_INHERIT_FLAGS_FOR_APPCONTAINER` not found inside `mod imp`, the const visibility is `pub(crate)` at the crate root path `crate::sandbox::windows_init::DACL_INHERIT_FLAGS_FOR_APPCONTAINER` — confirm the helper references it via the full path (the helper code above already does).

(No commit yet — keep accumulating.)

---

## Task 3: Wire grant + revoke into `launch_with_app_container` lifecycle

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/src/sandbox/windows_init.rs` — two insertions inside `pub(super) fn launch_with_app_container` (starts line ~317).

Step 3.5 (grant) goes after `CreateAppContainerProfile` succeeds and ac_sid is populated. Step 6.5 (revoke) goes after `WaitForSingleObject + GetExitCodeProcess` complete and before `cleanup` runs `DeleteAppContainerProfile`.

- [ ] **Step 3.1: Insert grant call site after `CreateAppContainerProfile` success**

In `launch_with_app_container`, locate the block ending with the `if hr != 0 { ... return Err(LaunchError::AppContainerSetupFailed(format!("CreateAppContainerProfile failed: hr={hr:#010x}"))); }` (around line 410-421). The next existing block is the `// ---------- 4. Build SECURITY_CAPABILITIES + attribute list ----------` comment (line ~423).

Insert this block between the two — i.e., after the closing `}` of the `if hr != 0` block, before the `// ---------- 4. ...` comment:

```rust

        // ---------- 3.5. SP-6 v2: grant workspace DACL ----------
        // Best-effort: if the grant fails, the target may fail on
        // workspace writes but the sandbox itself stays up. We never
        // hard-fail this step (per spec § 3, even when
        // require_app_container=true).
        if let Some(ref ws) = parsed.policy.workspace_path {
            if let Err(e) = set_workspace_dacl_entry(
                ws,
                ac_sid,
                windows_sys::Win32::Security::Authorization::GRANT_ACCESS,
            ) {
                eprintln!(
                    "aleph sandbox-init-windows: workspace DACL grant failed ({e}); \
                     target may fail on workspace writes"
                );
            }
        }
```

- [ ] **Step 3.2: Insert revoke call site after `GetExitCodeProcess`**

Locate the section labeled `// ---------- 6. Wait + GetExitCode ----------` (around line ~533). It contains `WaitForSingleObject`, builds `wait_err`, then calls `GetExitCodeProcess` setting `code_ok`. Right after the `let code_ok = unsafe { GetExitCodeProcess(pi.hProcess, &mut code) };` line (around line 542), and before the next block labeled `// ---------- 7. Cleanup (always runs) ----------` (around line 544), insert:

```rust

        // ---------- 6.5. SP-6 v2: revoke workspace DACL ----------
        // Best-effort. The SID is about to be invalidated by
        // DeleteAppContainerProfile, which makes any leftover ACE dead
        // weight (spec § 7 risk register), so revoke failure is logged
        // but ignored.
        if let Some(ref ws) = parsed.policy.workspace_path {
            if let Err(e) = set_workspace_dacl_entry(
                ws,
                ac_sid,
                windows_sys::Win32::Security::Authorization::REVOKE_ACCESS,
            ) {
                eprintln!(
                    "aleph sandbox-init-windows: workspace DACL revoke failed ({e}); \
                     AppContainer SID is about to be invalidated, ACE will become dead weight"
                );
            }
        }
```

- [ ] **Step 3.3: Verify cargo check still compiles**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo check -p alephcore 2>&1 | tail -10
```

Expected: clean compile on macOS. The fully-qualified `windows_sys::Win32::Security::Authorization::GRANT_ACCESS` / `REVOKE_ACCESS` paths are gated by the surrounding `#[cfg(target_os = "windows")] mod imp` block, so macOS doesn't compile them.

- [ ] **Step 3.4: Run full sandbox lib test suite to verify no regression**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo test -p alephcore --lib sandbox 2>&1 | tail -5
```

Expected: all 159 existing sandbox lib tests pass + 1 new test from Task 1 = 160 passed.

If the count differs from 160, diff the failing tests against `[[project_baseline_test_failures]]` (per memory feedback: main has pre-existing failures that aren't ours — but sandbox specifically was clean post-SP-6 v1).

(No commit yet.)

---

## Task 4: Update SANDBOX.md + clippy + final commit

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/docs/reference/SANDBOX.md` — append one paragraph to the existing SP-6 / Windows defense surface subsection.

- [ ] **Step 4.1: Read the current SANDBOX.md to locate the SP-6 section**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && grep -n "AppContainer\|SP-6" docs/reference/SANDBOX.md | head -20
```

Expected: the grep prints line numbers of "AppContainer" and "SP-6" mentions. Use the topmost line in the "Current Windows defense surface" subsection (or the closest equivalent) to anchor the edit. The SP-6 v1 commit added a multi-line block describing the AppContainer tier.

- [ ] **Step 4.2: Append a one-paragraph SP-6 v2 update**

Use Edit tool to insert a paragraph at the end of the AppContainer description block in SANDBOX.md. The new paragraph reads:

```markdown

**SP-6 v2 (2026-05-20)**: the workspace DACL grant promised by SP-6 v1
§ 2.4 is now wired. Before each AppContainer launch, the init process
adds an inheritable `GENERIC_ALL` allow ACE for the per-execution
AppContainer SID on the session workspace directory; after the target
exits, the same helper revokes the ACE (best-effort). Failure at any
step logs to stderr and continues — DACL is an enabler, not a sandbox
enforcement primitive, so the sandbox itself never blocks on it.
Targets that don't need workspace writes (computation-only) are
unaffected. Targets requiring system paths (`~/.gitconfig`, `%TEMP%`)
remain an accepted AppContainer limitation.
```

The Edit tool invocation (adjust `old_string` to match an existing nearby line in your local file — use the closing of the SP-6 v1 paragraph or whatever immediately precedes the next subsection):

```python
# Example shape — actual old_string must be exact from your file
Edit(
    file_path="/Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl/docs/reference/SANDBOX.md",
    old_string="<exact final line of existing SP-6 paragraph from your file>",
    new_string="<that same line>\n\n**SP-6 v2 (2026-05-20)**: ..."
)
```

- [ ] **Step 4.3: Run clippy on touched files only**

Per memory `[[project_fmt_clippy_baseline_drift]]`: main is NOT clippy-clean overall; only the files we touched must pass `-D warnings`.

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo clippy -p alephcore --lib --tests -- -A clippy::all -W clippy::correctness -W clippy::suspicious -W clippy::perf 2>&1 | grep -E "windows_init|warning|error" | head -30
```

Expected: zero warnings/errors mentioning `windows_init`. If a real correctness/suspicious warning fires on the new code, fix it inline; pre-existing warnings on other files are tolerated.

If you want stricter coverage scoped to the touched file:

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo clippy -p alephcore --lib --tests -- -D warnings 2>&1 | grep -B 1 -A 5 "windows_init"
```

Expected: no output (no warnings from the touched file).

- [ ] **Step 4.4: Final test sweep before commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && cargo test -p alephcore --lib sandbox 2>&1 | tail -5
```

Expected: `test result: ok. 160 passed; 0 failed`.

- [ ] **Step 4.5: Commit (single commit per spec § 9)**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && git status
```

Expected output (unstaged):
```
modified:   src/sandbox/windows_init.rs
modified:   docs/reference/SANDBOX.md
```

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && git add src/sandbox/windows_init.rs docs/reference/SANDBOX.md && git commit -m "$(cat <<'EOF'
sandbox: SP-6 v2 — wire workspace DACL grant for AppContainer SID

Closes the gap left by SP-6 v1: launch_with_app_container accepted a
workspace_path policy field but never used it, leaving every target
needing workspace writes blocked by ACCESS_DENIED under the strictest
sandbox tier.

Adds:
- DACL_INHERIT_FLAGS_FOR_APPCONTAINER cross-platform constant + test
  (regression net for the inherit-flag bits we hard-code from MSDN).
- LocalFreeGuard RAII helper for PSECURITY_DESCRIPTOR + PACL cleanup.
- set_workspace_dacl_entry unsafe Win32 helper that GETs the existing
  DACL, builds an EXPLICIT_ACCESS_W for the AppContainer SID with
  GENERIC_ALL + inheritance, merges via SetEntriesInAclW, and writes
  back via SetNamedSecurityInfoW.
- Two call sites in launch_with_app_container (step 3.5 grant, step 6.5
  revoke) using the same helper with GRANT_ACCESS / REVOKE_ACCESS.

Best-effort throughout: any DACL failure logs to stderr and continues
without aborting the launch. The SID is invalidated by
DeleteAppContainerProfile anyway, so a leftover ACE is dead weight.

No public API changes; no windows-sys version bump; no new features.
Single file change in src/sandbox/windows_init.rs (~130 LOC) + one
paragraph in docs/reference/SANDBOX.md.

Test verification (macOS dev box):
- cargo check -p alephcore: clean
- cargo test -p alephcore --lib sandbox: 160 passed (159 from SP-6 v1
  + 1 new dacl_inherit_flags_matches_msdn_documented_bits)
- cargo clippy on touched files: clean

Runtime verification deferred to Windows CI (same posture as
SP-3a / SP-6 v1 unsafe Win32 paths).
EOF
)"
```

- [ ] **Step 4.6: Verify final state**

```bash
cd /Volumes/TBU4/Workspace/Aleph-sp6-workspace-dacl && git log --oneline -3
```

Expected (newest at top):
```
<sha> sandbox: SP-6 v2 — wire workspace DACL grant for AppContainer SID
fdecfb1c5 docs: spec — SP-6 v2 workspace DACL grant for AppContainer SID
819f40b43 merge: sandbox hardening cycle 1 (SP-2..SP-6) + cycle 1 A+C
```

---

## Self-Review Notes

**Spec coverage check (against `2026-05-20-sandbox-sp6v2-workspace-dacl-design.md`):**

| Spec section | Covered by |
|---|---|
| § 1 Goal & scope — wire DACL grant + revoke | Tasks 2, 3 |
| § 1 best-effort always (no `require_*` escalation) | Step 3.1 + 3.2 (no flag check; just log + continue) |
| § 2.2 Lifecycle steps 3.5 + 6.5 | Steps 3.1 + 3.2 |
| § 2.3 grant pseudocode | Step 2.2 (full helper) |
| § 2.4 revoke pseudocode (mirror of grant) | Step 2.2 (same helper, mode param) |
| § 2.5 RAII (LocalFreeGuard) | Step 2.1 |
| § 2.6 Inheritance flags | Task 1 (constant) + Step 2.2 (used) |
| § 4 File-level changes | Task 4 (SANDBOX.md) + accumulated changes (windows_init.rs) |
| § 5 windows-sys feature audit (no upgrade) | No-op (verified, see § 5 of spec) |
| § 6.1 cross-platform unit tests | Task 1 |
| § 6.2 Windows CI integration tests | Deferred per spec (CI's job) |
| § 9 implementation sequence | Tasks 1–4 |

**Placeholder scan:** All steps contain real commands, real code, and real expected output. No "TBD" / "implement later" / "add appropriate error handling" / "similar to Task N" patterns.

**Type consistency check:** Helper named `set_workspace_dacl_entry` is used consistently in Steps 2.2, 3.1, 3.2. Constant named `DACL_INHERIT_FLAGS_FOR_APPCONTAINER` is defined in Task 1 and used in Step 2.2. `LocalFreeGuard` is defined in Step 2.1 and used in Step 2.2. `ACCESS_MODE` import added in Step 2.3, used as function param in Step 2.2 — verified consistent.

Caveat for the executor: SANDBOX.md exact anchor line (Step 4.2's `old_string`) depends on what SP-6 v1's docs commit landed; the plan describes the *shape* of the edit but requires the executor to inspect the current file and pick the right anchor. This is intentional — the spec is small and the doc structure is fluid.
