# Module: desktop-shell (round 1)

- Path: `desktop/shell/`
- Files scanned: 29 (23 `.rs` + `Cargo.toml` + `build.rs` + 4 JSON config files)
- Total LOC: ~7 330 (7 127 `.rs`, 201 config/build)
- R1/R5/R9 verification summary:
  - **R1 (brain-limb separation)**: PASS — the shell is pure OS integration; no core/business logic lives here.
  - **R5 (AI comes to you / don't steal focus)**: PASS — notifications focus-gate, update prompts are non-modal, tray/dock re-entry is quiet.
  - **R9 (all configurability exposed as tools)**: PASS — the only invoke surface is connection-target config; no business logic commands.

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 0 |
| Medium   | 4 (all fixed) |
| Low      | 4 (report only) |

Most of the specific defects called out in the audit brief are **already fixed** in this worktree: cert TOCTOU pins to fingerprint, webview media grants check origin + audio-only, deep-link logs are redacted, remote WS credentials are sent only over `wss`, external-link matches full origin, update sentinels check origin, and token deletion failures are logged. The remaining findings are smaller follow-ups around URL-scheme handling, log hygiene, JS escaping, and main-thread dispatch on macOS.

## High-Confidence Issues

### [Medium] `data:` URLs treated as internal; `javascript:` URLs handed to the OS handler

- **Location**: `desktop/shell/src/external_link.rs:90-103`
- **Description**: `is_internal` returns `true` for the `data:` scheme, so a `data:text/html,…` navigation loads inside the shell's single webview. `javascript:` falls through to `open_external`, which passes it to `xdg-open` / `open` / `rundll32`. Both schemes are unsafe in an external-nav guard.
- **Trigger condition**: A malicious or attacker-influenced Panel link uses `data:` or `javascript:`; or a crafted `_blank` anchor is rewritten by `CLICK_INTERCEPTOR_JS` into a top-level navigation.
- **Expected vs actual**: Expected: pseudo-schemes that are not the Panel surface are rejected outright. Actual: `data:` loads in-webview; `javascript:` is forwarded to the OS handler.
- **Suggested fix / Decision**: FIXED. Removed `data` from the internal scheme list and added an explicit `route` rejection for `javascript:` and `data:` URLs before any OS handler is invoked. Added regression tests.

### [Medium] Malformed notification-bridge frame leaks raw head at `warn` level

- **Location**: `desktop/shell/src/notify.rs:120-127`
- **Description**: The comment correctly states that raw frame text must stay at trace level only, but the `tracing::warn!` call included `head = &text[..80]`. A malformed frame may still contain tokens, approval titles/bodies, or other sensitive payload in its first 80 bytes.
- **Trigger condition**: Gateway sends a non-JSON frame over the event bus while the shell is at `info`/`warn` logging.
- **Expected vs actual**: Expected: raw frame content never reaches `warn`. Actual: first 80 bytes are emitted at `warn`.
- **Suggested fix / Decision**: FIXED. Removed the `head` field from the warn span; the raw frame remains available at `trace` only.

### [Medium] Daemon-error splash eval uses hand-rolled JS escaping

- **Location**: `desktop/shell/src/main.rs:552-566`
- **Description**: `show_daemon_error` only escaped backslash and single-quote before inserting the daemon message into a single-quoted JS string. Newlines, carriage returns, and other control characters were not escaped, so a daemon error containing them breaks the JS literal and can alter the executed script.
- **Trigger condition**: `aleph-server` fails to start and emits an error string containing a newline or unmatched quote.
- **Expected vs actual**: Expected: arbitrary daemon text is safely embedded in the eval'd script. Actual: the string can break out of the literal.
- **Suggested fix / Decision**: FIXED. Replaced the hand-rolled replace with `serde_json::to_string`, which produces a fully escaped, JS-safe double-quoted literal. Also made the helper dispatch its window show/eval to the main thread on macOS (see next finding).

### [Medium] `focus_window` runs window UI off the main thread on macOS

- **Location**: `desktop/shell/src/main.rs:525-548`
- **Description**: `focus_window` is called from the background tokio runtime (lite boot, lite supervisor, notification-driven paths), from global-shortcut callbacks, and from tray/menu handlers. Only the tray/menu handlers are guaranteed to be on the main thread; on macOS, AppKit window operations must be dispatched to the main thread.
- **Trigger condition**: Lite shell first reveal, remote target recovery, or any future background caller invokes `focus_window` on macOS.
- **Expected vs actual**: Expected: show/unminimize/set_focus are main-thread-affine on macOS. Actual: they execute on whatever thread called `focus_window`.
- **Suggested fix / Decision**: FIXED. Kept the `RevealGate` latch set synchronously (so the single-instance guard observes it immediately), then dispatched the actual UI operations to the main thread on macOS via `AppHandle::run_on_main_thread`. Non-macOS platforms continue to call directly.

## Per-perspective findings

### Security

- **Cert trust TOCTOU**: `cert_trust/pending.rs:79-95` already requires both `host` and `fingerprint` to match the pending record before approval. A second TLS challenge overwriting the record cannot cause approval of an unseen fingerprint.
- **Deep-link logging**: `deeplink.rs:60-75` redacts query, fragment, and path before the `info!` log; full URL is only emitted at debug. No fix needed.
- **External nav allow-list**: `external_link.rs` already matches full origin (`scheme://host:port`), with regression tests for different scheme/port on the same host. The new fix above adds `javascript:`/`data:` rejection.
- **Gateway token transport**: `notify.rs:179-195` only sends credentials when the remote target scheme is `https` (mapped to `wss`); an `http` remote connects as guest with a logged warning. No fix needed.
- **Update sentinels**: `update.rs:85-101` verifies the sentinel URL originates from the Panel's configured origin via `ConnectionTarget::serves_origin`, with tests for foreign origins and different ports. No fix needed.
- **Loopback any-port trust**: `external_link.rs:121-124` treats **any** `http(s)` URL on `127.0.0.1` / `localhost` / `[::1]` as internal, not just port `18790`. A malicious Panel page could therefore navigate the webview to another local service (e.g. `http://127.0.0.1:3000/`). The remote-origin branch is correctly pinned; loopback is the remaining over-broad arm. **Low/Medium — report only** (tightening it changes existing tests and the local-dev assumption; recommend a follow-up that restricts loopback to the configured daemon port).
- **Default capability**: `capabilities/default.json` grants `core:default`, which is a broad bundle of window/webview permissions, but it is scoped to `http://127.0.0.1:18790/*` and the dynamic remote-drag capability adds only `core:window:allow-start-dragging`. Acceptable for a local-trust Panel; still worth auditing whether `core:default` can be narrowed to an explicit allow-list. **Low — report only**.

### Logic / Threading

- **Daemon child PID handling**: The shell does not store or signal daemon PIDs; lifecycle is port-based and shutdown is via `aleph-server stop`. PID-reuse risk from `utils/scratch.rs` does not apply here.
- **Background-thread window operations beyond `focus_window`**: `navigate_to_target`, `show_connection_page`, and `show_lite_connect_page` are also called from the background tokio runtime on macOS. They were not refactored in this round to keep the diff focused. The `focus_window` fix addresses the explicitly flagged tray/summon path; the remaining calls are **Low — report only** and should be main-thread-dispatched in a follow-up if macOS asserts surface.
- **Permission monitor**: `perm_monitor.rs` only triggers daemon restart for `InputMonitoring` and `ScreenRecording`; microphone/camera TCC changes are correctly left alone. No fix needed.
- **Lite double-probe**: `connect_setup::connect_to` probes, then calls `connection::set_connection_target`, which calls `reroute_for_target` and probes again. The second probe is bounded and harmless, but redundant. **Low — report only**.
- **Cert approval reroute**: `cert_trust::pending::approve_cert` reloads the *current* target, not the target that originally triggered the TLS challenge. If the user switched targets while the prompt was open, the approved cert is pinned but the webview navigates somewhere else. **Low — report only**.

### Architecture

- **R2 / R4 hold**: All invoke commands are I/O config toggles; no business logic or UI reasoning lives in the shell.
- **R10 (thin harness)**: The supervisor state machine is pure and unit-tested; resident loops are minimal.
- **Cross-platform parity**: macOS, Windows, and Linux cert-trust adapters all feed the same `install::resolve` decision core.

### Quality

- **Test coverage**: The module has extensive source-level and runtime tests, especially for the supervisor, connection parsing, external-link routing, gateway probe, and cert decision core.
- **Error handling**: No production `expect`/`unwrap` surfaces remain; poisoned locks use `unwrap_or_else(PoisonError::into_inner)` per project convention.
- **Logging**: Deep-link and malformed-frame logging are now redacted; daemon-error eval is safely escaped.

## Conclusion

`desktop/shell/` is in solid shape. The audit brief's headline risks (cert TOCTOU, webview media grants, deep-link token leakage, WS credential leak, external-link origin check, update origin check, token deletion observability) are already remediated. This round added four medium fixes: rejecting `javascript:`/`data:` URLs, keeping malformed bridge frames out of warn logs, safe daemon-error JS escaping, and main-thread dispatch for `focus_window` on macOS. Four low-severity items remain as report-only follow-ups, led by the loopback any-port trust arm and the broader set of background-thread window operations on macOS.

## What was not done

- No `cargo check` / `cargo test` / `clippy` was run per the task constraints.
- `desktop/shared`, `desktop/linux`, `desktop/macos`, `desktop/windows`, and `Cargo.lock` were not modified.
- The loopback any-port allow-list, lite double-probe, cert-approval reroute target drift, and remaining background-thread window operations on macOS were reported but not changed.
