# Desktop Windows Parity & Build Fix — Design

**Date:** 2026-05-21
**Branch:** `worktree-desktop-windows-parity`
**Reference comparison:** `/Volumes/TBU4/Github/openclaw`

## Background

OpenClaw's desktop / computer-use capability is a **broker/delegation model**:
it hosts a PeekabooBridge socket for the external `peekaboo` CLI, delegates to
Codex's `computer-use` MCP plugin, or registers TryCua's `cua-driver` MCP. All
three are macOS-only. OpenClaw has **no native Windows or Linux desktop
implementation**.

Aleph already exceeds this architecturally — it owns a real cross-platform
`DesktopCapability` trait system with native macOS / Linux / Windows backends.
The way to *surpass* openclaw is therefore to **finish and fix the native
Windows path**, which is currently both broken and incomplete.

## Problem

### P0 — `aleph-desktop-windows` does not compile on Windows

`aleph-desktop-windows` is a `[target.'cfg(target_os = "windows")'.dependencies]`
entry of the top-level `alephcore` crate. Cross-compiling
(`cargo check --target x86_64-pc-windows-gnu`) shows the crate fails with **20
compile errors** — meaning **the entire Aleph project cannot build on Windows
today**. `aleph-desktop` (the shared crate) compiles clean; all errors are in
`aleph-desktop-windows`.

Root causes (all from `windows` crate 0.58 API drift — the code was merged
without ever being cross-compile-checked):

- **`escape_listener.rs`** — hook symbols (`SetWindowsHookExW`, `WH_KEYBOARD_LL`,
  `UnhookWindowsHookEx`, `CallNextHookEx`, `KBDLLHOOKSTRUCT`, `WM_KEYDOWN`)
  imported from `Win32::UI::Input::KeyboardAndMouse` but they live in
  `Win32::UI::WindowsAndMessaging`; `WPARAM/LPARAM/LRESULT` not in scope at the
  callback signature; `GetModuleHandleW` returns `HMODULE` but `HINSTANCE` is
  expected; `HHOOK` (`*mut c_void`) and `*const WindowsEscapeListener` are not
  `Send`, so `Mutex<Option<HHOOK>>` and `static LISTENER_PTR` break `Send/Sync`,
  which in turn breaks `impl DesktopPlatform for WindowsPlatform`.
- **`system.rs`** — `GetTickCount` imported from `Win32::Foundation` (it is in
  `Win32::System::SystemInformation`); `enum_proc` declared `-> i32` but the
  `WNDENUMPROC` callback type returns `BOOL`; `list_running_apps`'s `enum_proc`
  references `LPARAM` without importing it; `get_clipboard::<String>(...)` passes
  one of two required generic args; `GetComputerNameExW` API shape changed.

### P1 — the `desktop` tool is non-functional on Windows

The `desktop` builtin tool routes every action through `ScreenCapability`. All
three platform crates embed the shared `NativeScreen`, which delegates to the
`action::*` / `perception::*` functions in `desktop/shared`. Four `action`
functions are `NotImplemented` on Windows, so even after P0 is fixed these
actions silently fail:

| Action surface       | Windows state    | Source                      |
|----------------------|------------------|-----------------------------|
| `clipboard_read`     | `NotImplemented` | `action/input.rs:301`       |
| `clipboard_write`    | `NotImplemented` | `action/input.rs:365`       |
| `paste`              | broken (uses `clipboard_write`) | (composite)  |
| `window_list`        | `NotImplemented` | `action/window.rs:31`       |
| `focus_window`       | `NotImplemented` | `action/window.rs:67`       |
| `quit_app`           | `NotImplemented` | `action/app_launch.rs:147`  |

The know-how exists in-repo (`desktop/windows/src/system.rs` has working Win32
clipboard via the already-declared `clipboard-win` dep, and window enumeration).
This is a missing-wiring gap.

### P2 — `WindowsSystem::quit_app` matches the wrong windows

`desktop/windows/src/system.rs:72` matches windows by **title substring** —
`quit_app("Word")` also closes "Password Manager". The code comment already
flags this as an open bug.

### P3 — two minor error-swallowing sites

- `src/builtin_tools/desktop/mod.rs:104` — `let _ = listener.start()` drops the
  escape-listener init failure; the abort hot-key then never works, invisibly.
- `src/builtin_tools/desktop/native.rs` paste restore — two
  `let _ = screen.clipboard_write(...)` drop clipboard-restore failures.

### P4 — orphan directory

`crates/desktop-macos/` holds only stale Swift-PM `.build` artifacts. It is not a
Cargo workspace member and is referenced nowhere.

## Scope

Bug-fixes, wiring, and cleanup only. No destructive refactoring. No speculative
new capabilities. Verification uses `cargo check --target x86_64-pc-windows-gnu`,
which type-checks every `#[cfg(target_os = "windows")]` block.

### Phase 1 — make `aleph-desktop-windows` compile on Windows (P0)

- **`escape_listener.rs`**
  - Import the hook symbols from `Win32::UI::WindowsAndMessaging`; keep
    `VK_ESCAPE` from `Win32::UI::Input::KeyboardAndMouse`.
  - Add module-level `#[cfg(windows)]` imports for `WPARAM/LPARAM/LRESULT` so the
    `keyboard_hook_proc` signature resolves.
  - Convert `GetModuleHandleW`'s `HMODULE` to `HINSTANCE` for `SetWindowsHookExW`.
  - Make the listener `Send + Sync`: store the hook handle and the global
    listener pointer as integer addresses (`Mutex<Option<isize>>` for the hook,
    `AtomicUsize` for `LISTENER_PTR`) instead of raw-pointer newtypes. This is
    the minimal change that keeps the existing architecture intact.
- **`system.rs`**
  - Import `GetTickCount` from `Win32::System::SystemInformation`.
  - Fix `list_running_apps`'s `enum_proc`: import `LPARAM`, return `BOOL`.
  - `get_clipboard::<String>(...)` → `get_clipboard::<String, _>(...)`.
  - Rewrite the `GetComputerNameExW` call for the 0.58 API (`PWSTR` buffer,
    `Result` return).
  - `quit_app` is rewritten in Phase 2/A5 (its broken `enum_proc` is deleted).

### Phase 2 — Windows `NativeScreen` parity (P1, P2)

All Win32 code is `#[cfg(target_os = "windows")]`, mirroring the existing macOS /
Linux arms. `desktop/shared` already declares the `windows` crate with
`Win32_UI_WindowsAndMessaging`, `Win32_Foundation`, `Win32_System_Threading` —
sufficient for A2–A4. A1 adds `clipboard-win` (already used by `desktop/windows`).

- **A1** `action::clipboard_read` / `clipboard_write` (`action/input.rs`) — Windows
  arm via `clipboard-win`; add `clipboard-win = "5"` to `desktop/shared/Cargo.toml`
  windows deps. Fixes `clipboard_read`, `clipboard_write`, `paste`.
- **A2** `action::window_list` (`action/window.rs`) — Windows arm via `EnumWindows`
  + `GetWindowTextW` + `IsWindowVisible` + `GetWindowThreadProcessId`;
  `WindowInfo.id` = `HWND` address.
- **A3** `action::focus_window` (`action/window.rs`) — Windows arm: `IsWindow`
  guard, `ShowWindow(SW_RESTORE)` if minimized, `SetForegroundWindow`.
- **A4** `action::quit_app` (`action/app_launch.rs`) — Windows arm with **correct
  process-executable-name matching** (not title substring): `EnumWindows` →
  `GetWindowThreadProcessId` → `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` +
  `QueryFullProcessImageNameW` → `PostMessageW(WM_CLOSE)` to matching windows.
  The basename predicate is a pure `exe_name_matches` helper, unit-tested on any OS.
- **A5** `WindowsSystem::quit_app` (`desktop/windows/src/system.rs`) — replace the
  buggy substring implementation with a one-line delegation to
  `aleph_desktop::action::quit_app`. Fixes P2 and removes the duplicate (and
  P0-broken) `enum_proc`.

### Phase 3 — robustness & cleanup (P3, P4)

- **B1** `mod.rs` escape-listener start — `let _ =` → `if let Err(e) { warn!(...) }`.
- **B2** `native.rs` paste restore — two `let _ =` → `if let Err(e) { warn!(...) }`.
- **C1** remove `crates/desktop-macos/`.

## Out of scope (deferred)

- Linux desktop — `aleph-desktop-linux` cannot be cross-checked on this host
  (no Linux std target / C sysroot); a separate cycle on a Linux host.
- `agent_id` / `context` audit plumbing (`mod.rs:193`) — cross-cutting, separate cycle.
- `desktop_ax_*` in `core_tools.rs` — live discovery (`BUILTIN_TOOL_DEFINITIONS`)
  already lists them; the dispatcher `tool_index` is slated for R10 dissolution.
- Peekaboo-style new gestures (`swipe`, `menu list`) — YAGNI / R3.

## Testing

- Pure unit test for `exe_name_matches` (runs on any OS).
- Existing `WindowsSystem` and `src/builtin_tools/desktop/tests.rs` suites stay green.
- Compile verification:
  - `cargo check --target x86_64-pc-windows-gnu -p aleph-desktop -p aleph-desktop-windows`
    — must go from 20 errors to 0.
  - macOS: `cargo check -p aleph-desktop -p aleph-desktop-windows -p alephcore`
    + `cargo test` for the desktop tool suite.

## Risk

Low–medium. Phase 1 is mechanical API-drift repair, verified by cross-compilation.
Phase 2 is additive (`NotImplemented` arms filled) plus one delegation. Phase 3 is
error-logging and a dead-directory removal. No public trait/API surface changes.
The Win32 enumeration paths cannot be runtime-tested on the macOS dev host;
they are covered by type-checking the Windows target and by the pure-helper test.
