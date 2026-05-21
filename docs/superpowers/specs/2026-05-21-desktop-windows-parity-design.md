# Desktop Windows Parity — Design

**Date:** 2026-05-21
**Branch:** `worktree-desktop-windows-parity`
**Reference comparison:** `/Volumes/TBU4/Github/openclaw`

## Background

OpenClaw is a "super AI assistant". Its desktop / computer-use capability is a
**broker/delegation model**, not a native implementation:

- **PeekabooBridge host** — OpenClaw.app hosts a UNIX-socket broker so the
  external `peekaboo` CLI can reuse the app's macOS TCC grants. macOS-only.
- **Codex Computer Use** — delegates to Codex's native `computer-use` MCP plugin.
- **cua-driver MCP** — registers TryCua's upstream driver as a normal MCP server.
  macOS-only.

OpenClaw has **no native Windows or Linux desktop-control implementation**.

Aleph already exceeds this architecturally: it owns a real cross-platform
`DesktopCapability` trait system with native macOS / Linux / Windows backends
(`desktop/shared` + `desktop/{macos,linux,windows}`). The way to *surpass*
openclaw is therefore not to copy the broker pattern — it is to **finish the
native Windows path**, which is currently incomplete.

## Problem

The `desktop` builtin tool routes every action through `ScreenCapability`.
All three platform crates embed the shared `NativeScreen`, which delegates to
the synchronous `action::*` / `perception::*` functions in `desktop/shared`.

Four `action` functions are `NotImplemented` on Windows, so the `desktop` tool
**silently fails on Windows** for these actions:

| Action surface            | Windows state                              | Source                         |
|---------------------------|--------------------------------------------|--------------------------------|
| `clipboard_read`          | `NotImplemented`                           | `action/input.rs:301`          |
| `clipboard_write`         | `NotImplemented`                           | `action/input.rs:365`          |
| `paste`                   | broken (depends on `clipboard_write`)      | (composite)                    |
| `window_list`             | `NotImplemented`                           | `action/window.rs:31`          |
| `focus_window`            | `NotImplemented`                           | `action/window.rs:67`          |
| `quit_app`                | `NotImplemented`                           | `action/app_launch.rs:147`     |

The know-how already exists in-repo: `desktop/windows/src/system.rs` has working
Win32 clipboard (via the already-declared `clipboard-win` dep) and window
enumeration (`EnumWindows`). This is a *missing-wiring* gap, not a missing-design
gap.

Separately, `WindowsSystem::quit_app` (`desktop/windows/src/system.rs:72`)
matches windows by **title substring** — `quit_app("Word")` also closes
"Password Manager". The code comment acknowledges this as an open bug.

Two minor robustness issues swallow errors silently:

- `desktop/.../mod.rs:104` — `let _ = listener.start()` drops escape-listener
  init failure; the abort hot-key then never works and the failure is invisible.
- `desktop/.../native.rs` paste restore — `let _ = screen.clipboard_write(...)`
  drops clipboard-restore failure; the user's original clipboard is lost silently.

Cleanup: `crates/desktop-macos/` holds only stale Swift-PM `.build` artifacts.
It is not a Cargo workspace member and is referenced nowhere — dead.

## Scope

Bug-fixes, wiring, and cleanup only. No destructive refactoring. No speculative
new capabilities (Peekaboo gestures like `swipe` / `menu list` are explicitly
out of scope per YAGNI / R3).

### Part A — Windows `NativeScreen` parity

All Win32 code is gated by `#[cfg(target_os = "windows")]`, mirroring the
existing macOS / Linux arms in each function. The `windows` crate is already a
dependency of `desktop/shared` with the features `Win32_UI_WindowsAndMessaging`,
`Win32_Foundation`, `Win32_System_Threading` — sufficient for A2–A4. A1 needs
`clipboard-win` (already used by `desktop/windows`; add to `desktop/shared`).

- **A1 — `action::clipboard_read` / `clipboard_write`** (`action/input.rs`)
  Implement the Windows arm via `clipboard-win` (`get_clipboard`/`set_clipboard`,
  `formats::Unicode`). Add `clipboard-win = "5"` to the
  `[target.'cfg(target_os = "windows")'.dependencies]` of
  `desktop/shared/Cargo.toml`. Fixes `clipboard_read`, `clipboard_write`, and the
  `paste` action on Windows.

- **A2 — `action::window_list`** (`action/window.rs`)
  Implement the Windows arm via `EnumWindows` + `IsWindowVisible` +
  `GetWindowTextW` + `GetWindowThreadProcessId`. `WindowInfo.id` is set to the
  `HWND` value (`hwnd.0 as u64`) so `focus_window` can round-trip it.

- **A3 — `action::focus_window`** (`action/window.rs`)
  Implement the Windows arm: cast `window_id` back to `HWND`, call
  `ShowWindow(SW_RESTORE)` (un-minimize) then `SetForegroundWindow`.

- **A4 — `action::quit_app`** (`action/app_launch.rs`)
  Implement the Windows arm with **correct process-name matching** (not title
  substring): `EnumWindows` → per visible window `GetWindowThreadProcessId` →
  `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`
  → compare the executable basename case-insensitively, with an optional `.exe`
  suffix → `PostMessageW(WM_CLOSE)` to every matching window. The basename match
  predicate is extracted as a small pure helper (`exe_name_matches`) so it is
  unit-testable on any OS.

- **A5 — `WindowsSystem::quit_app`** (`desktop/windows/src/system.rs`)
  Replace the buggy title-substring implementation with a one-line delegation to
  `aleph_desktop::action::quit_app`. This fixes the bug and removes the duplicate
  implementation. `desktop/windows` already depends on `aleph-desktop`.

### Part B — Robustness

- **B1** — `mod.rs` escape-listener start: replace `let _ = listener.start()`
  with `if let Err(e) = listener.start() { warn!(...) }`. `escape_started` is
  still set afterwards so a hard-failed listener is logged once, not retried on
  every action (no log spam).

- **B2** — `native.rs` paste clipboard-restore: replace the two
  `let _ = screen.clipboard_write(...)` calls with `if let Err(e) = ...` that
  emits a `warn!`.

### Part C — Cleanup

- **C1** — `git rm -r crates/desktop-macos/`. Confirmed: not in
  `[workspace].members`, not referenced by any `Cargo.toml` path dependency.

## Out of scope (deferred)

- `agent_id` / `context` audit plumbing (`mod.rs:193-194`) — a cross-cutting
  change to the `AlephTool` call chain; higher risk, separate cycle.
- Linux `user_idle_seconds` — requires a new X11/DBus dependency for marginal
  value; not desktop-*control*.
- Registering the `desktop_ax_*` tools in `core_tools.rs` — the live discovery
  path (`BUILTIN_TOOL_DEFINITIONS`) already lists them; the dispatcher
  `tool_index` is slated for R10 dissolution, so adding entries there would feed
  soon-dead code.
- Peekaboo-style new gestures (`swipe`, `menu list`, motion smoothing) — YAGNI.

## Testing

- **A1–A4**: a pure unit test for the `exe_name_matches` helper (runs on any OS).
  The Win32 enumeration paths are environment-dependent; they are verified by
  cross-compilation type-checking rather than runtime assertions.
- **A5**: existing `WindowsSystem` tests remain green.
- **B1/B2**: existing `src/builtin_tools/desktop/tests.rs` suite remains green.
- **Compile verification**:
  - macOS: `cargo check -p aleph-desktop -p aleph-desktop-windows` (non-Windows arms).
  - Windows cross-check: `cargo check --target x86_64-pc-windows-gnu -p aleph-desktop -p aleph-desktop-windows` — type-checks every `#[cfg(target_os = "windows")]` block introduced here.

## Risk

Low. Every change is additive (filling a `NotImplemented` arm), a one-line
delegation, an error-logging upgrade, or a dead-directory removal. No public API
or trait surface changes. Cross-compilation type-checking covers the Windows code
paths that cannot be run on the macOS dev host.
