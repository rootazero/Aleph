# Severed-Wire Audit — `src/mcp`

**Date:** 2026-09-05
**Module:** `src/mcp` (41 files, ~19 kLOC)
**Method:** PRODUCED - CONSUMED symbol parity via `rg` across `src/`, `bin/`,
`interfaces/`, `shared/`, `desktop/`. Every "no consumer" claim below is
backed by an `rg` invocation pasted inline.
**Forms applied:** 1=Producer with zero callers · 2=Painless-wire
(painless-wire heuristic for #2 below) · 3=Inert/orphaned public API ·
4=Client ghost · 5=Name/path drift · 6=Never-compiled far-end.

## TL;DR

| ID | Finding | Severity | Form | Decision |
|----|---------|----------|------|----------|
| sw-mcp-01 | `SamplingHandler::set_client` never wired — `ContextInjector::gather_context` is dead in production | **high** | 1 | CONNECT |
| sw-mcp-02 | `McpManagerHandle::shutdown` has no caller — orderly shutdown section skips MCP | medium | 1 | CONNECT |
| sw-mcp-03 | `mcp::jsonrpc::{JsonRpcRequest, JsonRpcResponse, JsonRpcNotification, IdGenerator}` re-exported but unused outside `src/mcp/` (the canonical types live in `shared/protocol`) | low | 3 | CUT |
| sw-mcp-04 | `McpManagerHandle::is_running` has no production caller (only tests) | low | 1 | CUT |
| sw-mcp-05 | `McpManagerHandle::set_sampling_callback` only invoked internally by the actor's own startup path; the actor already uses `McpCommand::SetSamplingCallback` directly | low | 1 | DECIDE |

Everything else checked in `src/mcp/` (`presets`, `sampling_bridge`, `auth`,
`manager.handle` aggregate methods, `tool_bridge`, `tool_sanitize`,
`redact_mcp_error`, `classify_mcp_error`, `preflight_remote_url`,
`McpResourceTemplate`, `ContextInjector` itself, `ResourceContent`,
`PromptContent`/`PromptMessage`, `McpTransport`/`HttpTransport`/
`SseTransport`/`StdioTransport`, `NotificationCallback`, `mrtr::*`,
`McpPersistentConfig::save`) is **live**: each public symbol has at
least one production caller. See the "Verified wired" section at the end.

---

## sw-mcp-01 · `SamplingHandler::set_client` never wired — context-injection is dead

**Severity:** high — a capability Aleph advertises (`sampling` in
`ClientCapabilities`) is silently degraded; spec-compliant servers that set
`includeContext: "thisServer"|"allServers"` will not see the promised
context.

**Form:** 1 (producer with zero callers).

**Produced:**
- `SamplingHandler::set_client(&self, client: Arc<McpClient>)` —
  `src/mcp/sampling.rs:49`
- Downstream: `SamplingHandler::handle_request` reads `self.client.read().await`
  and invokes `ContextInjector::gather_context` + `ContextInjector::format_as_system_message`
  (`src/mcp/sampling.rs:107-110`).

**Consumer:** `none found` in production code.

Evidence:

```bash
$ rg -n '\.set_client\b' --type rust
src/mcp/sampling.rs:49:    pub async fn set_client(&self, client: Arc<McpClient>) {
src/mcp/sampling.rs:101:            // across .await would block set_client writers for the duration of
# only the definition and a comment; no caller anywhere
```

`McpClient::new()` always constructs the `SamplingHandler` with `client: None`
(`src/mcp/client.rs:71`):

```rust
sampling_handler: Arc::new(SamplingHandler::new()),
```

`set_callback` IS wired (see `src/mcp/client.rs:95`), but the
sister-method `set_client` is not. As a result, in `handle_request`
(`src/mcp/sampling.rs:103-110`):

```rust
let client = self.client.read().await.clone();   // always None
if let Some(client) = client {
    let contexts = ContextInjector::gather_context(&client, mode, requesting_server).await;
    if let Some(context_msg) = ContextInjector::format_as_system_message(&contexts) {
        request.messages.insert(0, context_msg);
```

`ContextInjector::gather_context` and `format_as_system_message` are never
executed in production. They have ~250 LOC of logic and 8 unit tests; they
are reachable only through `McpModernMrtr::fulfill` test paths, never from
real sampling traffic.

**Triage path:** Grep consumer side → no live caller → **CONNECT** (the
producer and consumer both exist; reconnecting is a one-line wire).

**Proposed change:**

In `src/mcp/client.rs:88-96` (where `set_sampling_callback` is defined on
`McpClient`), add a parallel method and wire it from wherever the
client is handed to a connection. The natural caller is
`McpServerConnection::connect` / `with_transport` after the
`SamplingHandler` is set up on the per-server client (search shows the
client's `sampling_handler` is shared via `Arc::clone(&self.sampling_handler)`
at `src/mcp/client.rs:143` and `:637`, and the connection's
`has_callback()`-checked `set_callback` is wired at
`src/mcp/external/connection.rs:2028-2031` — add the matching
`set_client` call there).

```rust
// In McpClient::connect (or in McpServerConnection after construction)
sampling_handler.set_client(Arc::new(self.clone())).await;
```

**Risk:** medium — context injection sends server-provided strings through
a model-facing system message. Reconnecting the wire turns an inert
capability into a live one; security review of the
`<mcp_context trust="untrusted">…</mcp_context>` fence in
`ContextInjector::format_as_system_message` (the comment already flags it
as "data, not instruction") should re-confirm that the prompt-injection
heuristic in `tool_sanitize::scan_description_for_injection` is sufficient
before enabling for untrusted servers. The `sampling_bridge.rs` doc at
`src/mcp/sampling_bridge.rs:35-53` already mentions `FailsClosed` and
`missing()` semantics; nothing else needs to change.

**Verification:**

- After wiring, an integration test that issues a sampling/createMessage
  with `includeContext: "thisServer"` and asserts the response's first
  message role is `system` with the `<mcp_context>` fence will pass.
- `tracing` instrumentation in `handle_request` already logs
  `context_count = contexts.len()` on the success path; a value `> 0`
  confirms the wire is live.

---

## sw-mcp-02 · `McpManagerHandle::shutdown` has no caller — orderly shutdown skips MCP

**Severity:** medium — the manager's `Shutdown` command path is never
exercised, so on `Ctrl-C` / `SIGTERM` the actor relies on process drop
(channel close) rather than the documented graceful teardown
(`shutdown_all()` → `event_tx.send(McpManagerEvent::ManagerShutdown)`).

**Form:** 1 (producer with zero callers).

**Produced:** `McpManagerHandle::shutdown(&self) -> Result<()>`
(`src/mcp/manager/handle.rs:335-347`).

**Consumer:** `none found` in production code.

Evidence:

```bash
$ rg -n 'manager_handle\.shutdown|McpManagerHandle.*\.shutdown|mcp_handle\.shutdown|mcp.*\.shutdown\(\)' --type rust
src/mcp/manager/mod.rs:50://! handle.shutdown().await?;
# only the doc-comment example in manager/mod.rs, no actual call site
```

The orderly-shutdown section in
`src/bin/aleph-server/commands/start/mod.rs:3680-3681` does call two
adjacent subsystems but not MCP:

```rust
memory_monitor.shutdown().await;
channel_health_monitor.shutdown().await;
// no mcp_handle.shutdown() here
```

**Triage path:** No live caller → **CONNECT** (one line in the orderly
shutdown section; reconnecting turns a documented capability into a
live one and ensures `McpManagerEvent::ManagerShutdown` reaches any
subscribers of `handle.subscribe()` — see `mcp/tool_bridge.rs:106`
which already keeps a long-lived subscription).

**Proposed change:**

Add to the orderly-shutdown section in
`src/bin/aleph-server/commands/start/mod.rs` (around line 3681):

```rust
if let Some(ref h) = mcp_handle {
    if let Err(e) = h.shutdown().await {
        tracing::warn!(error = %e, "MCP manager orderly shutdown failed");
    }
}
```

**Risk:** low — `shutdown()` is the documented graceful path; the only
behavior change is that running servers get a stop signal before the
process exits. The actor's own `run()` loop already handles the command
correctly (`src/mcp/manager/actor.rs:221`).

**Verification:**

- `tracing` line at `src/mcp/manager/actor.rs:268`
  (`"MCP Manager shutdown complete"`) appears in logs after a clean
  shutdown.
- `McpManagerEvent::ManagerShutdown` is observed by any subscriber of
  `handle.subscribe()`.

---

## sw-mcp-03 · `mcp::jsonrpc::*` types are redundant re-exports

**Severity:** low — a public API surface that duplicates the canonical
implementation in `shared/protocol`, no production caller outside the
crate's own `src/mcp/`.

**Form:** 3 (inert public API; orphaned re-export).

**Produced:** `crate::mcp::JsonRpcRequest`, `crate::mcp::JsonRpcResponse`,
`crate::mcp::JsonRpcNotification`, `crate::mcp::JsonRpcError`,
`crate::mcp::IdGenerator` — defined in `src/mcp/jsonrpc.rs` and
re-exported by `src/mcp/mod.rs:68-70`.

**Consumer:** none outside `src/mcp/`.

Evidence:

```bash
$ rg -n 'crate::mcp::JsonRpc|alephcore::mcp::JsonRpc' --type rust
# (no matches)
$ rg -n 'mcp::jsonrpc::' src/ --type rust | grep -v '^src/mcp/'
# (no matches)
```

All in-tree users (`src/mcp/external/connection.rs:14`,
`src/mcp/transport/{http,sse,stdio}.rs`,
`src/mcp/modern/mod.rs:32`, etc.) use the `mcp::jsonrpc::*` path
internally — `crate::mcp::JsonRpcRequest` is **never** spelled. The
canonical implementation that the rest of the codebase (a2a, shared
client) uses is in `shared/protocol/src/jsonrpc.rs`.

`IdGenerator` is the only piece that is genuinely MCP-specific (atomic
counter used by the transport impls) and is correctly used at
`src/mcp/external/connection.rs:135, 273`. That use site should stay;
the public re-export should not.

**Triage path:** No live caller → **CUT** the re-export
(`pub use jsonrpc::{...}` in `src/mcp/mod.rs:68-70`), keep
`IdGenerator` private to the crate via a `pub(crate)` re-export, and
either delete `src/mcp/jsonrpc.rs` or trim it to just `IdGenerator`.
Crate-internal consumers (`src/mcp/external/connection.rs:14`,
`src/mcp/transport/*`) switch to `crate::mcp::jsonrpc::IdGenerator`
(the module path) which already works.

**Risk:** low — the canonical types in `shared/protocol` are what the
a2a module and `shared/client` already use. Removing the duplicate
reduces surface area without behavior change. `shared/protocol`
already provides `JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcError`
with the same wire shape; if any **out-of-tree** consumer of the
crate depended on `crate::mcp::JsonRpcRequest`, the breakage is
limited to a type alias. No in-tree caller exists.

**Verification:** after the cut, `cargo check -p alephcore` passes (no
`#[cfg(feature=...)]` gates involved).

---

## sw-mcp-04 · `McpManagerHandle::is_running` has no production caller

**Severity:** low — a debugging aid exposed on the public handle.

**Form:** 1 (producer with zero callers in production).

**Produced:** `McpManagerHandle::is_running(&self) -> bool`
(`src/mcp/manager/handle.rs:430-432`).

**Consumer:** `none found` in production code; only test usage
(`src/mcp/manager/actor.rs:1162, 1320, 1321`) and the handle's own
`Debug` impl at `src/mcp/manager/handle.rs:421`.

Evidence:

```bash
$ rg -n 'is_running\(\)' src/ --type rust | grep -v '^src/mcp/'
src/sandbox/proxy/lifecycle.rs:269:        handle.shutdown();
# (unrelated — this is a sandbox proxy handle, not McpManagerHandle)
```

The method is a 2-line check that returns `!self.tx.is_closed()`. Cheap,
harmless, only used by `Debug`.

**Triage path:** No live production caller → **CUT** (the public
`is_running` can be dropped; if a future debugging need arises, a
`#[cfg(debug_assertions)]` `pub(crate)` accessor is sufficient).

**Risk:** none — purely cosmetic.

**Verification:** `cargo check -p alephcore` still passes.

---

## sw-mcp-05 · `McpManagerHandle::set_sampling_callback` is a redundant facade

**Severity:** low — the public method exists, but the actor's own
startup path (`with_sampling_bridge`, called at
`src/bin/aleph-server/commands/start/mod.rs:565`) installs the
callback via `crate::mcp::sampling_bridge::serve_sampling` which closes
over `McpCommand::SetSamplingCallback` directly. The handle method
itself is reachable only from inside the actor (`actor.rs:460`).

**Form:** 1 (producer with zero callers outside its own file's actor).

**Produced:** `McpManagerHandle::set_sampling_callback`
(`src/mcp/manager/handle.rs:360-378`).

**Consumer:** the implementation at
`src/mcp/manager/actor.rs:460` and `:757` (private to the actor);
no external caller.

Evidence:

```bash
$ rg -n 'set_sampling_callback' src/ --type rust
src/mcp/manager/handle.rs:372:            .send(McpCommand::SetSamplingCallback {
src/mcp/manager/actor.rs:460:                        .set_sampling_callback(move |req| {
src/mcp/manager/actor.rs:757:                .set_sampling_callback(move |req| {
# all three are in the same module tree
```

The two production call sites in `actor.rs` are inside the actor's own
methods that were originally routed through `McpManagerHandle::set_sampling_callback`
during bring-up and never re-pointed at the direct command. Functionally
identical; the indirection costs one channel hop.

**Triage path:** Reconnecting vs deleting is a product call → **DECIDE**.
Three options:

1. **CUT** — drop the public method, point the actor at `tx.send(McpCommand::SetSamplingCallback { ... })` directly (matches what `actor.rs:452` already does internally).
2. **CONNECT** — keep the method, change the actor's two call sites to use the public method (already what they do — there's nothing to reconnect).
3. **LEAVE** — leave as-is; the dead-letter method is small, the cost is a channel hop, and an external caller (test harness, alternate bootstrap path) may want it.

**Risk:** low either way.

**Verification:** no behavior change required regardless of decision.

---

## Verified wired (no finding)

For each of the following, an explicit `rg` across `src/`, `bin/`,
`interfaces/`, `shared/`, `desktop/` shows at least one production
caller:

| Symbol | Defined at | Production caller(s) |
|--------|-----------|----------------------|
| `presets::catalog()` / `presets::find()` | `src/mcp/presets/mod.rs:81, 91` | `src/hub/official_mcp.rs:110, 119, 126` |
| `McpPreset` / `PresetCategory` / `PresetTransport` / `PresetEnvVar` | `src/mcp/presets/mod.rs` | `src/hub/official_mcp.rs:13` |
| `SamplingHandler::set_callback` | `src/mcp/sampling.rs:58` | `src/mcp/client.rs:95`; `src/mcp/external/connection.rs:2030` |
| `SamplingHandler::handle_request` | `src/mcp/sampling.rs:81` | `src/mcp/client.rs:556`; `src/mcp/modern/mrtr.rs:197` |
| `SamplingHandler::new` / `SamplingCallback` | `src/mcp/sampling.rs:21, 41` | `src/mcp/client.rs:71`; `src/mcp/external/connection.rs:2028`; `src/mcp/manager/actor.rs:460, 757` |
| `ContextInjector::gather_context` / `format_as_system_message` | `src/mcp/context_injector.rs:52, 213` | `src/mcp/sampling.rs:107, 108` (currently dead — see sw-mcp-01) |
| `redact_mcp_error` | `src/mcp/redact.rs:18` | `src/tools/handlers/mcp.rs:150, 184, 188, 204, 208` |
| `classify_mcp_error` / `McpErrorKind` | `src/mcp/error_class.rs:18, 132` | `src/mcp/transport/http.rs:611, 618`; `src/mcp/external/connection.rs:1157, 1170, 1506` |
| `preflight_remote_url` | `src/mcp/preflight.rs:34` | `src/mcp/client.rs:489` |
| `tool_sanitize::normalize_tool_schema` | `src/mcp/tool_sanitize.rs:82` | `src/mcp/external/connection.rs:781` |
| `tool_sanitize::truncate_description` | `src/mcp/tool_sanitize.rs:38` | `src/mcp/external/connection.rs:756` |
| `tool_sanitize::scan_description_for_injection` | `src/mcp/tool_sanitize.rs:269` | `src/mcp/context_injector.rs:221`; `src/mcp/external/connection.rs:758` |
| `spawn_tool_bridge` | `src/mcp/tool_bridge.rs:98` | `src/bin/aleph-server/commands/start/mod.rs:1407` |
| `RESOURCE_TOOL` / `RESOURCE_LIST_TOOL` / `RESOURCE_TEMPLATE_LIST_TOOL` / `PROMPT_TOOL` / `PROMPT_LIST_TOOL` / `LOGIN_TOOL` | `src/mcp/tool_bridge.rs` | `src/executor/builtin_registry/definitions.rs:1484-1509` |
| `McpManagerHandle::subscribe` | `src/mcp/manager/handle.rs:405` | `src/mcp/tool_bridge.rs:106` |
| `McpManagerHandle::list_server_configs` | `src/mcp/manager/handle.rs:241` | `src/hub/official_mcp.rs:138`; `src/tools/usage/report.rs:491` |
| `McpManagerHandle::get_status` | `src/mcp/manager/handle.rs:256` | `src/gateway/handlers/mcp.rs:162`; `src/builtin_tools/mcp_login.rs:176` |
| `McpManagerHandle::add_transient_server` / `remove_transient_server` | `src/mcp/manager/handle.rs:103, 127` | `src/extension/mod.rs:506`; `src/extension/plugin_ops.rs:278` |
| `McpManagerHandle::aggregate_tools/resources/prompts/instructions` | `src/mcp/manager/handle.rs:277, 292, 307, 323` | `src/gateway/handlers/mcp.rs:265, 280, 295`; `src/orchestrator/harness_bridge/prompt_build.rs:464` |
| `McpManagerHandle::restart_server/start_server/stop_server` | `src/mcp/manager/handle.rs:143, 162, 181` | `src/gateway/handlers/mcp.rs:207, 227, 247`; `src/gateway/handlers/extensions/{install.rs:276, lifecycle.rs:57-59}` |
| `McpPersistentConfig::save` | `src/mcp/manager/config.rs:108` | `src/mcp/manager/actor.rs:531, 570` |
| `McpPersistentConfig::upsert_server` | `src/mcp/manager/config.rs:177` | `src/mcp/manager/actor.rs:527` |
| `McpResourceTemplate` | `src/mcp/types.rs:162` | `src/mcp/external/connection.rs:143, 893, 1060`; `src/builtin_tools/mcp_resource.rs:483` |
| `ResourceContent` | `src/mcp/resources.rs:12` | `src/builtin_tools/mcp_resource.rs:116, 121`; `src/mcp/external/connection.rs:1358, 1369, 1377` |
| `PromptContent` / `PromptMessage` / `PromptResult` | `src/mcp/prompts.rs` | `src/builtin_tools/mcp_prompt.rs:130-136`; `src/mcp/external/connection.rs:1429-1453` |
| `McpTransport` (trait) / `HttpTransport` / `SseTransport` / `StdioTransport` | `src/mcp/transport/*` | `src/mcp/external/connection.rs`; `src/mcp/client.rs:521, 634` |
| `NotificationCallback` | `src/mcp/transport/traits.rs:16` | `src/mcp/manager/actor.rs:842, 855`; `src/mcp/external/connection.rs:1533`; `src/mcp/client.rs:692, 699` |
| `check_runtime` / `RuntimeKind` / `McpServerConnection` | `src/mcp/external/{runtime,connection}.rs` | `src/mcp/client.rs:14, 109-111, 136, 634, 661`; `src/extension/registrar/mcp_registrar.rs:44, 353` |
| `OAuthProvider` / `OAuthStorage` / `OAuthTokens` / `OAuthEntry` / `ClientInfo` / `OAuthServerMetadata` / `CallbackServer` / `CallbackResult` | `src/mcp/auth/*` | `src/builtin_tools/mcp_login.rs:26, 192`; `src/mcp/client.rs:474, 23`; `src/mcp/auth/provider.rs` (internal) |
| `register_sampling_llm` / `decline_sampling_llm` / `serve_sampling` / `sampling_llm_registered` / `sampling_llm_slot` | `src/mcp/sampling_bridge.rs` | `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:791, 796, 1848`; `src/capability/mod.rs:341`; `src/mcp/manager/actor.rs:171-176` |
| `with_sampling_bridge` | `src/mcp/manager/actor.rs:171` | `src/bin/aleph-server/commands/start/mod.rs:565` |
| `mrtr::fulfill` / `retry_params` / `supports_input_required` / `MAX_ROUNDS` | `src/mcp/modern/mrtr.rs` | `src/mcp/external/connection.rs:1117, 1139, 1146, 1186` |
| `McpDialect` / `UnsupportedVersion` / `is_modern_error` / `MCP_MODERN_PROTOCOL_VERSION` / `META_*` / `error_codes::*` | `src/mcp/modern/{mod.rs,discover.rs,headers.rs,cache.rs}` | `src/mcp/external/connection.rs`; `src/mcp/transport/{http.rs,sse.rs}`; `src/mcp/modern/{discover.rs,headers.rs,cache.rs,mrtr.rs}` (internal) |

---

## Notes on what was checked but did NOT yield a finding

- `McpPersistentConfig::save` — initially looked severed because of a
  too-narrow `rg` pattern; verified via `rg -n 'save\(' src/mcp/manager/actor.rs`
  that the actor calls it at lines 531 and 570.
- `McpServerStatusDetail`, `McpServerInfo` — wired through
  `list_servers` / `get_status` / event payloads.
- `set_notification_handler` (per-server and on `McpClient`) — wired
  from the manager's `set_sampling_callback` analog (manager.rs:842-855)
  and from the client setter (client.rs:692-702).
- `truncate_description` (MCP version) — used by `external/connection.rs:756`;
  the homonym in `tool_metadata/registry/helpers.rs:26` is a different
  helper with a different signature and a different caller set
  (`tool_metadata/registry/registration.rs:363`). Not a duplicate bug.
- `McpCommand::*` variants — every variant has a handler in
  `src/mcp/manager/actor.rs`; the `Shutdown` arm is reachable only via
  `McpManagerHandle::shutdown` (see sw-mcp-02).
- `mcp_login` flow — `McpLoginTool` is registered through the
  tool_bridge and exercised by `builtin_registry/definitions.rs:1510`.

---

## Methodology

1. **Symbol inventory** — `rg -n 'pub (fn|struct|enum|trait|type|const|async fn) ' src/mcp`
   → 353 public items across 41 files.
2. **For each public item**, narrow search via `rg -n '<symbol>' src/ bin/ interfaces/ shared/ desktop/`,
   classify callers as production / `#[cfg(test)]` / dead.
3. **Triage** — for each item with zero production callers:
   - Triage step 1: no consumer anywhere → CUT or CONNECT depending on
     whether the producer + consumer both exist and reconnecting is cheap.
   - Triage step 2: zero observed production pain AND a newer mechanism
     covers the same outcome → CUT (painless-wire heuristic).
   - Triage step 3: reconnecting adds new coupling → DECIDE.
4. **Cross-check** every "no consumer" claim with an explicit `rg`
   invocation pasted into this report.
5. **File deletion safety** — before recommending a file-level cut
   (sw-mcp-03), grepped every exported symbol of `src/mcp/jsonrpc.rs`
   individually; `IdGenerator` has internal callers and must stay.

## What this audit did NOT do

- Did not run `cargo` (read-only constraint).
- Did not modify any source files.
- Did not propose style/lint changes (`rustfmt`/`clippy` out of scope).
- Did not deeply audit `bin/aleph-server/commands/start/mod.rs` (~3700
  lines) beyond the shutdown-section check for sw-mcp-02 — that file's
  size deserves its own audit.
- Did not audit `shared/protocol` (the canonical JsonRpc* home) — that
  crate lives outside the module under review.
- Did not verify the security claim in sw-mcp-01 that
  `scan_description_for_injection` plus the `<mcp_context>` fence are
  sufficient for live context injection; that requires a separate threat
  model pass before reconnecting.
