# Severed-Wire Audit — src/mcp

**Date:** 2026-08-17
**Method:** PRODUCED–CONSUMED symbol parity (`rg` across `src/`, `interfaces/`, `shared/`), read-before-write triage.
**Scope:** 40 files, ~19.2K LOC (`src/mcp/**`). Read-only; no source modified.
**Worktree:** `.worktrees/review-fix-2026-08-17` (branch `main`).

## Summary

| Severity | Count |
|----------|-------|
| critical | 0 |
| high     | 0 |
| medium   | 3 |
| low      | 12 |

| Decision | Count |
|----------|-------|
| CUT      | 9 |
| CONNECT  | 2 |
| DECIDE   | 4 |

15 findings. All severities are low/medium — the module's parallel-looking stacks
(`client.rs` / `external/` / `manager/` / `transport/` / `modern/`) are **layered, not
parallel**: each is reached from production boot and consumes the next. The severed
wires found are mostly *inert knobs* (config surface with a reader but no writer),
*orphaned re-exports*, and *test-only builders*.

---

## Architecture verification (no severed wire — checked and found wired)

- **Boot → manager:** `src/bin/aleph-server/commands/start/mod.rs:252-255` constructs
  `McpManagerActor::new(None)`, then `.with_secret_resolver(resolver).with_sampling_bridge().run()`
  (lines 492-493). Manager is live.
- **manager → client → external/connection:** `manager/actor.rs:740` builds `McpClient`,
  `client.rs:141` (`start_external_server`) and `client.rs:508/525` (`start_remote_server`)
  construct `HttpTransport`/`SseTransport`; `external/connection.rs:228` spawns
  `StdioTransport`. All three transports are constructed in production.
- **modern/ layer:** consumed by `external/connection.rs` (`mrtr`, `discover`, `headers`,
  `cache`, `error_codes`), `transport/http.rs` (`headers`, `cache`), `transport/traits.rs`
  (`McpDialect`), `protocol.rs` (`MCP_MODERN_PROTOCOL_VERSION`). No orphaned submodule;
  all pub items in `cache.rs`/`discover.rs`/`mrtr.rs` have consumers (verified per-symbol).
- **Legacy protocol.rs + jsonrpc.rs are live, not dead:** legacy `initialize` handshake at
  `external/connection.rs:356` (`MCP_LEGACY_PROTOCOL_VERSION`) and `:570`
  (`InitializeParams::aleph_default`); sampling types flow through `jsonrpc::mcp` into
  `sampling.rs`, `sampling_bridge.rs`, `context_injector.rs`, `manager/handle.rs`.
- **auth/ OAuth is wired both ends:** read path `auth::stored_bearer_token` called from
  `client.rs:479` (`start_remote_server`); write path = `mcp_login` builtin
  (`builtin_tools/mcp_login.rs:23`, registered via `tool_bridge.rs:322-323`, `LOGIN_TOOL`
  consumed by `executor/builtin_registry/definitions.rs:1462`).
- **Bridges:** `spawn_tool_bridge` (start/mod.rs:1212), `register_sampling_llm`
  (builder/agent_init/mod.rs:785), `serve_sampling` (manager/actor.rs:173),
  `preflight_remote_url` (client.rs:494), `redact_mcp_error` (tools/handlers/mcp.rs),
  `normalize_tool_schema`/`scan_description_for_injection`/`truncate_description`
  (external/connection.rs:745-765) — all wired.
- **Prior-review cross-check (re-verified on current code):** mcp-batch-1 HIGH finding
  ("broken pipe" misclassified as `SessionExpired`) is **fixed** — `error_class.rs`
  `SESSION_MARKERS` no longer lists `"broken pipe"`/`"connection closed"` and
  `TRANSIENT_MARKERS` now contains both. Not re-reported.

---

## Findings

### [MEDIUM][CONNECT] F1 — `SamplingHandler::set_client` never called → `include_context` sampling injection silently inert

- **Produced:** `SamplingHandler::set_client` — `src/mcp/sampling.rs:49`.
- **Consumers:** none.
  - `rg -n "\.set_client\(" src/ interfaces/ shared/` → **no matches** (only the definition at sampling.rs:49 and a comment at :101).
- **Form:** 1/2. The reader side exists: `handle_request` (`sampling.rs:104-108`) clones
  `self.client` and injects context when `request.include_context` is `Some` — but the
  field `client: RwLock<Option<Arc<McpClient>>>` is only ever written by `set_client`,
  which nothing calls, so it is always `None`. A server that requests `include_context`
  (`thisServer`/`allServers`) silently gets **no context** and no error.
- **Decision:** CONNECT.
- **Wire-up plan:** call `self.sampling_handler.set_client(...)` once per `McpClient` —
  the natural seam is at the end of `start_external_server`/`start_remote_server`
  (or lazily in `handle_request` if the client Arc is unavailable at construction).
  Note the handler is shared (`Arc`) into every `McpServerConnection`
  (`client.rs:148,642`), so a single set covers all servers — acceptable, or scope the
  client per-connection if per-server context is required.
- **Risk of CONNECT:** context injection adds server-listed resource/tool names into
  sampling messages (token cost, small prompt-injection surface — already gated by the
  server asking for it). If the feature is intentionally disabled, CUT the field, the
  method, and the injection branch at `sampling.rs:101-120` instead.
- **Verification:** after wiring, a server requesting `include_context: "allServers"` sees
  a prepended `SamplingMessage`; without wiring, `sampling.rs:104` (`self.client.read()`)
  is always `None`.

### [MEDIUM][CONNECT] F2 — `McpManagerConfig::tool_filter` / `McpToolFilter` / `with_tool_filter`: reader wired, zero production writers

- **Produced:** `McpManagerConfig.tool_filter: Option<McpToolFilter>` — `src/mcp/manager/types.rs:86`;
  builder `with_tool_filter` — `types.rs:196`; type `McpToolFilter` — `src/mcp/types.rs:48`;
  `McpToolFilter::is_noop` — `types.rs:70` (test-only consumers at :271,:282).
- **Consumers:** reader `manager/actor.rs:740` (`client.set_tool_filter(config.tool_filter.clone())`)
  → `client.rs:185` (`filter.allows(...)`). **No production writer:**
  - `rg -n "with_tool_filter" src/ interfaces/ shared/` → only `manager/types.rs:196` (def) and `:880` (test).
  - All production constructors (`types.rs:112,130,148`) set `tool_filter: None`;
    construction sites `hub/install.rs:44-73`, `hub/official_mcp.rs:308-320`,
    `extension/mcp_config.rs:211-233`, `gateway/handlers/mcp_config.rs:312-317,446-452`,
    `extension/loader.rs:666` never call `with_tool_filter`. The gateway JSON API
    (`McpServerConfigJson`, `mcp_config.rs:53-68`) has no `tool_filter` field; the
    "Preserve … tool_filter" comment at `mcp_config.rs:379` is aspirational — there is
    nothing to preserve.
- **Form:** 5 (inert config knob; the whole per-server allow/deny feature is unreachable).
- **Decision:** CONNECT.
- **Wire-up plan:** expose `tool_filter` (allow/deny lists) in `McpServerConfigJson` +
  `gateway/handlers/mcp_config.rs` upsert path and/or `extension/mcp_config.rs`; then the
  existing reader chain (actor → client → `McpToolFilter::allows`) activates unchanged.
  Alternative: CUT field, builder, type, `client.set_tool_filter`, and the `client.rs:180-187`
  filter branch.
- **Risk:** catalog-time filtering only (client.rs:180 comment); no call-time enforcement —
  document that, or add call-time gating, when wiring.
- **Verification:** after wiring, `hub/install.rs`-style config with a filter drops
  non-matching tools from `list_tools`.

### [MEDIUM][DECIDE] F3 — `HealthCheckConfig` is orphaned pub API, always `Default`

- **Produced:** `pub struct HealthCheckConfig { interval, max_failures, max_restarts, restart_window }` —
  `src/mcp/manager/actor.rs:39-49`; re-exported `manager/mod.rs:59` and `src/mcp/mod.rs:93`.
- **Consumers:** only inside `actor.rs` — field `health_config` (:76), always
  `HealthCheckConfig::default()` (:125), read at :215,320-324; non-default construction
  only in a test (:1163).
  - `rg -n "HealthCheckConfig" src/ --glob '!src/mcp/**'` → **no matches**.
  - `McpManagerActor::new(config_path: Option<PathBuf>)` (actor.rs:101) accepts no
    health-config parameter; boot passes `None` (start/mod.rs:255).
- **Form:** 1/6 (orphaned pub config surface; knobs are inert — intervals/restart caps are
  hard-coded defaults).
- **Decision:** DECIDE. Options: (a) add a `with_health_config(...)` builder mirroring
  `with_secret_resolver`/`with_sampling_bridge` and thread a config field through boot;
  (b) delete the struct + re-exports and inline the defaults. Do not silently keep a pub
  config type nobody can configure.
- **Risk:** the restart-window/`max_restarts` logic (actor.rs:310-353) is load-bearing;
  only the *knob surface* is inert. Any change must keep `record_failure`/`mark_dead`
  semantics intact.
- **Verification:** `rg "HealthCheckConfig" src/` shows either a boot call site (a) or only
  internal defaults (b).

### [LOW][CUT] F4 — `McpClient::sampling_handler` getter: zero callers

- **Produced:** `pub const fn sampling_handler(&self) -> &Arc<SamplingHandler>` — `src/mcp/client.rs:88-89`.
- **Consumers:** none. `rg -n "\.sampling_handler\(\)" src/ interfaces/ shared/` → **no matches**
  (field `sampling_handler` itself is used internally at :71,148,538,642,100).
- **Form:** 1. Decision: **CUT** (delete lines 88-89). Harmless; no behavior change.
- **Verification:** `cargo check -p alephcore` after removal (not run per protocol).

### [LOW][CUT] F5 — `McpErrorKind::as_str`: zero callers

- **Produced:** `pub const fn as_str(&self) -> &'static str` — `src/mcp/error_class.rs:35-45`.
- **Consumers:** none (the `as_str` hits repo-wide are other types — `ExtensionKind`,
  `PromptRole`). `rg -n "as_str" src/mcp/error_class.rs` → only the definition (:35).
  Variants themselves are consumed (`McpErrorKind::AuthExpired` etc. at
  `transport/http.rs:616,623`, `external/connection.rs:1489`); only `as_str` is dead.
- **Form:** 1. Decision: **CUT**.
- **Verification:** remove method; `classify_mcp_error` + guidance paths unaffected.

### [LOW][CUT] F6 — `sampling_llm_registered`: re-exported crate API, test-only consumer

- **Produced:** `pub fn sampling_llm_registered() -> bool` — `src/mcp/sampling_bridge.rs:44`;
  re-exported `src/mcp/mod.rs:78`.
- **Consumers:** only its own test (`sampling_bridge.rs:170`) and the re-export.
  `rg -n "sampling_llm_registered" src/ interfaces/ shared/` → sampling_bridge.rs:44, :170, mod.rs:78.
  (`register_sampling_llm` at agent_init/mod.rs:785 and `serve_sampling` at actor.rs:173 are live;
  this predicate is not part of the production path — `serve_sampling` reads the OnceCell directly.)
- **Form:** 1/4/6 (pub crate-level API with no production consumer).
- **Decision:** CUT — drop the re-export (mod.rs:78) and the fn, or gate `#[cfg(test)]`.
- **Verification:** no production references after removal.

### [LOW][CUT] F7 — `modern::RESULT_TYPE_COMPLETE`: zero consumers

- **Produced:** `pub const RESULT_TYPE_COMPLETE: &str = "complete";` — `src/mcp/modern/mod.rs:259`.
- **Consumers:** none. `rg -n "RESULT_TYPE_COMPLETE" src/ interfaces/ shared/` → only mod.rs:259.
  (`RESULT_TYPE_INPUT_REQUIRED` is used at mod.rs:271; `result_kind` treats any other
  value as `Complete` by design, so the constant is never needed.)
- **Form:** 1. Decision: **CUT** (line 259).
- **Verification:** `result_kind` tests (mod.rs:402-416) still pass.

### [LOW][DECIDE] F8 — `modern::error_codes::SPEC_RESERVED_RANGE`: zero consumers

- **Produced:** `pub const SPEC_RESERVED_RANGE: RangeInclusive<i32> = -32099..=-32020;` — `src/mcp/modern/mod.rs:73`.
- **Consumers:** none. `rg -n "SPEC_RESERVED_RANGE" src/ interfaces/ shared/` → only mod.rs:73.
  `is_modern_error` (mod.rs:78-85) deliberately compares bounds literally because
  `RangeInclusive::contains` is not `const` — the const is documentation-only.
- **Form:** 1. **Decision:** DECIDE — either CUT it, or keep it as spec documentation and
  add a test asserting `is_modern_error` bounds equal the constant's (prevents silent drift).
- **Verification:** n/a (no behavior).

### [LOW][CUT] F9 — `check_all_runtimes`: test-only

- **Produced:** `pub fn check_all_runtimes() -> Vec<RuntimeCheckResult>` — `src/mcp/external/runtime.rs:183`.
- **Consumers:** only `runtime.rs:257` (test). `rg -n "check_all_runtimes" src/ interfaces/ shared/` →
  runtime.rs:183, :257. Not re-exported at crate level (`external/mod.rs:13` re-exports only
  `check_runtime, RuntimeKind`).
- **Form:** 4. Decision: **CUT** or gate `#[cfg(test)]`.
- **Verification:** `check_runtime` path (client.rs:116) unaffected.

### [LOW][DECIDE] F10 — `CallbackServer::with_port` / `with_timeout`: test-only builders

- **Produced:** `pub const fn with_port` — `src/mcp/auth/callback.rs:64`; `pub const fn with_timeout` — `:72`.
- **Consumers:** only tests (`callback.rs:406,413,447`). Production (`builtin_tools/mcp_login.rs:127`)
  uses `CallbackServer::new()` with defaults.
- **Form:** 4. **Decision:** DECIDE — harmless builder API on a pub re-exported type
  (`mod.rs:61-63`); either keep as extension point or gate `#[cfg(test)]`.
- **Verification:** n/a.

### [LOW][DECIDE] F11 — `McpRemoteServerConfig::with_bearer_token` / `with_header`: test-only builders

- **Produced:** `with_bearer_token` — `src/mcp/types.rs:219`; `with_header` — `:228`.
- **Consumers:** only the serde round-trip test (`types.rs:333-336`). Production sets
  `config.headers` directly (`manager/actor.rs:816`, `client.rs:477-488`).
- **Form:** 4. **Decision:** DECIDE — keep (small convenience API on a pub config type) or
  gate `#[cfg(test)]`. If kept, consider using them in `actor.rs:816` so the surface is live.
- **Verification:** n/a.

### [LOW][CUT] F12 — `McpTransport::as_any`: required trait method, zero call sites

- **Produced:** `fn as_any(&self) -> &dyn std::any::Any;` — `src/mcp/transport/traits.rs:154`;
  implemented at `transport/http.rs:580`, `transport/sse.rs:582`, `transport/stdio.rs:561`,
  test mocks at `traits.rs:277` and `external/connection.rs:1738`.
- **Consumers:** none. `rg -n "\.as_any\(" src/ interfaces/ shared/` → **no matches**.
  No downcast exists anywhere; `set_request_handler` (SSE) is reached via the concrete
  `SseTransport` in `client.rs:525-542`, not through `as_any`.
- **Form:** 1. Decision: **CUT** — remove from trait + the 5 impl sites (mechanical;
  also drop `use std::any::Any` constraints if they exist solely for this).
- **Verification:** `cargo check -p alephcore` (not run per protocol); the three transports
  remain usable via the trait object.

### [LOW][CUT] F13 — orphaned re-exports of context_injector types

- **Produced:** `src/mcp/mod.rs:66` — `pub use context_injector::{ContextInjector, InjectedContext, ResourceContext, ToolContext};`
- **Consumers:** none for the re-export path. `ContextInjector` is reached internally via
  the module path (`sampling.rs:13`); the three data types are constructed/consumed only
  inside `context_injector.rs` and `sampling.rs`.
  - `rg -n "InjectedContext" src/ interfaces/ shared/` → only context_injector.rs + sampling.rs (module-internal).
  - `rg -n "\bmcp::ToolContext\b|mcp::InjectedContext|mcp::ResourceContext" src/ interfaces/ shared/` → **no matches**.
- **Form:** 6. Decision: **CUT** the re-export line (keep the types pub-in-module or make
  them private). Bonus: removes the `ToolContext` name shadowing hazard with
  `crate::tools::context::ToolContext`.
- **Verification:** grep for `mcp::ToolContext` stays empty; sampling/context injection
  (F1 reader) unaffected.

### [LOW][CUT] F14 — orphaned re-export of `SseEvent` / `SseNotification` / `SseRequest`

- **Produced:** `src/mcp/transport/mod.rs:24` — `pub use sse_events::{SseEvent, SseNotification, SseRequest};`
- **Consumers:** only `transport/sse.rs` (module-internal; `sse.rs:27,335-435`).
  `rg -n "SseEvent|SseNotification|SseRequest" src/ interfaces/ shared/ --glob '!src/mcp/**'` → **no matches**.
  Not re-exported from `src/mcp/mod.rs:83-86` (which lists only the three transports +
  `McpTransport` + `NotificationCallback`).
- **Form:** 6. Decision: **CUT** line 24 (types remain reachable inside `transport::sse`).
- **Verification:** grep stays empty; SSE transport unaffected.

### [LOW][CUT] F15 — stale `sync_builtins` doc references (name drift)

- **Produced (stale):** `src/executor/builtin_registry/definitions.rs:1416` —
  "`mcp/tool_bridge.rs::sync_builtins` registers these against a capability gate", and
  `:2512` — "…`sync_builtins` keys every install off it".
- **Consumers:** the referenced function does not exist. `rg -n "sync_builtins" src/mcp/` → **no matches**;
  the actual registration driver is `reconcile_capability_tools` (`src/mcp/tool_bridge.rs:263`).
- **Form:** 5 (name-drift residue: docs describing a renamed symbol; the const-name table
  those docs explain is otherwise correct — the `RESOURCE_TOOL`/`PROMPT_TOOL`/`LOGIN_TOOL`
  consts at tool_bridge.rs:36-54 are consumed at definitions.rs:1437-1464).
- **Decision:** CUT (fix the two doc comments to reference `reconcile_capability_tools`).
- **Verification:** `rg "sync_builtins" src/` → no matches after the doc fix.

---

## Skipped / not reported

- `SamplingHandler::text_response` / `error_response` (sampling.rs:142,153) — explicitly
  `#[cfg(test)]`; test-only by design, not a severed wire.
- Field-level usage of data types (`McpPreset` fields, `McpTool`, `McpResource`,
  `McpPrompt`, protocol wire structs) — verified consumed; not findings.
- No `#[allow(dead_code)]` and no `#[deprecated]` items exist anywhere in `src/mcp/`
  (checked: `rg "allow\(dead_code\)|deprecated" src/mcp/` → none) — so form 6 is limited
  to the re-export surface reported above.
- Did **not** run cargo (protocol constraint); all claims backed by `rg` evidence above.
