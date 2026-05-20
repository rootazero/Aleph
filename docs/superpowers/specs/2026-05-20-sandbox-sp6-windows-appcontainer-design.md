# SP-6 — Windows AppContainer

**Date**: 2026-05-20
**Status**: Design
**Branch**: `feat/sandbox-hardening-cycle1` (continued in same worktree)
**Predecessor**: `2026-05-20-sandbox-hardening-cycle1-design.md` § 9 (SP-6 entry) + `2026-05-20-sandbox-sp3a-windows-restricted-token-design.md`

## 1. Goal & scope

Add AppContainer as the strongest Windows sandbox primitive on Aleph's
soft-degrade chain. AppContainer is what Edge / Chrome / UWP use; it
combines:
- A per-execution **AppContainer SID** — files and registry keys not
  granted to that SID are inaccessible to the contained process.
- **Capability-based access** — the process gets exactly the
  capabilities (`INTERNET_CLIENT`, `PRIVATE_NETWORK_CLIENT_SERVER`, …)
  enumerated at launch; everything else returns ACCESS_DENIED.
- A trust level **below Low IL** — effectively "Untrusted".

This is strictly stronger than SP-3a's `CreateRestrictedToken + Low IL`
combo. SP-6 replaces SP-3a's restricted-token launch path inside
`sandbox-init-windows` as the new top of the soft-degrade chain:

```
AppContainer (SP-6) → CreateRestrictedToken+LowIL (SP-3a) → CreateProcessW (cycle 1)
```

**In scope (this cycle)**:
- `windows-sys` upgrade `0.59 → 0.61` (required for stable
  `Win32_Security_AppContainer` + `Win32_Security_Isolation` feature
  modules). Existing Win32 call sites (job.rs, windows_init.rs,
  windows/driver.rs) audited and fixed for any signature drift.
- `aleph-server sandbox-init-windows` gains a `launch_with_app_container`
  path inside the existing `imp` module.
- Capability SID list derived from `SandboxCapabilities.network`:
  - `NetworkPolicy::None` → empty capability list (no network)
  - `NetworkPolicy::AllowAll` → `[INTERNET_CLIENT, PRIVATE_NETWORK_CLIENT_SERVER]`
  - `NetworkPolicy::AllowHosts(_)` → not supported in SP-6 (WFP is the
    only mechanism that does per-host on Windows; deferred indefinitely).
- Workspace DACL grant: an Allow-Modify ACE is added for the
  AppContainer SID on the session workspace directory before spawn;
  removed in cleanup.
- Per-execution unique AppContainer profile name; `CreateAppContainerProfile`
  before spawn, `DeleteAppContainerProfile` after wait. No persistent
  registry state.
- Soft-degrade: any AppContainer step fails → fall through to SP-3a
  restricted token path (which itself further degrades to plain
  `CreateProcessW` per its own logic). One `warn` per process per
  degradation step.

**Out of scope** (deferred):
- WFP integration for per-host network filtering inside AppContainer
  (still admin-only on Windows; tracked as separate spec).
- Registry virtualization tuning (we accept the default AppContainer
  registry redirection).
- `LowBox` direct API (predecessor of AppContainer, more privileged
  but harder to manage).
- Persistent shared AppContainer profile across executions (extra
  complexity for marginal startup win).

**Success criteria**:
1. On Windows 10+ where AppContainer profiles can be created, sandboxed
   commands run inside a per-execution AppContainer; the target sees
   ACCESS_DENIED on every filesystem path except its workspace.
2. Where `CreateAppContainerProfile` fails (locked-down policies,
   non-AppContainer-capable Windows versions), the soft-degrade chain
   silently advances to SP-3a's restricted token.
3. AppContainer profiles are cleaned up after each run; no orphan
   profiles accumulate in the user's registry.
4. macOS / Linux code paths untouched.
5. Existing cycle 1 / SP-2 / SP-4 / SP-5 / SP-3a tests continue to pass.

## 2. Architecture

### 2.1 Where AppContainer fits in the chain

Today (post-SP-3a) inside `sandbox-init-windows`:

```
run_init(args)
  → launch_with_restricted_token(parsed)
      → if PrivilegeNotHeld and !require_restricted_token:
           launch_with_host_token(parsed)
```

Post-SP-6:

```
run_init(args)
  → if policy.use_app_container:
       launch_with_app_container(parsed)
         on AppContainerSetupFailed and !require_app_container:
             warn + fall through to ↓
  → launch_with_restricted_token(parsed)
      → if PrivilegeNotHeld and !require_restricted_token:
           launch_with_host_token(parsed)
```

The `WindowsInitPolicy` JSON gains one field; everything else in the
SP-3a code path stays identical.

### 2.2 AppContainer profile lifecycle

```c
// Setup (host side, before CreateProcessAsUserW):
WCHAR name[64];  // "aleph-sandbox-<pid>-<nanos>"
PSID app_container_sid = NULL;
HRESULT hr = CreateAppContainerProfile(
    name,
    display_name,
    description,
    capabilities,         // SID_AND_ATTRIBUTES[]
    capability_count,
    &app_container_sid
);
if (hr == E_ACCESSDENIED || hr == 0x800700b7 /* ERROR_ALREADY_EXISTS */) {
    // soft-degrade
    return AppContainerSetupFailed;
}

// Workspace DACL grant (best-effort; many targets work even if this fails):
EXPLICIT_ACCESS_W ea = { GENERIC_ALL, GRANT_ACCESS, ... };
ea.Trustee.ptstrName = (LPWSTR)app_container_sid;
PACL new_dacl = NULL;
SetEntriesInAclW(1, &ea, existing_dacl, &new_dacl);
SetNamedSecurityInfoW(workspace, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, ...);

// Spawn (with STARTUPINFOEX carrying PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES):
SECURITY_CAPABILITIES sc = { app_container_sid, capabilities, capability_count };
SIZE_T attr_size = 0;
InitializeProcThreadAttributeList(NULL, 1, 0, &attr_size);
LPPROC_THREAD_ATTRIBUTE_LIST attr_list = HeapAlloc(GetProcessHeap(), 0, attr_size);
InitializeProcThreadAttributeList(attr_list, 1, 0, &attr_size);
UpdateProcThreadAttribute(
    attr_list, 0,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    &sc, sizeof(SECURITY_CAPABILITIES),
    NULL, NULL
);
STARTUPINFOEXW si = { sizeof(STARTUPINFOEXW), 0 };
si.StartupInfo.cb = sizeof(STARTUPINFOEXW);
si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
si.StartupInfo.hStdInput  = GetStdHandle(STD_INPUT_HANDLE);
si.StartupInfo.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
si.StartupInfo.hStdError  = GetStdHandle(STD_ERROR_HANDLE);
si.lpAttributeList = attr_list;
PROCESS_INFORMATION pi = {0};
BOOL ok = CreateProcessW(
    NULL,
    cmd_line,
    NULL, NULL,
    TRUE,
    EXTENDED_STARTUPINFO_PRESENT,
    NULL, NULL,
    &si.StartupInfo,
    &pi
);

WaitForSingleObject(pi.hProcess, INFINITE);
GetExitCodeProcess(pi.hProcess, &code);

// Cleanup:
DeleteProcThreadAttributeList(attr_list);
HeapFree(GetProcessHeap(), 0, attr_list);
FreeSid(app_container_sid);
DeleteAppContainerProfile(name);  // ignore failure
// Workspace DACL: revert the ACE we added (best-effort)
```

Notes:
- `CreateProcessW` (not `CreateProcessAsUserW`) — the AppContainer
  identity comes from `SECURITY_CAPABILITIES`, not from a primary token
  swap. This sidesteps the `SE_INCREASE_QUOTA` requirement that bedeviled
  SP-3a.
- `bInheritHandles = TRUE` so stdio HANDLEs pass through.
- `EXTENDED_STARTUPINFO_PRESENT` is required for the attribute list to
  be honored.

### 2.3 Capability SID derivation

Capability SIDs are well-known per Win32; we resolve at runtime via
`DeriveCapabilitySidsFromName`:

| `NetworkPolicy` | Capability name(s) |
|---|---|
| `None` | (none) |
| `AllowAll` | `internetClient`, `privateNetworkClientServer` |
| `AllowHosts(_)` | not applicable — returns `UnsupportedPolicy` at `profile_for` time |

Filesystem capabilities (`documentsLibrary`, `picturesLibrary`, etc.)
are NOT granted. The target only sees its workspace via the explicit
DACL we add in step 2.

### 2.4 Workspace DACL grant + cleanup

Without an ACL grant on the workspace, an AppContainer can't read the
contents the sandbox owner placed there. We add a `GENERIC_ALL` ACE
keyed on the per-execution AppContainer SID before spawn, then remove
it (or let it expire when the profile is deleted — Windows automatically
invalidates ACEs for deleted SIDs, but cleanup is hygienic).

Best-effort: a SetNamedSecurityInfoW failure logs but doesn't abort.
Some targets work fine without workspace writes (e.g. computation-only
commands).

## 3. windows-sys upgrade

`0.59 → 0.61` because:
- `0.59` lacks stable `CreateAppContainerProfile` /
  `DeleteAppContainerProfile` / `DeriveCapabilitySidsFromName` in the
  user-mode feature set.
- `0.61` ships them under `Win32_Security_Authorization` + a new
  `Win32_Security_AppContainer` module.
- Migration risk: enum repr changes for `BOOL`, removal of a few
  pre-0.60 deprecated alias symbols. Existing call sites:
  - `src/sandbox/platforms/windows/job.rs` (JobObject API).
  - `src/sandbox/platforms/windows/driver.rs` (CREATE_NEW_PROCESS_GROUP import).
  - `src/sandbox/windows_init.rs` (the SP-3a Win32 surface).

Plan: bump version → run `cargo check -p alephcore` on macOS (which
doesn't compile the Linux/Windows-gated blocks, but exercises the
top-level dep graph). Then ship as commit 1; SP-6 logic as commit 2.

Existing `Cargo.toml` features (add `Win32_Security_AppContainer`):

```toml
windows-sys = { version = "0.61", features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_Security_AppContainer",   # NEW: AppContainer profile API
    "Win32_Security_Authorization",
    "Win32_System_Threading",
    "Win32_System_JobObjects",
    "Win32_Storage_FileSystem",
    "Win32_System_Console",
    "Win32_System_Pipes",
    "Win32_System_Memory",
    "Win32_System_SystemServices",   # NEW: PROC_THREAD_ATTRIBUTE_* constants
] }
```

## 4. File-level changes

| File | Change | Approx LOC |
|---|---|---|
| `Cargo.toml` | `windows-sys 0.59 → 0.61` + new `Win32_Security_AppContainer` and `Win32_System_SystemServices` features. | +3 |
| `src/sandbox/windows_init.rs` | Extend `WindowsInitPolicy`: + `use_app_container: bool` + `require_app_container: bool` + capability list. Extend `imp`: + `launch_with_app_container`. `run_init` calls the new path first, falls through on `AppContainerSetupFailed`. Cross-platform tests verify JSON shape + default values + parser. | +220 |
| `src/sandbox/platforms/windows/driver.rs` | `WindowsSandboxOptions` gains `use_app_container` + `require_app_container`. `profile_for` populates them in the serialized policy. | +12 |
| `src/sandbox/config.rs` | `WindowsSandboxConfig.use_app_container: bool` (default `true`) + `require_app_container: bool` (default `false`). | +10 |
| `src/sandbox/platforms/mod.rs` | Thread the new config fields into `WindowsSandboxOptions`. | +2 |
| `src/sandbox/platforms/windows/job.rs` | Fix any `0.59→0.61` API drift. Expected mostly no-op. | tbd |
| `docs/reference/SANDBOX.md` | Update "Current Windows defense surface" with AppContainer; deferred table marks SP-6 strike-through. | +25 |

Net: ~270 LOC + the upgrade-fix lines (best estimate <30).

## 5. Error handling

| Failure | Behavior (`require_app_container=false`) | Behavior (`require_app_container=true`) |
|---|---|---|
| `CreateAppContainerProfile` returns `E_ACCESSDENIED` / `E_INVALIDARG` | warn + soft-degrade to SP-3a path | init exits 64 |
| `DeriveCapabilitySidsFromName` fails for a known cap name | warn + soft-degrade | init exits 64 |
| `InitializeProcThreadAttributeList` / `UpdateProcThreadAttribute` failure | warn + soft-degrade | init exits 64 |
| `CreateProcessW(EXTENDED_STARTUPINFO_PRESENT)` failure | warn + soft-degrade | init exits 64 |
| `SetNamedSecurityInfoW` (workspace DACL) failure | warn but continue (some targets don't need workspace write) | warn but continue (same) |
| `WaitForSingleObject` / `GetExitCodeProcess` failure | exit 65 (matches SP-3a's WaitFailed) | same |

Cleanup paths always best-effort (`DeleteAppContainerProfile`,
`FreeSid`, attribute-list teardown) — if they fail, we log but don't
override the target's exit code.

## 6. Testing

### Unit tests (cross-platform — pure logic + JSON shape)

- `policy_round_trips_with_app_container_flags`.
- `policy_default_enables_app_container_disables_require`.
- `parse_init_args_extracts_app_container_policy` (mirrors SP-3a).
- `capability_names_for_allow_all` returns `["internetClient", "privateNetworkClientServer"]`.
- `capability_names_for_none` returns `[]`.

### Windows CI integration tests (`#[cfg(target_os = "windows")] + #[ignore]`)

- `app_container_profile_created_and_destroyed` — spawn a no-op target;
  inspect `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\
  CurrentVersion\AppContainer\Mappings\<sid>` doesn't exist after run.
- `target_runs_under_app_container_sid` — spawn `whoami /groups`, assert
  output contains the per-execution AppContainer SID.
- `allow_all_grants_internet_capability` — sandbox a `Test-NetConnection`
  call; assert it succeeds.
- `none_denies_internet` — same call without capability; assert
  `ACCESS_DENIED` / network failure.
- `soft_degrade_when_appcontainer_fails` — manual test path; not CI-automated.

### What we don't test from macOS dev box

- The actual `CreateAppContainerProfile` / `CreateProcessW` chain.
- Stdio inheritance under AppContainer.
- Workspace DACL behavior.

Same posture as SP-3a: ship the code, lean on Windows CI.

## 7. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| Author can't test locally → bugs land before CI catches | High | Code structured to minimize unsafe surface; soft-degrade chain is the design's first-class fallback. Every AppContainer setup error path leads back to SP-3a behavior. |
| `0.59 → 0.61` breaks an unrelated Windows call site | Med | Pre-flight: every `windows-sys` import audited; `cargo check` enforces on every commit. |
| `CreateAppContainerProfile` fails on Windows Server (no AppContainer support pre-Server 2016) | Med | Soft-degrade. Documented. |
| Per-execution profile name collisions in pathological parallel-execution scenarios | Low | Name includes both pid and ns-resolution timestamp; collision requires same pid + same ns → effectively impossible. |
| Workspace DACL grant leaks an ACE if cleanup fails | Low | AppContainer SID is per-execution unique; even if the ACE remains, the SID is invalid post-cleanup → ACE is dead weight, not a privilege escalation. Document. |
| Targets relying on system file access (e.g. `git` reading `~/.gitconfig`) fail under strict AppContainer | High | Expected. AppContainer is the strictest tier; users invoking strict mode know what they're signing up for. Document. |
| `DeleteAppContainerProfile` leaves a 32-byte registry entry per execution if cleanup fails | Low | Best-effort cleanup. Manual `Remove-AppxPackage`-style cleanup ops can run on a schedule if it becomes a problem. |
| Capability names change between Windows versions | Low | Use string capability names (resolved via `DeriveCapabilitySidsFromName`) rather than hard-coded SIDs; the documented names are stable across releases. |

## 8. Alignment with redlines & principles

- **R3 (core minimalism)**: `windows-sys` upgrade is the version-bump
  variety, not a new crate. AppContainer logic lives in the existing
  `windows_init.rs`. ✓
- **R10 (thin harness)**: `launch_with_app_container` is one function;
  no AppContainer-management framework, no profile-pool, no abstraction
  layer. ✓
- **P5 (least knowledge)**: driver knows only "policy JSON + target +
  args"; init owns AppContainer Win32 details. ✓
- **P7 (defensive design)**: every step soft-degrades; require_*
  config flags promote to hard error. Cleanup runs unconditionally. ✓

## 9. Implementation sequence

1. **windows-sys upgrade** (its own commit so the diff is reviewable):
   - `Cargo.toml`: bump `0.59 → 0.61`, add
     `Win32_Security_AppContainer` and `Win32_System_SystemServices`
     features.
   - `cargo check -p alephcore` on macOS — fix any non-Linux/Windows
     call sites that broke. The macOS path doesn't pull `windows-sys`
     at all, so this mostly proves the lockfile resolves.
   - Manual audit: every `windows_sys::Win32::…` import in
     `windows_init.rs`, `windows/job.rs`, `windows/driver.rs` is still
     valid against 0.61's API surface.
   - Commit: `sandbox: upgrade windows-sys 0.59 → 0.61 for SP-6 AppContainer`.

2. **SP-6 logic** (second commit):
   - `windows_init.rs`: add `use_app_container` + `require_app_container`
     + capability list to `WindowsInitPolicy`; new `imp::launch_with_app_container`
     function; new `LaunchError::AppContainerSetupFailed`; cross-platform
     capability-name helper + tests.
   - `windows/driver.rs`: extend `WindowsSandboxOptions`; profile_for
     populates new fields; refuse `AllowHosts` at profile_for time with
     a SP-6-specific message (clarifying that WFP is the missing piece).
   - `config.rs`: new fields with defaults + serde.
   - `platforms/mod.rs`: thread fields.
   - `cargo check -p alephcore` on macOS.
   - `cargo test -p alephcore --lib sandbox` (regression + new SP-6 unit tests).
   - `cargo test -p alephcore --test sandbox_capability_approval` (regression).
   - `cargo clippy -p alephcore --lib --tests -- -D warnings` (touched-file regressions only).
   - Update `docs/reference/SANDBOX.md` "Current Windows defense surface" + deferred table.
   - Commit: `sandbox: SP-6 — Windows AppContainer at top of soft-degrade chain`.

## 10. Out-of-scope follow-up specs

After SP-6 ships, the only major Windows hardening item left is **WFP
per-host network filtering**, which we already deferred as SP-3b. WFP
requires admin and overlaps poorly with AppContainer's capability
model; treat as a separate research cycle if user demand emerges.

## 11. Closing note on the broader cycle

SP-6 is the final spec on the brainstorming list. After this lands,
the cycle 1 closeout becomes complete: every one of SP-2 / SP-3a /
SP-4 / SP-5 / SP-6 has shipped; only SP-3b (WFP, admin-only) remains
unshipped — appropriately so, as the user already noted that "二选一"
between SP-3b and SP-6 should resolve in SP-6's favor.
