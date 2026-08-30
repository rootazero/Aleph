# Module: desktop-platforms (review 2026-08-29)

## Summary

- **Files audited**: 46 `.rs` files + 3 `Cargo.toml`
  - `desktop/linux/src/**/*.rs` (13 files, ~3 400 LOC)
  - `desktop/macos/src/**/*.rs` + `tests/**/*.rs` (14 + 6 files, ~5 400 LOC)
  - `desktop/windows/src/**/*.rs` + `tests/**/*.rs` (11 + 3 files, ~6 800 LOC)
- **Total LOC**: ~15 600
- **Issues**: 5 total (0 critical / 1 high / 3 medium / 1 low)
- **R1 platform-API isolation**: PASS — all native API calls are inside the platform crates, and each crate gates its platform-only deps under `#[cfg(target_os = ...)]`.
- **Wiring completeness**: PASS — every required trait method on `desktop/shared` traits is implemented on all three platforms (most via `NativeScreen` or the platform-specific `Screen` impl).
- **Production `unwrap`/`expect`/`panic`**: PASS after fixes — only one production `expect` was found and it has been replaced with an error path.

## High-Confidence Issues

### [High] `desktop/linux/src/ax/mod.rs:245` — `expect` in production resolve path

- **Location**: `desktop/linux/src/ax/mod.rs`, inside `AccessibilityCapability::resolve`.
- **Description**: After ranking AT-SPI locator candidates, the code does:
  ```rust
  let proxy = candidates
      .into_iter()
      .nth(index)
      .expect("rank_candidates returns an in-range index")
      .proxy;
  ```
  This is an unconditional panic surface in production code. The contract with `rank_candidates` is not enforced by the type system; a drift in the ranker, an empty candidate list that still returns `Some(0)`, or a future change can turn a malformed request into a process crash.
- **Risk / trigger**: Any `ax.set_value` or `ax.perform_action` call on Linux where the candidate list and the returned index disagree.
- **Suggested fix**: Replace the `expect` with a `DesktopError::PlatformError` return.
- **Decision**: FIXED — replaced with an `ok_or_else` error return.

### [Medium] `desktop/linux/src/automation.rs` — `run_background` retries PowerShell on any error

- **Location**: `desktop/linux/src/automation.rs`, `AutomationCapability::run_background`, `ScriptLanguage::PowerShell` arm.
- **Description**: The `pwsh → powershell` fallback treats *every* error from `spawn_background` as "binary missing" and tries the next interpreter. This is inconsistent with the synchronous `run_script` path, which only falls back on a genuine spawn failure. A script that started under `pwsh` but failed to run (e.g., syntax error, missing module) will be silently re-executed under `powershell`, producing two child attempts and a confusing error from the wrong interpreter.
- **Risk / trigger**: A malformed or environment-dependent PowerShell background command.
- **Suggested fix**: Gate the fallback on `is_spawn_failure(&e)` and propagate non-spawn errors immediately.
- **Decision**: FIXED — now mirrors the `run_script` fallback logic.

### [Medium] `desktop/macos/src/system/mod.rs` — synchronous AppKit calls run on the async runtime thread

- **Location**: `MacOSSystem::{launch_app, quit_app, list_running_apps, clipboard_read, clipboard_write}`.
- **Description**: These `async` trait methods were calling synchronous AppKit/NSPasteboard code (`workspace::launch_app`, `workspace::quit_app`, `workspace::list_running_apps`, `aleph_desktop::macos::clipboard::read`, `aleph_desktop::macos::clipboard::write_text`) directly on the tokio worker thread. The other macOS system methods and the equivalent Linux/Windows methods already wrap such work in `tokio::task::spawn_blocking`. Blocking or main-thread-only Cocoa work on an async worker risks stalling the runtime and, for NSPasteboard/NSWorkspace, undefined behavior when the main run loop is not driven.
- **Risk / trigger**: Concurrent system calls on macOS; clipboard operations are especially sensitive because NSPasteboard is documented as main-thread-only.
- **Suggested fix**: Move all synchronous native calls in `MacOSSystem` into `spawn_blocking`, matching `list_installed_apps`, `system_info`, and the Windows/Linux system impls.
- **Decision**: FIXED — `launch_app`, `quit_app`, `list_running_apps`, `clipboard_read`, and `clipboard_write` now all use `spawn_blocking`. (A full main-thread dispatch would require a running `NSRunLoop`/`CFRunLoop` on the daemon's main thread, which is out of scope for this round.)

### [Medium] `desktop/linux/src/sleep_inhibitor.rs` — `inhibit_sleep` blocks the caller thread

- **Location**: `LinuxPower::inhibit_sleep` and `spawn_inhibitor`.
- **Description**: `PowerCapability::inhibit_sleep` is a synchronous method. The implementation polls the inhibitor child for up to `SETTLE_WINDOW` (400 ms) using `std::thread::sleep` on the calling thread. When the caller is an async task, this blocks a tokio worker for the whole settle window. The Windows and macOS inhibitors do not have this property because they are handle-based and return immediately.
- **Risk / trigger**: Every sleep-inhibit request on Linux stalls one async worker for 0–400 ms.
- **Suggested fix**: Spawn the settle polling on a dedicated std thread and return the guard immediately once the child is confirmed alive, or make the trait method async. Either change touches the `PowerCapability` contract, so it needs its own focused PR.
- **Decision**: DEFERRED — the behavior is bounded and documented; changing the trait shape is larger than a review-round fix.

### [Low] `desktop/windows/src/pim.rs` — DASL `LIKE` wildcards are not escaped in `restrict_query`

- **Location**: `desktop/windows/src/pim.rs`, `restrict_query`.
- **Description**: The Outlook `Items.Restrict` DASL query uses `LIKE '%{q}%'`. Only the single quote is escaped; DASL `LIKE` also treats `%` and `_` as wildcards. A query containing those characters will match more messages than the literal fallback scan, so the restricted path and the fallback scan can return different result sets for the same call.
- **Risk / trigger**: User query containing `%` or `_` while Outlook is reachable.
- **Suggested fix**: Escape `%` → `[%]` and `_` → `[_]` in `escape_dasl`, or switch to an exact-match comparison when the query contains wildcards.
- **Decision**: REPORTED — functional inconsistency, not a crash or injection; no live call site is known to depend on wildcard semantics.

## Per-perspective findings

### Security

- **Command injection surfaces are constrained to the trait contract**: `AutomationCapability::run_script` intentionally executes caller-supplied script code. All three platforms pass the code as a single argument to the interpreter (`bash -c`, `cmd /C`, `osascript -e`, `powershell -Command`), so the caller cannot break out of the script argument into a new shell token *unless* the script language itself is exploited. There is no additional unvalidated interpolation point on the platform side.
- **Windows shortcuts**: `run_shortcut` passes name and input via environment variables, not command-line concatenation. The script itself is a compile-time constant. This removes the PowerShell argument-injection vector the comment describes.
- **Windows PIM**: `ps_escape_dq` + `escape_powershell_wildcards` correctly neutralize `"`, `$`, `` ` ``, `[`, `*`, `?` in the double-quoted parts of the generated script. The DASL wildcard issue above is a semantics drift, not an injection.
- **macOS notifications**: `escape_applescript` handles `\`, `"`, `\n`, `\r`; no string-literal breakout is possible with those controls.
- **Path traversal**: `LinuxPim` only reads from `~/.thunderbird/Mail`; folder IDs are compared as strings and never used as filesystem paths. `WindowsPim` and `MacOSPim` mail IDs go through PowerShell/Swift helpers with no path arithmetic on the Rust side.
- **COM init/uninit on Windows**: `ComGuard` in `windows/src/ax.rs` correctly pairs `CoInitializeEx`/`CoUninitialize` and skips uninit when `RPC_E_CHANGED_MODE` is returned. `WindowsSystem::launch_app` uses a local `ComExit` RAII guard for its STA requirement. No imbalance found.
- **Subprocess exit codes**: All three platforms check `output.status.success()` after shell-outs and surface stderr. `WindowsMedia::list_dshow_devices` deliberately ignores the expected non-zero exit of the dummy-device probe.

### Logic

- **State machines**: macOS permission mappers are exhaustive over `AVAuthorizationStatus`, `SFSpeechRecognizerAuthorizationStatus`, and `UNAuthorizationStatus`. Windows maps `Allow/Deny` plus the `ToastEnabled` switch. Linux reports `Granted/Unknown` based on session type and device groups, with no false `Denied`. All three match the shared `PermissionStatus` enum.
- **Error propagation**: Error paths consistently clean up handles (e.g., `CloseHandle` after failed `PowerSetRequest`, `CFRelease` on failed `CFMachPortCreateRunLoopSource`, temp-file deletion after failed captures on Linux/Windows).
- **Escape-listener lifetimes**: Windows unhooks before posting `WM_QUIT`, clears the global pointer before freeing state, and joins the worker. macOS disables the tap and removes the source before releasing the port and the context. No use-after-free path was found.
- **D-Bus/AT-SPI return handling**: `linux/src/ax/bus.rs` retries with a fresh connection when the shared AT-SPI bus appears dead; `invalidate_shared` is called on registry/listing failures. The Windows UIA gate serializes concurrent COM work to avoid `E_FAIL` races.
- **Threading**: macOS `EscapeListener` runs its `CFRunLoop` on a dedicated thread, satisfying the main-run-loop requirement without blocking tokio. Windows `WindowsEscapeListener` runs a dedicated Win32 message-loop thread. Linux `LinuxEscapeListener` is filesystem-based and lock-free.
- **Targeted input contract**: macOS `MacOSScreen` refuses to report a targeted event as successful unless the Swift helper explicitly returns `delivery == "targeted"`. Windows and Linux inherit the `NotImplemented` default for targeted input, which is the honest answer.

### Architecture (R1–R10)

- **R1**: All platform API calls live in the platform crates. `NativeScreen` remains in `desktop/shared` but only as a cross-platform wrapper; no core code calls platform APIs.
- **R2**: No business UI in any platform crate. macOS PIM goes through the Swift bridge; everything else is pure I/O or mappings.
- **R3**: No heavy new dependencies introduced; platform crates use their target-gated native bindings (`windows`, `objc2`, `atspi`) as expected.
- **R5**: Notifications, input, and escape paths are designed not to steal focus or swallow keys unless explicitly requested.
- **R7/R8**: No regex-based intent detection; role/action mappings are static tables derived from protocol constants.
- **R9/R10**: Configuration is exposed through the shared trait surface; no extra middleware layer.

### Wiring completeness

Every method required by `desktop/shared/src/traits/*.rs` is implemented or intentionally defaulted on each platform:

| Trait | Linux | macOS | Windows |
|-------|-------|-------|---------|
| `AutomationCapability` | `LinuxAutomation` | `MacOSAutomation` | `WindowsAutomation` |
| `MediaCapability` | `LinuxMedia` | `MacOSPlatform` impl | `WindowsMedia` |
| `PermissionCapability` | `LinuxPermission` | `MacOSPermission` | `WindowsPermission` |
| `PimCapability` | `LinuxPim` (mail only) | `MacOSPim` (full) | `WindowsPim` (mail only) |
| `PowerCapability` | `LinuxPower` | `MacosPower` | `WindowsPower` |
| `ScreenCapability` | `NativeScreen` | `MacOSScreen` / `NativeScreen` | `NativeScreen` |
| `SystemCapability` | `LinuxSystem` | `MacOSSystem` | `WindowsSystem` |
| `AccessibilityCapability` | `LinuxAccessibility` | `BridgeAccessibility` | `WindowsAccessibility` |
| `EscapeAbort` | `LinuxEscapeListener` | `EscapeListener` | `WindowsEscapeListener` |

`ScreenCapability::screenshot_window` is intentionally left as the trait default (`NotImplemented`) on Linux/Windows where `NativeScreen` has no backend and on macOS where the Swift bridge provides it. `PimCapability` non-mail methods default to `NotImplemented` on Linux and Windows, which is the documented platform gap.

### Quality / coverage gaps

- `desktop/linux/src/ax/mod.rs` has a live AT-SPI smoke test (`live_tree_speaks_the_ax_role_vocabulary`) that guards the cross-platform role vocabulary contract.
- `desktop/windows/src/ax.rs` and `desktop/linux/src/ax/roles.rs` both duplicate a copy of the consumer-side `INTERACTABLE_ROLES` list in unit tests so the two sides cannot drift silently.
- **Suggested tests to add**:
  1. Linux `ax/mod.rs`: a unit test that feeds a bogus `rank_candidates` result and asserts `PlatformError` instead of panic.
  2. Linux `automation.rs`: a test that a non-spawn PowerShell background failure is propagated without trying the fallback interpreter.
  3. macOS `system/mod.rs`: a test asserting that `clipboard_read/write` and `list_running_apps` complete without blocking the runtime (e.g., schedule a timer that must fire within the call).
  4. Windows `pim.rs`: a test asserting that `restrict_query("a%b_c")` produces a DASL string that matches the literal fallback scan's semantics.

## Conclusion

The three desktop platform crates are in solid shape. The only production panic surface found was the single `expect` in the Linux AT-SPI resolver; it has been removed. The other fixes in this round remove an inconsistent PowerShell fallback retry and move synchronous macOS AppKit/clipboard work off the async runtime thread. The remaining medium finding (Linux sleep inhibitor blocking the caller) is bounded and documented; the low finding (Windows DASL wildcard semantics) is a correctness edge case with no known live trigger. No critical issues or missing trait implementations were found.

## What was not done

- No `cargo check` / `cargo test` / `clippy` was run per the task constraints.
- `desktop/shared` and `desktop/shell` were not modified.
- No full main-thread dispatch for macOS AppKit calls was introduced; the fix wraps them in `spawn_blocking`, which is the same pattern used elsewhere but does not guarantee main-thread execution if an API strictly requires it.
- The Linux sleep-inhibitor blocking behavior was reported but not changed.
- The Windows DASL wildcard behavior was reported but not changed.
