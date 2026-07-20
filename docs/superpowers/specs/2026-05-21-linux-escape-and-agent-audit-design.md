# Linux Desktop Escape Abort + agent_id Audit Pipeline — Design

**Date:** 2026-05-21
**Status:** Implemented
**Cycle:** Continuation of the desktop subsystem audit (follows
`2026-05-21-desktop-windows-parity-design.md`).

## Context

The Windows-parity cycle closed the desktop subsystem's P0 (un-buildable on
Windows) and P1 (un-wired Windows `NativeScreen` arms). Its deferred backlog
named three follow-ups. This cycle resolves two of them; the third is
explicitly dropped.

| Item | Decision |
|------|----------|
| Linux desktop | **Implement** — close the one real functional gap. |
| `agent_id` audit pipeline | **Implement** — wire existing infrastructure. |
| Peekaboo-style new gestures | **YAGNI** — not implemented. No requesting consumer; the existing click/drag/hover/scroll/key set already covers computer-use flows. |

## Item 1 — Linux Escape Abort

### Problem

A code review of `aleph-desktop-linux` found the crate ~95% complete and
sound — `system.rs`, `automation.rs`, `pim.rs`, `sleep_inhibitor.rs`, and the
shared `action::*` / `perception::*` Linux arms (`wmctrl`, `xdg-open`,
`wl-paste`/`xclip`, Tesseract, `xcap`) are all genuinely implemented. Unlike
the Windows crate, it uses no version-fragile FFI crate — only `std::process`
subprocesses — so it carries no compile-drift risk.

The single real defect: `escape_listener.rs` was a **silent no-op stub**.
`start()` returned `Ok(())` while doing nothing; `is_aborted()` always
returned `false`. The desktop tool's `check_escape` calls `listener.start()`
and, on `Ok`, assumes the abort hotkey is live — so on Linux the operator got
**no abort path and no indication** that automation could not be stopped.

### Constraint

Linux has no portable global-hotkey API. X11 needs a live display connection
plus an event-loop thread; Wayland deliberately denies global key grabs to
ordinary clients. A compositor-specific keyboard hook would be fragile,
dependency-heavy (violating R3 core-minimalism), and — critically — this host
has no Linux `std` target, so such code could not be compile-validated.

### Solution — filesystem sentinel

`LinuxEscapeListener` watches a sentinel file, `~/.aleph/desktop-abort`. The
user aborts a runaway desktop-automation session from any terminal:

```sh
touch ~/.aleph/desktop-abort
```

- `start()` clears any stale sentinel (a fresh session must not begin already
  "aborted") and logs the watch path.
- `is_aborted()` reports `true` once the sentinel exists — one `stat` per
  mutating action, negligible cost.
- `reset()` / `stop()` remove the sentinel.
- `HOME` unset → degrades to a logged no-op (never aborts).

This is pure `std` (`fs`, `path`, `env`) — zero new dependencies, identical
behaviour under X11 / Wayland / headless, and fully unit-testable on any host
even though the crate itself only compiles for Linux. It completes existing
scaffolding (`LinuxEscapeListener` was already wired into `LinuxPlatform`),
rather than adding a speculative feature.

## Item 2 — agent_id Audit Pipeline

### Problem

`src/builtin_tools/desktop/mod.rs::check_approval` built every `ActionRequest`
with two unfilled `TODO`s:

```rust
agent_id: String::new(), // TODO: plumb agent_id from agent loop call context
context: String::new(),  // TODO: populate with action description for audit
```

`ApprovalPolicy::record` — the audit sink — logs `agent = %request.agent_id`,
so every recorded desktop action carried a blank agent and no context. The
audit trail was unaccountable.

### Solution — wire `TURN_CONTEXT`

The plumbing already exists: `TURN_CONTEXT` is a task-local scoped by
`ScopedToolService::execute` (the single production tool-dispatch chokepoint)
for the duration of every tool call. `check_approval` runs before any
`spawn_blocking`, so the task-local is in scope at the read. This is a pure
wiring fix — no new harness machinery, R10-compliant.

A new pure helper `audit_identity(action, target) -> (agent_id, context)`:

- `agent_id` — `TURN_CONTEXT.session_key.agent_id()`; falls back to `"main"`
  outside a scoped turn (direct calls, tests), consistent with
  `parse_caller_agent_id`.
- `context` — `desktop.<action> (<target>)`, plus ` via <channel>/<conversation>`
  when the turn originated from a channel.

`check_approval` gains an `action: &str` parameter so the audit context names
the concrete action (`click`, `type_text`, …) rather than the coarse
`ActionType` enum.

## Testing

- **Item 1:** 5 unit tests in `escape_listener.rs` (no-sentinel, created,
  reset, stale-clear-on-start, `HOME`-unset no-op). Pure `std` — they run on
  any Linux build; not runnable on this macOS host but correct by
  construction.
- **Item 2:** 4 tests in `desktop/tests.rs` — `audit_identity` fallback,
  channel turn, non-channel turn, and an end-to-end test asserting a
  `CapturingPolicy` receives the real `agent_id` and audit context. Fully
  compiled and run on macOS (`alephcore` builds natively).

## Out of Scope

- X11/Wayland native keyboard hooks (constraint above).
- Attaching an approval policy to `DesktopTool` at construction — a separate
  concern; this cycle only ensures the pipeline is *correct* when a policy is
  present.
- Peekaboo-style gestures (YAGNI, per the table above).
