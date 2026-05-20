# SP-3a — Windows Restricted Token + Low Integrity Level

**Date**: 2026-05-20
**Status**: Design
**Branch**: `feat/sandbox-hardening-cycle1` (continued in same worktree)
**Predecessor**: `2026-05-20-sandbox-hardening-cycle1-design.md` § 9 (SP-3 entry, split per SP-4 closeout into SP-3a + SP-3b)

## 1. Goal & scope

Add Windows defense-in-depth equivalent to SP-2's Linux landlock+seccomp:
strip privileges from the sandboxed process via a Chrome-pattern
restricted token + drop integrity level to Low. Today's Windows path
relies on `JobObject` containment only — the child runs with the user's
full token and Medium integrity, which is far weaker than the Linux
post-SP-2 stance.

**In scope (this cycle)**:
- `aleph-server sandbox-init-windows` hidden subcommand (parallel to
  SP-2's `sandbox-init`).
- Inside the subcommand:
  - `OpenProcessToken(GetCurrentProcess(), ...)` to grab the host's token.
  - `CreateRestrictedToken(.., DISABLE_MAX_PRIVILEGE, ..)` to derive a
    restricted token (no privileges except `SeChangeNotifyPrivilege`).
  - `SetTokenInformation(.., TokenIntegrityLevel, ..)` to drop integrity
    to Low (`S-1-16-4096`).
  - `CreateProcessAsUserW(restricted_token, target, ..)` with inherited
    stdio so output flows back through the init's stdio (which is itself
    inherited from the tokio-spawned init process).
  - Wait for target, exit with target's exit code.
- Driver-side: `WindowsSandboxDriver::run` wraps the user program with
  `aleph-server sandbox-init-windows --policy <json> -- <program> <args>`,
  spawned via the existing tokio `Command` path (unchanged stdio
  handling, unchanged JobObject attach).
- Soft-degrade: if `CreateProcessAsUserW` returns
  `ERROR_PRIVILEGE_NOT_HELD` (1314 — host lacks `SE_INCREASE_QUOTA`),
  fall through to direct `CreateProcessW(target, ...)`. JobObject still
  applies. Single `tracing::warn!` per process.

**Out of scope** (deferred):
- AppContainer (SP-6) — proper modern Windows sandbox primitive; the
  user accepted that SP-3b vs SP-6 is a separate decision after we see
  SP-3a in flight.
- WFP / per-host network filtering (SP-3b).
- DACL on workspace files (separate Windows-side hardening cycle).
- Restricted token with custom SID denylist beyond the
  `DISABLE_MAX_PRIVILEGE` blanket strip.

**Success criteria**:
1. On Windows where `SE_INCREASE_QUOTA` is granted (standard desktop
   user accounts), sandboxed commands run with a restricted primary
   token (no privileges) at Low integrity. Verified by Windows CI.
2. Where it isn't granted (locked-down server policies), the sandbox
   spawns the target with the normal token + JobObject; one warn line
   per process documents the degradation.
3. macOS / Linux code paths untouched.
4. Existing cycle 1 / SP-4 / SP-2 / SP-5 tests continue to pass.

## 2. Architecture

### 2.1 Why the init-subcommand pattern (consistency with SP-2)

`tokio::process::Command` on Windows internally calls `CreateProcessW`
with the parent's primary token. There is no stable Rust API to swap
the primary token of an already-spawned process (the primary token is
fixed at creation time). The only way to give the target a restricted
primary token is to call `CreateProcessAsUserW` ourselves.

Two ways to integrate this:

- **Direct in driver.rs**: bypass tokio Command, manually create
  HANDLE pipes, build `STARTUPINFOW`, call `CreateProcessAsUserW`,
  manually wait via `WaitForSingleObject`, manually read stdio HANDLEs
  into Rust buffers. ~500 lines of unsafe Win32 in the driver's hot
  path.
- **Init subcommand**: driver uses tokio Command normally (clean
  stdio, clean async wait); the spawned `aleph-server sandbox-init-windows`
  process does the unsafe Win32 in one isolated file. The target
  inherits init's stdio (which is tokio's pipes), so output flows
  through automatically.

Both produce the same security outcome (target runs under restricted
token). The init pattern wins on:

- Mirrors SP-2 (one file for OS-specific kernel/Win32 calls, driver
  stays simple).
- Cross-platform parts (policy struct, JSON shape) compile + test on
  macOS dev box.
- No HANDLE bookkeeping in the async driver — Drop semantics on Win32
  HANDLEs interact poorly with tokio runtimes.

Cost: one extra `aleph-server` process per sandboxed execution.
Negligible (subprocess startup ~10ms on a warm system).

### 2.2 Invocation chain

Today's Windows path:

```
WindowsSandboxDriver::run
  → tokio Command::new(program).args(args)
  → spawn → JobObject::assign(child)
  → wait_with_output
```

Post-SP-3a:

```
WindowsSandboxDriver::run
  → tokio Command::new(aleph_server_exe)
      .args(["sandbox-init-windows", "--policy", json, "--", program, args...])
  → spawn → JobObject::assign(child)
  → wait_with_output

(inside the init child)
  → OpenProcessToken / CreateRestrictedToken / SetTokenInformation(IL=Low)
  → CreateProcessAsUserW(target) with inherited stdio
  → on ERROR_PRIVILEGE_NOT_HELD: fall back to CreateProcessW (soft-degrade)
  → WaitForSingleObject + GetExitCodeProcess
  → ExitProcess(target_exit_code)
```

JobObject containment continues to apply to the entire chain (init +
target are both inside the same job), so memory/process-count limits
and kill-on-close still work.

### 2.3 Restricted token construction

```c
HANDLE host_token;
OpenProcessToken(
    GetCurrentProcess(),
    TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
    &host_token
);

HANDLE restricted_token;
CreateRestrictedToken(
    host_token,
    DISABLE_MAX_PRIVILEGE,   // strips every privilege except SeChangeNotify
    0, NULL,                  // SidsToDisable: rely on DISABLE_MAX_PRIVILEGE-related flags only
    0, NULL,                  // PrivilegesToDelete: same
    0, NULL,                  // SidsToRestrict: none (Chrome adds Logon SID here; we keep simple)
    &restricted_token
);

// Set integrity level to Low (S-1-16-4096)
PSID low_sid;
ConvertStringSidToSidW(L"S-1-16-4096", &low_sid);
TOKEN_MANDATORY_LABEL tml;
tml.Label.Sid = low_sid;
tml.Label.Attributes = SE_GROUP_INTEGRITY;
SetTokenInformation(
    restricted_token,
    TokenIntegrityLevel,
    &tml,
    sizeof(TOKEN_MANDATORY_LABEL) + GetLengthSid(low_sid)
);
LocalFree(low_sid);
```

### 2.4 CreateProcessAsUserW invocation

```c
WCHAR cmd_line[CMD_LINE_MAX];  // built from target + args, properly quoted
STARTUPINFOW si = { sizeof(STARTUPINFOW) };
si.dwFlags = STARTF_USESTDHANDLES;
si.hStdInput  = GetStdHandle(STD_INPUT_HANDLE);
si.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
si.hStdError  = GetStdHandle(STD_ERROR_HANDLE);
PROCESS_INFORMATION pi = {0};

BOOL ok = CreateProcessAsUserW(
    restricted_token,
    target_wide,           // application name (NULL → parse from cmd_line)
    cmd_line,              // command line (mutable; CreateProcess writes into it)
    NULL, NULL,            // no security attrs
    TRUE,                  // bInheritHandles — required for stdio pass-through
    0,                     // no creation flags
    NULL,                  // inherit env
    NULL,                  // inherit cwd
    &si,
    &pi
);

if (!ok) {
    DWORD err = GetLastError();
    if (err == ERROR_PRIVILEGE_NOT_HELD /* 1314 */) {
        // Soft-degrade. Fall back to plain CreateProcessW with host token.
        tracing::warn!("...");
        ok = CreateProcessW(target_wide, cmd_line, ..., &si, &pi);
    }
    if (!ok) ExitProcess(67);
}

WaitForSingleObject(pi.hProcess, INFINITE);
DWORD code;
GetExitCodeProcess(pi.hProcess, &code);
CloseHandle(pi.hThread);
CloseHandle(pi.hProcess);
CloseHandle(restricted_token);
CloseHandle(host_token);
ExitProcess(code);
```

### 2.5 Policy payload

```rust
struct WindowsInitPolicy {
    /// When true, refuse to spawn if CreateProcessAsUserW returns
    /// ERROR_PRIVILEGE_NOT_HELD. Default false → soft-degrade to
    /// plain CreateProcessW + JobObject (cycle 1 behavior).
    require_restricted_token: bool,
}
```

The policy is intentionally tiny for SP-3a — there's no per-call
configuration knob worth exposing. Future cycles (SP-3b WFP, SP-6
AppContainer) will extend this struct.

## 3. File-level changes

| File | Change | Approx LOC |
|---|---|---|
| `src/sandbox/windows_init.rs` *(new)* | `WindowsInitPolicy` struct + JSON round-trip + 4 unit tests (cross-platform). `run_init` entry point + `apply_restricted_token` + `spawn_target` (Windows-only `#[cfg]`-gated). On non-Windows, `run_init` prints "unsupported" and exits 78. | ~280 |
| `src/sandbox/mod.rs` | `pub mod windows_init;` | +1 |
| `src/sandbox/driver.rs` | Add `windows_init_policy: Option<String>` to `OsSandboxProfile` (parallel to `linux_init_policy`). | +6 |
| 5 other `OsSandboxProfile` constructors | Add `windows_init_policy: None`. | +5 |
| `src/sandbox/platforms/windows/driver.rs` | `profile_for`: serialize `WindowsInitPolicy` to JSON; populate `windows_init_policy`. `run`: build command line as `aleph-server sandbox-init-windows --policy <json> -- <program> <args>` invoked via existing tokio Command path. | +30 |
| `src/sandbox/config.rs` | Add `WindowsSandboxConfig.require_restricted_token: bool` (default `false`). | +5 |
| `src/sandbox/platforms/mod.rs` | Thread the new config field into a `WindowsSandboxOptions` (mirrors `LinuxSandboxOptions`). | +5 |
| `src/bin/aleph-server/cli.rs` | Add hidden `SandboxInitWindows { args: Vec<String> }` subcommand. | +6 |
| `src/bin/aleph-server/main.rs` | Sync dispatcher arm that calls `windows_init::run_init`. | +3 |
| `docs/reference/SANDBOX.md` | New "Windows defense surface" subsection update + deferred table. | +25 |

Net: ~370 LOC. Zero new crates (`windows-sys 0.59` is already pulled).

## 4. `windows-sys` features needed

Already in `Cargo.toml` are: `Win32_Foundation`, `Win32_Security`,
`Win32_Security_Authorization`, `Win32_System_Threading`,
`Win32_System_JobObjects`, `Win32_Storage_FileSystem`,
`Win32_System_Console`, `Win32_System_Pipes`.

SP-3a uses APIs from the existing feature set:
- `OpenProcessToken`, `CreateRestrictedToken`, `SetTokenInformation`,
  `GetTokenInformation`: `Win32_Security`.
- `ConvertStringSidToSidW`, `LocalFree`: `Win32_Security_Authorization`.
- `CreateProcessAsUserW`, `CreateProcessW`, `WaitForSingleObject`,
  `GetExitCodeProcess`, `STARTUPINFOW`, `PROCESS_INFORMATION`:
  `Win32_System_Threading`.
- `GetStdHandle`: `Win32_System_Console`.
- `CloseHandle`: `Win32_Foundation`.

**No `Cargo.toml` changes needed.**

## 5. Error handling

All failures inside `windows_init::run_init` log to stderr and exit with
a numeric code. The driver surfaces these via `SandboxOutput.exit_code`.

| Failure | Exit code | Behavior |
|---|---|---|
| Argv parse failure (bad JSON, missing `--`, etc.) | 66 | stderr msg + exit |
| `OpenProcessToken` failure | 65 | stderr msg + exit (unrecoverable) |
| `CreateRestrictedToken` failure | 65 | same |
| `SetTokenInformation` (Low IL) failure | 65 | same |
| `CreateProcessAsUserW` → `ERROR_PRIVILEGE_NOT_HELD` (1314) | n/a | warn + retry with `CreateProcessW` (host token, Medium IL, but still JobObject); same exit code as target |
| `CreateProcessAsUserW` → other error AND `require_restricted_token=true` | 64 | stderr msg + exit |
| `CreateProcessAsUserW` → other error AND `require_restricted_token=false` | n/a | warn + retry with `CreateProcessW` |
| All spawn paths fail | 67 | stderr msg + exit |
| `WaitForSingleObject` / `GetExitCodeProcess` failure | 65 | same |

JobObject containment continues to apply regardless of token path. So
even the worst case (init plus target spawned with host token) still
benefits from cycle 1's memory cap + active-process limit.

## 6. Testing

### Unit tests (cross-platform — JSON / policy logic only)

- `windows_init_policy_round_trips_through_json`.
- `windows_init_policy_default_does_not_require_restricted_token`.
- `parse_init_args_extracts_policy_and_target` (mirrors SP-2).
- `parse_init_args_rejects_missing_policy / missing_target / bad_json`.

### Windows CI integration tests (`#[cfg(target_os = "windows")] + #[ignore]`)

- `restricted_token_drops_se_assignprimarytoken` — sandbox a target that
  calls `LookupPrivilegeValueW` + `PrivilegeCheck` for
  `SeAssignPrimaryTokenPrivilege`; assert "not held". (Requires writing
  a small Rust test binary; gated `#[ignore]`.)
- `low_integrity_is_applied` — sandbox a target that calls
  `GetTokenInformation(.., TokenIntegrityLevel, ..)`; assert
  `S-1-16-4096`.
- `soft_degrade_on_privilege_not_held` — programmatically remove
  `SE_INCREASE_QUOTA` from the test runner's token (impractical to do
  in CI; this test stays as a manual/in-repo runbook rather than CI
  automation).

### What we don't test on macOS dev box

- The actual `CreateProcessAsUserW` path. Cargo check compiles the
  `#[cfg(target_os = "windows")]` block on a Windows runner only.
- Soft-degrade behavior — requires a Windows env with the privilege
  removed.

This is explicitly accepted: we ship the code, lean on CI for Windows
verification, document the gap.

## 7. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| Author cannot test locally → bugs land before CI catches them | High | Code structured to minimize unsafe surface; soft-degrade keeps cycle 1 behavior on any unexpected error path; CI runs `cargo check` on Windows on every PR. |
| `CreateProcessAsUserW` returns `ERROR_PRIVILEGE_NOT_HELD` on common dev/server setups | Med | Documented soft-degrade. `require_restricted_token=true` lets ops decide. |
| Inherited stdio HANDLEs break Job Object association | Low | JobObject is assigned to the init process (cycle 1's existing wiring); target inherits job membership via parent-child relation. |
| `CreateRestrictedToken` removes a privilege some target binary actually needs (e.g. `SeBackupPrivilege` for archivers) | Med | `DISABLE_MAX_PRIVILEGE` is the documented "remove everything except SeChangeNotify" flag — same posture Chrome/Edge sandbox uses. Targets requiring privileges shouldn't run sandboxed. |
| Low Integrity prevents target from writing to its workspace cwd | Med | Workspace dir is bind-mounted by cycle 1's path; we'll need an explicit DACL grant (followup, not SP-3a). For now, document. |
| Init process exits before target completes (race) | Low | `WaitForSingleObject(pi.hProcess, INFINITE)` blocks; init can't exit early. |
| HANDLE leak in error paths | Low | All HANDLEs `CloseHandle`d before exit; init process exiting collapses any leak anyway. |
| `windows-sys` feature gap surfaces only on Windows compile | Low | Pre-cycle-1 audit already verified the listed features cover SP-3a's API set; CI build catches anything missed. |

## 8. Alignment with redlines & principles

- **R3 (core minimalism)**: zero new crates; uses already-present
  `windows-sys 0.59` features. ✓
- **R10 (thin harness)**: `windows_init.rs` is a single-purpose file;
  no traits, no token-management framework, just the one entry point. ✓
- **P5 (least knowledge)**: driver knows only "policy JSON + target +
  args"; init knows the Win32 details. ✓
- **P7 (defensive design)**: every failure either soft-degrades or
  exits with a documented code; `require_restricted_token=true` lets
  operators escalate. ✓

## 9. Implementation sequence

1. `src/sandbox/windows_init.rs` (new):
   - `WindowsInitPolicy` struct + serde + 4 cross-platform unit tests.
   - `#[cfg(target_os = "windows")] pub fn run_init(args: Vec<String>) -> !`.
   - `#[cfg(not(target_os = "windows"))] pub fn run_init(_) -> ! { exit(78); }`.
   - Helpers: `parse_init_args` (cross-platform), `apply_restricted_token`, `spawn_target_with_token` (Windows-only).
2. `src/sandbox/mod.rs` — `pub mod windows_init;`.
3. `src/sandbox/driver.rs` — add `windows_init_policy` field.
4. Update 5 `OsSandboxProfile` constructors (factory.rs, seatbelt.rs, bwrap.rs, workspace.rs mock, sandbox_capability_approval.rs RecordingDriver).
5. `src/sandbox/config.rs` — add `WindowsSandboxConfig.require_restricted_token`.
6. `src/sandbox/platforms/mod.rs` — thread it into a new `WindowsSandboxOptions` (or extend the existing if any).
7. `src/sandbox/platforms/windows/driver.rs`:
   - `profile_for`: serialize `WindowsInitPolicy` to JSON; populate `windows_init_policy`.
   - `run`: rebuild command line as `aleph-server sandbox-init-windows --policy <json> -- <program> <args>`; pass through existing tokio + JobObject path.
8. `src/bin/aleph-server/cli.rs` — `SandboxInitWindows { args }` hidden subcommand.
9. `src/bin/aleph-server/main.rs` — synchronous dispatcher arm.
10. `cargo check -p alephcore` (macOS — only cross-platform parts compile; Windows-only paths gated out).
11. `cargo test -p alephcore --lib sandbox` (regression check; +4 from SP-3a JSON tests).
12. `cargo test -p alephcore --test sandbox_capability_approval` (regression check).
13. `cargo clippy -p alephcore --lib --tests -- -D warnings` (only assess touched-file regressions).
14. Update `docs/reference/SANDBOX.md` "Current Windows defense surface" section.
15. Commit: `sandbox: SP-3a — Windows restricted token + Low integrity level`.

## 10. Out-of-scope follow-up specs

- **SP-3b**: WFP per-host network filtering (admin-only — likely deferred indefinitely; SP-6 supersedes for most purposes).
- **SP-6**: AppContainer — modern Windows sandbox primitive, requires `windows-sys` 0.61+ upgrade. SP-3b vs SP-6 decision pending.
- Workspace DACL grants for Low-IL processes (separate small cycle).
