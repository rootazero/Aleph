# Module: webchat-core-api (round 1)

- **Path**: `interfaces/webchat/src`
- **Files scanned**: 73 .rs files
  - Top-level: 12 (`lib.rs`, `app.rs`, `api.rs`, `context.rs`, `generation.rs`, `models.rs`, `preset_providers.rs`, `disposed_reads.rs`, `appearance.rs`, `i18n.rs`, `i18n_census.rs`, `platform_host.rs`, `panic_overlay.rs`)
  - `api/*.rs`: 43
  - `state/*.rs`: 12
  - `memory_graph/*.rs`: 6
- **Total LOC**: ~20 600
- **R2/R4/R10 verification summary**
  - **R2 UI-logic-in-WASM**: PASS. All business UI state lives in the WASM Panel; the native shell is a container (see `platform/` and `desktop/`, out of scope for this batch).
  - **R4 Interface-is-pure-I/O**: PASS. `api/*.rs` are thin JSON-RPC wrappers around Gateway methods; no persistence/memory/task-planning logic lives in the interface layer.
  - **R10 Thin harness / zero middleware tax**: PASS. The Panel does not contain an agent harness; `state/typewriter.rs` and `state/reattach.rs` are pure presentation/repair plumbing. No extra LLM-routing middleware.

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 2 |
| Low      | 4 |

All High and Medium findings received minimal root-cause fixes in this worktree.

## High-Confidence Issues

### [High] `state/sessions.rs:94` production `.expect()` in `SessionMap::new`
- **Location**: `interfaces/webchat/src/state/sessions.rs:94`
- **Description**: `let owner = Owner::current().expect("SessionMap::new must run under a reactive owner");` is production code that panics if `SessionMap::new()` is ever called outside an active Leptos owner. It violates the project rule "Never `unwrap()` in production" (AGENTS.md). The current caller is `app.rs::AppContent`, which does run under an owner, so the invariant is not enforced by the type system and is one refactor/move away from crashing the whole Panel.
- **Trigger condition**: Any future caller constructing `SessionMap` outside a component/effect (e.g. in a test helper, a phone screen refactor, or a leaked async task) will immediately panic the WASM runtime.
- **Expected vs Actual**: Expected: a fallible/default construction path. Actual: hard panic.
- **Suggested fix / Decision**: FIXED — replaced the `expect` with `Owner::current().unwrap_or_else(Owner::new);` so a missing owner falls back to a fresh root owner instead of panicking. The change is one line and preserves the existing in-component behavior.

### [Medium] `api/chat.rs:send` has no client-side cap on attachment payload size
- **Location**: `interfaces/webchat/src/api/chat.rs` (`ChatApi::send`)
- **Description**: File attachments are forwarded as base64 strings inside the `chat.send` JSON-RPC params. The function previously summed the payload only to build the array; it never rejected an accidentally huge paste/drop. A multi-megabyte attachment creates a giant JSON body, blocks the message loop, and can stall layout before the server enforces its own limit.
- **Trigger condition**: User drops or pastes a large file into the composer.
- **Expected vs Actual**: Expected: early, user-visible rejection for unreasonably large attachments. Actual: the Panel serializes and attempts to send an unbounded payload.
- **Suggested fix / Decision**: FIXED — added `ATTACHMENT_TOTAL_CAP` (10 000 000 base64 bytes, ~10 MB) and a pre-serialization guard that returns an error if the total base64 length exceeds the cap.

### [Medium] `memory_graph/markdown_excerpt.rs` parses arbitrarily large markdown
- **Location**: `interfaces/webchat/src/memory_graph/markdown_excerpt.rs` (`render_excerpt`)
- **Description**: `render_excerpt` only emits 180 characters of HTML, yet it runs the full `pulldown_cmark` parser over the entire source note. A multi-megabyte note therefore pays the full parse cost for output that will be truncated anyway.
- **Trigger condition**: Opening a memory note / graph node whose content is very large.
- **Expected vs Actual**: Expected: excerpt rendering cost bounded by excerpt size. Actual: cost scales with the whole note.
- **Suggested fix / Decision**: FIXED — added `MAX_INPUT_LEN` (4096 chars) and truncate the input before parsing; the output is still capped at 180 chars and the ellipsis behavior is preserved.

### [Low] Hard-coded English UI copy in `context.rs` fallback and alerts
- **Location**: `interfaces/webchat/src/context.rs` (`DashboardContext` ErrorBoundary fallback, `load_initial_alerts`)
- **Description**: Strings such as `"System Error"`, `"Reload Dashboard"`, and `"Database size: {db_size:.1} MB"` are emitted directly from Rust rather than through `t!` / `t_string!`. They are visible strings that do not follow the locale file discipline enforced by `i18n_census.rs`.
- **Trigger condition**: A render panic or a memory-status alert on a non-English locale.
- **Suggested fix / Decision**: Report only. These are fallback/alert messages; moving them into `locales/{en,zh}.json` is a separate i18n sweep.

### [Low] `api/memory.rs::format_timestamp_secs` uses `js_sys::Date` without target gating
- **Location**: `interfaces/webchat/src/api/memory.rs` (`format_timestamp_secs`)
- **Description**: The helper constructs a `js_sys::Date` unconditionally. It is only reached in the WASM build, but it is not `#[cfg(target_arch = "wasm32")]`-gated; a host-unit-test call would panic.
- **Trigger condition**: Any future test that exercises `RawMemory` formatting on the host.
- **Suggested fix / Decision**: Report only. No current test calls it, and adding a cfg gate would touch the public signature.

### [Low] `state/memory.rs::MemoryState::new` creates a side-effecting `Effect` in the constructor
- **Location**: `interfaces/webchat/src/state/memory.rs:53-64`
- **Description**: Persistence of `sidebar_collapsed` is wired inside `MemoryState::new()` via `Effect::new`. This couples state construction to a reactive owner and makes off-owner/unit-test construction fragile.
- **Trigger condition**: Calling `MemoryState::new()` outside a component owner.
- **Suggested fix / Decision**: Report only. The call currently lives in `AppContent`, where an owner exists; refactoring persistence out is a design change beyond this audit round.

### [Low] API route-registration concept does not apply to WASM client
- **Location**: `interfaces/webchat/src/api/*.rs`
- **Description**: The audit checklist asked for "every api/* handler registered in the router". `interfaces/webchat` is the Leptos WASM Panel; `api/*.rs` are typed JSON-RPC client wrappers, not server-side handlers. There is no central route table in this crate. Instead, the panel relies on `DashboardState::subscribe_topic` / `subscribe_events` to register event interests, and the per-module API structs are consumed directly by views.
- **Suggested fix / Decision**: Report only. Verified that each `api/*.rs` module exposes public methods and that the entry `api.rs` re-exports modules for ergonomic access; no registration step is missing.

## Per-perspective findings

### Security
- **XSS surface**: `panic_overlay.rs` uses `set_inner_html` but escapes `<`, `>`, `&` via `escape_html` before insertion. The panic message/stack may contain user data, but the escaping is correct.
- **Markdown excerpt**: `memory_graph/markdown_excerpt.rs` escapes HTML metacharacters and rejects `javascript:` / protocol-relative URLs in links, so output is safe for `inner_html=`.
- **Credential handling**: `context.rs` strips `?token=` and `?bt=` from the URL after handshake and scrubs them from crash-log URLs. localStorage keys are key names only.
- **Security config wire**: `api/security.rs` types rate-limit buckets and PII rules; serialization/deserialization is through serde, no machine logic.

### Logic
- **RPC readiness floor**: `context.rs::rpc_call` waits for `is_connected` via `await_gateway_ready`, preventing mount-time "not connected" races. This is the right shape for a WASM Panel.
- **SessionMap**: background conversation registry and `server_running` baseline reset across reconnects are correctly modeled; the fixed `expect` was the only production panic surface.
- **Typewriter pacing**: `state/typewriter.rs` bounds reveal lag to `MAX_REVEAL_LAG_SECS` and prunes stale cursors, preventing unbounded map growth.
- **Reconnect repair**: `state/reattach.rs` settles abandoned runs and re-joins live runs from one `run_concurrency` snapshot, avoiding the previous cross-connection seq-latching bug.

### Architecture (R1–R10)
- **R1**: No direct platform API calls in scope; all host probing is gated `#[cfg(target_arch = "wasm32")]`.
- **R2**: Complex UI (settings, chat, memory, canvas) lives in WASM; native shell code is in `platform/` and `desktop/`.
- **R3**: No heavy new dependencies introduced in this scope.
- **R4**: `api/*.rs` only translate between UI params and Gateway JSON-RPC; business state stays in Core.
- **R5/R6**: Multi-channel/multi-form-factor state sharing is handled via Leptos context (`provide_context`/`expect_context`) in `app.rs`.
- **R7**: No rule-engine intent detection in the Panel.
- **R8**: Tool/schema data is fetched from Core; no hard-coded command tables.
- **R9/R10**: Configuration surfaces are driven by Core RPCs; no thick middleware.

### Quality
- Source-level guards (`disposed_reads.rs`, `i18n_census.rs`) are present and well-tested, catching `get_untracked` after await and hard-coded copy regressions.
- Most `unwrap`/`expect` sites in the scope are inside `#[cfg(test)]` modules; the single production `expect` was fixed.
- Error handling in API wrappers uniformly returns `Result<..., String>`; no silent panics remain.

## Conclusion

`interfaces/webchat/src` is in solid architectural shape: the state-store wiring is centralized in `app.rs`, the API layer is a thin RPC facade, and the module guards have prevented several classes of regressions. The one production panic surface (`state/sessions.rs`) and the two unbounded-input DoS paths (`api/chat.rs` attachments, `memory_graph/markdown_excerpt.rs` excerpt parsing) were fixed directly. The remaining Low findings are localization/structural cleanups that do not affect runtime safety.
