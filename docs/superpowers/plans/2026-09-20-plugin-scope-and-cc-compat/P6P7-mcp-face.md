# Plan — P6 MCP server face + `packages/pi-aleph/` · P7 `qa/mcp_face/run.sh`

Base: `3ddc1f2e7` (+ `35e5f8bca` spec). Worktree: `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/plugin-scope-round`.
Spec sections covered: §3.7 (MCP face), §3.8 (`pi-aleph`), §3.5 G5, §4 MCP error rows, §6 `qa/mcp_face`.

## Facts established from the worktree (every task below cites these)

| # | Fact | Where (3ddc1f2e7) |
|---|---|---|
| F1 | The gateway HTTP listener is **axum 0.8** (`axum = { version = "0.8", features = ["ws"] }`, `axum-server 0.7` for TLS). One `Router` is built by `GatewayServer::build_router` and routes are added by `.merge(<sub-router>)`; sub-routers carry a **narrow** state (not `GatewaySharedState`) — the artifact byte route is the model to copy. SSE is already used by the A2A route (`axum::response::sse::{Event, KeepAlive, Sse}`). | `Cargo.toml:316-317` · `src/gateway/server/mod.rs:781-947` (`build_router`; `.merge(openai)` :917, artifacts :920-922, canvas :924-926, a2a :928-932, webhook :942-944) · `src/gateway/server/artifact_route.rs:130-192` (`ArtifactRouteState`, `artifact_routes`) · `src/a2a/adapter/server/routes.rs:9,295-297` |
| F2 | Route-level guards a sibling HTTP route copies from `/ws`: `trusted_proxy::resolve_client(peer, headers, enabled, trusted_ips) -> ResolvedClient { ip, secure, local }` — **`local` is the authority bit, never `ip.is_loopback()`**; `server::handler::refuse_insecure_remote(client_is_local, secure, allow_insecure_remote)` (426 for plaintext remote); `OriginPolicy::is_allowed(origin, host)`. In-process route tests insert `ConnectInfo(SocketAddr)` into request extensions and drive the router with `tower::ServiceExt::oneshot`. | `src/gateway/trusted_proxy.rs:17-77` · `src/gateway/server/handler.rs:82-88` (`pub(super)` — widen to `pub(crate)`) · `src/gateway/server/artifact_route.rs:279-320, 462-545` (fixture) |
| F3 | `connect` auth = `handlers::connect::resolve_connect_auth(is_loopback, shared_token, device_token, bootstrap_ticket, device_id, device_name, validate_shared_token, &DeviceTokenManager) -> ConnectAuthOutcome` (loopback ⇒ Authorized; device token ⇒ Authorized{device_id}; shared token ⇒ Authorized{None}); the `(user, role)` pair = `resolve_connection_identity(is_loopback, device_id, &SecurityStore) -> (Option<String>, &'static str)` (`"operator"` / `"member"` / `"guest"`, fail-closed). Production's shared-token validator closure is `SharedTokenManager::global().map(|m| m.validate(t).unwrap_or(false)).unwrap_or(false)`. Bearer parsing already exists: `openai_api::auth::extract_bearer_token`. The WS dispatch scopes `CALLER_USER / CALLER_ROLE / CALLER_IS_LOOPBACK / CALLER_CONN_ID` + `scope::with_scope(personal(user))` around every request in `dispatch_with_caller_context` — the gateway `CLAUDE.md` 地雷 says a new dispatch path must reuse that nest, not re-implement it. | `src/gateway/handlers/connect.rs:74-135, 168-200` · `src/gateway/server/handler.rs:1271-1284` (validator closure), `:2075-2099` (`dispatch_with_caller_context`) · `src/gateway/openai_api/auth.rs:10-25` · `src/gateway/CLAUDE.md:80-93` |
| F4 | MCP wire types. **Envelope**: the gateway's own `protocol::{JsonRpcRequest { jsonrpc, method, params: Option<Value>, id: Option<Value> }, JsonRpcResponse::{success, error}}` + codes `PARSE_ERROR -32700 / INVALID_REQUEST -32600 / METHOD_NOT_FOUND -32601 / INVALID_PARAMS -32602 / INTERNAL_ERROR -32603` (re-exported from `aleph_protocol`). The client-side `src/mcp/jsonrpc.rs::JsonRpcRequest` has `id: u64` and **cannot echo a string id**, so it is not the server envelope. **Payloads** from `src/mcp/protocol.rs`: `InitializeParams { protocol_version, capabilities, client_info: ClientInfo { name, version } }`, `InitializeResult { protocol_version, capabilities: ServerCapabilities { tools: Option<ToolCapability { list_changed }>, .. }, server_info: Option<ServerInfo { name, version }>, instructions }`, `ToolDefinition { name, description, input_schema, annotations: Option<ToolAnnotations> }`, `ToolsListResult { tools, next_cursor }`, `ToolCallParams { name, arguments }`, `ToolCallResult { content: Vec<ToolResultContent>, is_error }`, `ToolResultContent::{Text{text}, Image{data, mime_type}, ..}`. **Version literals**: `MCP_LEGACY_PROTOCOL_VERSION = "2025-03-26"` (`protocol.rs:658`); the newest literal in `src/mcp/` is `modern::MCP_MODERN_PROTOCOL_VERSION = "2026-07-28"` (`modern/mod.rs:39`) — a **handshake-less, session-less** revision (`server/discover` + per-request `_meta`, no `Mcp-Session-Id`). The newest *handshake* revision the client stack names is `2025-11-25` (`modern/mod.rs:15`: "`2025-11-25` and earlier speak the handshake"); `2025-06-18` is named at `protocol.rs:245`. | `src/gateway/protocol.rs:10-35, 66-100, 123-160` · `shared/protocol/src/jsonrpc.rs:13-21` · `src/mcp/protocol.rs:12-115, 120-135, 194-205, 210-215, 227-292, 658` · `src/mcp/modern/mod.rs:1-16, 39` |
| F5 | `tools.invoke` (`handlers/tools_invoke.rs:74-291`) dispatches **straight off the raw `ToolRegistry`** with four hand-written hard floors and never enters `ScopedToolService` — its own doc says "nothing on this surface gets exec-tier approval, `tool_permissions`, hooks, or the operation ledger". The scoped path the run loop uses is `execution_engine::build_request_tool_service(tool_registry: Arc<LoopToolRegistry>, allowed: BTreeSet<String>, subagent: None, turn_context, hook_executor, session_id, tool_permissions, exec_tier, unattended, core_tools, truncate, deferred, tool_health) -> Arc<dyn ToolService>`; the per-request `LoopToolRegistry` is `tools::adapters::build_registry_from_tools(Arc<R: ToolRegistry>, &[UnifiedTool])` + MCP-bridged tools joined from `tool_service_builder::mcp_tool_registry().snapshot()` via `McpRegistryTool::from_registry_entry` + plugin tools from `ExtensionManager::active_plugin_tools_snapshot()` through `tool_refresh::plugin_tool_to_unified_tool`. `ToolService::{list() -> Vec<tools::service::ToolDefinition { name, description, input_schema, metadata { idempotent, .. } }>, execute_with_cancel(name, input, CancellationToken) -> Result<ToolOutput { value, metadata { images } }, ToolError>}`; `ToolError::NotFound { name }` for a name outside the registry. | `src/gateway/handlers/tools_invoke.rs:1-19, 33-57` · `src/gateway/execution_engine/tool_service_builder.rs:153-233` · `src/gateway/execution_engine/run_loop/inner.rs:214-236, 762-822, 962-976` · `src/tools/adapters/registry_adapter.rs:20-34, 377, 538-578` · `src/gateway/execution_engine/tool_refresh.rs:11-24` · `src/tools/service.rs:13-60, 160-180` · `src/session/events.rs:190-230` |
| F6 | **Approval when no operator UI is present.** The confirm gate (`requires_confirmation` ∪ `Ask`-tier) routes through `CONFIRMATION_REQUESTER` = `FallbackApprovalRequester(channel → OperatorApprovalRequester)`. `unattended: true` on the `TurnContext`/service fails closed instantly with `ApprovalOutcome::Unavailable` (`dispatch.rs:813-843`; the model-facing text is `"running `<tool>` was not authorized — nobody was asked (Unavailable). … Do not retry …"` built by `ConfirmDenial::lead`, `dispatch.rs:130-137`). `unattended: false` raises a card with **no deadline** (`approval::approval_timeout_for_current_turn`, ruled 2026-08-28) that parks until an operator answers. `OperatorApprovalRequester`'s `Ok(0)`-subscribers auto-deny is **not** an operator-presence check: the boot path subscribes 17 internal consumers to the event bus, so `publish_frame` returns `Ok(n>0)` on every real server. Operator presence is observable from the connection table: `ConnectionState { first_message, caller_role: String, .. }` in `GatewayServer.connections: Arc<RwLock<HashMap<String, ConnectionState>>>` (tokio `RwLock`). | `src/bin/aleph-server/commands/start/mod.rs:3418-3478` · `src/tools/scoped/dispatch.rs:130-137, 560-614, 781-843` · `src/approval/mod.rs:141-162` · `src/approval/operator_requester.rs:176-306` · `src/gateway/event_bus.rs:402-462` · `src/gateway/server/mod.rs:41-84, 775-777` |
| F7 | **Principals are not an enum.** Identity = the `(user, role)` pair from F3, carried as task-locals (`caller_identity::{CALLER_ROLE, CALLER_USER, CALLER_IS_LOOPBACK, CALLER_CONN_ID}`) plus `TurnContext.caller_role`. The spend ledger's `spend::Principal { User(String), Unattributed }` is a `users.user_id` and is derived from `scope::current_room_author().or(ambient_owner())` — i.e. from the `with_scope(personal(user))` nest — never from an agent or client name. `identity::actor` is a ledger-actor override keyed by **agent id** (sub-agents), not a client. So `McpClient { client_name }` is a record on the MCP session (logs, `hook_session_id`, approval `command` text), while every gate and the ledger see the authenticated user/role exactly as a WS connection with the same credential would. | `src/gateway/caller_identity.rs:1-30, 33-90` · `src/spend/mod.rs:50-115, 690-745` · `src/identity/actor.rs:1-35` · `src/tools/turn_context.rs:21-68, 95-116` |
| F8 | **Process-global handle pattern** = `capability::CapabilitySlot<T>` (`install` stamps outcome; `decline(because)`; `get`), a `pub(crate) const fn <name>_slot() -> &'static dyn SlotStatus` accessor, and a row in `capability::ALL_SLOTS`; `capability::census` fails if a new bare `OnceLock` install-once static appears or a declared slot is missing from the roster. `spend::install_ledger` and `extension::manager_global::{init_extension_manager, try_extension_manager}` are the two named models. Boot's `start/mod.rs` has a source-level census pinning production calls to `install_policy(`/`install_ledger(` (`:3961-3999`) — copy its shape for `install_mcp_face(`. | `src/capability/mod.rs:1-160, 290-350` · `src/spend/mod.rs:541-585` · `src/extension/manager_global.rs:1-70` · `src/bin/aleph-server/commands/start/mod.rs:3955-3999` |
| F9 | **Tool catalogue for `expose ⊆ catalogue`**: `tools.catalog` (`handlers/tools_visibility.rs:135`) lists `ToolCatalog::list_all()` rows — builtin rows there carry **no `parameters_schema`** (`tool_catalog_init.rs:533-556` builds them from `BUILTIN_TOOL_DEFINITIONS` name+description only). Schemas live on the **executor** side: `BuiltinToolRegistry::unified_tools() -> impl Iterator<Item = &UnifiedTool>` ("the authoritative answer to what this registry can execute right now, with full parameter schemas") and `get_tool_schema(name)`. The compile-time name set is `executor::BUILTIN_TOOL_DEFINITIONS` (168 entries at 3ddc1f2e7). The single source of "side-effect-free builtin" is `tools::adapters::registry_adapter::READ_ONLY_TOOLS` (`pub(crate)`, 56 names; also the source of idempotency and the `Ask`-tier read exemption); operator-tier names are `gateway::method_authz::tool_requires_operator` (`OPERATOR_TOOLS`, includes `terminal`). | `src/gateway/handlers/tools_visibility.rs:132-150` · `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:25-51, 525-556` · `src/executor/builtin_registry/registry/inherent.rs:304-332` · `src/executor/builtin_registry/definitions.rs:68-81` · `src/tools/adapters/registry_adapter.rs:53-216` · `src/gateway/method_authz.rs:31-168` |
| F10 | Boot wiring points: `agent_result.tool_registry: Option<Arc<BuiltinToolRegistry>>` and `agent_result.tool_catalog: Option<Arc<ToolCatalog>>` (→ `.health()` is the shared `ToolHealthCache`) come back from `register_agent_handlers` (`start/mod.rs:1393`); `device_token_mgr` is built at `:563-566`; `auth_bundle.security_store` at `:834`; `app_config: Arc<tokio::sync::RwLock<Config>>` at `:1023`; `server.set_canvas_store(...)` at `:1390` is the model for a `set_*` that `build_router` later reads; `Mode: Simulated` (no API key) leaves `agent_result.tool_registry == None`. Default gateway port is the literal `18790` in `GatewayServerConfig::default()` — no named constant. Config sections are `#[serde(default)]` fields on `config::structs::Config` deriving `JsonSchema`; sub-configs derive `Debug, Clone, Serialize, Deserialize, JsonSchema`; unknown sections are `ReloadImpact::Restart` by default. | `src/bin/aleph-server/commands/start/mod.rs:558-566, 834, 1023, 1385-1391, 1393-1419, 1989` · `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:47-70` · `src/gateway/config.rs:248-252` · `src/config/structs.rs:50-58, 170-178` · `src/config/reload_impact.rs:283-300` |
| F11 | QA conventions: bash `run.sh <stage>` + python3 drivers over `websockets` (`qa/browser_managed/qa_rpc.py::{Ledger, Rpc}`), `qa/lib/{build.sh,scratch_home.sh}` (`qa_build`, `qa_redirect_home`), config generated by a `--port` boot then patched with `qa/busy_input/patch_config.py --gateway-port --mock-port` (dummy provider key ⇒ `Mode: Real`, so `tools.invoke` works), `[PASS]/[FAIL]/SKIP` lines, exit code = failure count, LAN-IP recipe from `qa/spend_budget/run.sh:130-150` (+ `qa/spend_budget/patch_config.py:92-93` sets `[gateway] host="0.0.0.0"`, `allow_insecure_remote=true`). Plugin fixtures are planted under `$ALEPH_HOME/plugins/installed/<id>/` (`qa/plugins/plant_plugins.py`); an `aleph.plugin.toml` with `kind = "static"` + `[[tools]] name/handler` registers a `ToolRegistration` (`declared_sections.rs:116-135`). The plugin toggle tool face is `plugin_manage { action: "enable"|"disable", name }` over `tools.invoke` (`qa/plugins/drive_tool_face.py:41-42`, `plugin_manage.rs:218-231`). The shared gateway token is readable over loopback via `gateway.token.current` (Admin class) / `gateway.token.rotate`. | `qa/README.md:1-40, 610-640` · `qa/plugins/run.sh:1-140` · `qa/spend_budget/run.sh:118-160` · `src/extension/validation.rs:240-262` · `src/gateway/handlers/gateway_token.rs:20-30` |
| F12 | Evidence constraints: dsh mounts one row `{ serverName, transport: 'streamable-http', url, headers }`, client capabilities `{}`, honours only `notifications/tools/list_changed`, names tools `mcp__<server>__<tool>` ≤ 64 chars, treats `isError: true` as a failed tool result, 60 s per-call timeout (`scan-dsh-cordis.md` §7.1–7.3, 7.6). pi has no MCP client; `pi-mcp-adapter` reads `.mcp.json` / `~/.pi/agent/mcp.json` / `package.json#pi.mcp`, negotiates **`2025-03-26` by default** (legacy handshake; supports `2025-11-25, 2025-06-18, 2025-03-26, 2024-11-05`), strips `$schema` / `additionalProperties`, never injects `instructions`, 30 s request timeout, identifies as `{name: "pi-mcp-<server>"}`; a JS-free package is `package.json#pi = { skills: string[], mcp: "./mcp.json" }` (`PiManifest.skills?: string[]` — an **array**; the `mcp` key is the adapter's, and servers loaded that way are prefixed `pi_aleph__aleph`, so the README tells users to prefer `~/.pi/agent/mcp.json` for the bare `aleph` prefix) (`scan-pi.md` §1.3, §9.1, §9.2). Claude Code `.mcp.json` row: `{"mcpServers":{"aleph":{"type":"http","url":…,"headers":{…}}}}`. | evidence files as cited |

Longest default-exposed name is `hub_catalog_search` (18) ⇒ `mcp__aleph__hub_catalog_search` = 30 < 64: dsh's cap is satisfied without hashing (§3.7 名字 row). A unit test pins every default name ≤ 51 chars (dsh's truncation threshold).

## Design decisions the facts forced (each is also listed under Contract deltas)

1. **Envelope** = `gateway::protocol::{JsonRpcRequest, JsonRpcResponse}` (arbitrary ids), **payloads** = `mcp::protocol::*`. Nothing is copied (spec §3.7, 判据 §1).
2. **Versions**: `SUPPORTED_PROTOCOL_VERSIONS = ["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION]` (newest first; the oldest literal is the client stack's own constant). `2026-07-28` is out of scope (no `initialize`, no sessions — Open question 1).
3. **Identity**: per request, `authorize()` = `resolve_connect_auth` (the bearer is passed as both the shared token and the device token; no ticket) + `resolve_connection_identity` → `McpCaller { role, user, is_local, device_id }`; the tool call runs inside the same task-local nest the WS dispatch uses, extracted into `caller_identity::with_caller_identity`.
4. **Dispatch** = `build_request_tool_service` over a registry built from `expose ∩ (static builtins ∪ live plugin tools ∪ MCP-bridged tools)`; `unattended = !operator_online()` where `operator_online` is a probe over the gateway connection table (F6) — this is what makes "no operator UI ⇒ deny" true instead of a card that parks forever.
5. **Face handle** = one `CapabilitySlot<Arc<McpFace>>` (`FailsClosed`), rostered; `try_mcp_face()` / `install_mcp_face()` / `decline_mcp_face()`.
6. `[mcp_server].expose` applies **live** (P6.9: `LIVE_SUBSECTIONS` gains `"mcp_server.expose"`, the face swaps an `ArcSwap<BTreeSet<String>>`, re-runs the G5 runtime check and broadcasts `list_changed`); `[mcp_server].enabled` stays `Restart` (the route is mounted at boot). Lifecycle transitions broadcast through the same `notify_tools_list_changed`, called from P1's `after_transition` — and **that one line is P6's** (R6.1, P6.4).
7. **Default `expose`** is *derived*, not listed: `READ_ONLY_TOOLS ∩ BUILTIN_TOOL_DEFINITIONS − OPERATOR_TOOLS − desktop_* − DEFAULT_EXPOSE_EXCLUDES` where `DEFAULT_EXPOSE_EXCLUDES` names `tool_usage`, `config_audit`, `node_list`, `user_profile` each with a reason (R6.6) — 36 names at 3ddc1f2e7, each named in P6.1's pin test. The pin makes admitting a new read-only tool to the default exposure a conscious act; a second test makes a stale exclusion (one the predicate would no longer admit anyway) red.
8. **Attended approval cards keep the dangling-card shape** (user ruling U-d): an MCP call raised to a connected operator parks with no deadline; if the client times out first (dsh 60 s / pi-mcp-adapter 30 s) the card stays pending and, once answered, nothing runs. Documented in P6.4's module doc and P6.8's README; "retire the card when the handler future drops" is a recorded follow-up, not built.

Phase order: P1 precedes P6, so every anchor below that touches `src/extension/` is against **plan-P1.md's code** (P1.9 `lifecycle.rs::after_transition`, P1.14's module-level `projection.rs::tests::G3_PINNED` slice), with the `3ddc1f2e7` cite kept in parentheses where an old line survives.

---
### Task P6.1: `McpServerConfig` — `[mcp_server]` section with a derived default exposure (G5, unit half)

**Files:**
- Create: `src/gateway/mcp_face/mod.rs` (skeleton — grows in P6.4)
- Create: `src/gateway/mcp_face/config.rs`
- Modify: `src/gateway/mod.rs:113-114` (add `pub mod mcp_face;` between `pub mod method_visibility;` and `pub mod openai_api;`)
- Modify: `src/config/structs.rs:170-172` (new field after `a2a`) and `src/config/structs.rs:397` (manual `Default`)
- Test: `src/gateway/mcp_face/config.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::tools::adapters::registry_adapter::READ_ONLY_TOOLS` (`pub(crate) const &[&str]`, F9), `crate::executor::BUILTIN_TOOL_DEFINITIONS` (F9), `crate::gateway::method_authz::tool_requires_operator(&str) -> bool` (F9).
- Produces:
  ```rust
  pub struct McpServerConfig { pub enabled: bool, pub expose: Vec<String> }   // impl Default: enabled=true, expose=default_expose()
  pub const DEFAULT_EXPOSE_EXCLUDES: &[(&str, &str /* reason */)];   // tool_usage, config_audit, node_list, user_profile
  pub fn default_expose() -> Vec<String>;
  pub fn is_default_exposable(name: &str) -> bool;
  pub fn unknown_expose_names<'a>(expose: &'a [String], known: &BTreeSet<String>) -> Vec<&'a str>;
  // Config::mcp_server: McpServerConfig  (config key `[mcp_server]`)
  ```

- [ ] **Step 1: Write the failing tests**

```rust
// src/gateway/mcp_face/config.rs — bottom of the file
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The default exposure at 3ddc1f2e7, spelled out once so that admitting a
    /// tool to the MCP default surface is a conscious edit here, not a side
    /// effect of adding it to `READ_ONLY_TOOLS`. A red here means one of the
    /// four sources moved (`READ_ONLY_TOOLS`, `BUILTIN_TOOL_DEFINITIONS`,
    /// `OPERATOR_TOOLS`, `DEFAULT_EXPOSE_EXCLUDES`) — decide, then update this list.
    const PINNED_DEFAULT: &[&str] = &[
        "agent_info", "agent_list", "list_models", "read_config_guide",
        "search", "web_fetch", "ctx_search", "document_extract", "file_read",
        "grep", "find", "memory_search", "memory_browse", "memory_explore", "memory_timeline",
        "memory_trace", "recall_context", "recall_events", "governance_metrics",
        "session_list", "session_read", "session_search", "task_list", "task_read_artifact",
        "team_status", "team_digest", "team_usage", "heartbeat_list", "skill_list",
        "skill_read", "skill_status", "note_orient", "note_graph_query", "hub_catalog_search",
        "hub_resolve_spec", "hub_fetch_docs",
    ];

    #[test]
    fn the_default_exposure_is_exactly_the_pinned_list() {
        let derived = default_expose();
        let pinned: Vec<String> = PINNED_DEFAULT.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(derived, pinned, "default_expose() drifted from the pinned list");
    }

    #[test]
    fn every_default_name_is_a_compile_time_builtin() {
        // G5, unit half: the default can never name a tool the executor does
        // not know. The runtime (host-dependent) half is `unknown_expose_names`.
        for name in default_expose() {
            assert!(
                crate::executor::BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == name),
                "{name} is not in BUILTIN_TOOL_DEFINITIONS"
            );
        }
    }

    #[test]
    fn the_default_never_exposes_an_operator_tier_desktop_or_excluded_tool() {
        for name in default_expose() {
            assert!(!crate::gateway::method_authz::tool_requires_operator(&name), "{name}");
            assert!(!name.starts_with("desktop_"), "{name}");
            assert!(
                !DEFAULT_EXPOSE_EXCLUDES.iter().any(|(n, _)| *n == name),
                "{name} is on the exclusion list and still exposed"
            );
        }
    }

    /// A by-name exclusion is a list, and a list rots: an entry the predicate
    /// would no longer admit anyway (renamed tool, moved to OPERATOR_TOOLS,
    /// dropped from READ_ONLY_TOOLS) is a stale line that reads like a live
    /// decision. Every exclusion must be one the predicate WOULD admit
    /// without it, and must carry a reason.
    #[test]
    fn every_exclusion_is_load_bearing_and_reasoned() {
        for (name, reason) in DEFAULT_EXPOSE_EXCLUDES {
            assert!(!reason.trim().is_empty(), "{name}: exclusion without a reason");
            assert!(
                crate::tools::adapters::registry_adapter::READ_ONLY_TOOLS.contains(name),
                "{name} is not read-only; the predicate already rejects it — delete the exclusion"
            );
            assert!(
                crate::executor::BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == *name),
                "{name} is not a compile-time builtin; the predicate already rejects it"
            );
            assert!(
                !crate::gateway::method_authz::tool_requires_operator(name),
                "{name} is operator-tier; the predicate already rejects it"
            );
            assert!(!name.starts_with("desktop_"), "{name}: the desktop_ prefix rule already covers it");
        }
    }

    #[test]
    fn default_names_stay_under_dshs_truncation_threshold() {
        // dsh: `mcp__<server>__<raw>` > 64 chars is truncated to 51 + hash.
        // `mcp__aleph__` is 12 chars, so raw names ≤ 52 never hash; pin 51.
        for name in default_expose() {
            assert!(name.len() <= 51, "{name} would be hashed by dsh");
        }
    }

    #[test]
    fn unknown_expose_names_reports_exactly_the_strangers() {
        let known: BTreeSet<String> = ["grep", "file_read"].iter().map(|s| (*s).to_string()).collect();
        let expose = vec!["grep".to_string(), "no_such_tool".to_string(), "file_read".to_string()];
        assert_eq!(unknown_expose_names(&expose, &known), vec!["no_such_tool"]);
        assert!(unknown_expose_names(&[], &known).is_empty());
    }

    #[test]
    fn the_section_round_trips_and_defaults() {
        let parsed: McpServerConfig = toml::from_str("").unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.expose, default_expose());

        let parsed: McpServerConfig =
            toml::from_str("enabled = false\nexpose = [\"grep\"]\n").unwrap();
        assert!(!parsed.enabled);
        assert_eq!(parsed.expose, vec!["grep".to_string()]);

        let text = toml::to_string(&McpServerConfig::default()).unwrap();
        let back: McpServerConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, McpServerConfig::default());
    }

    #[test]
    fn the_main_config_carries_the_section_under_mcp_server() {
        let cfg: crate::Config = toml::from_str("[mcp_server]\nexpose = [\"grep\"]\n").unwrap();
        assert!(cfg.mcp_server.enabled);
        assert_eq!(cfg.mcp_server.expose, vec!["grep".to_string()]);
        let cfg: crate::Config = toml::from_str("").unwrap();
        assert_eq!(cfg.mcp_server, McpServerConfig::default());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::mcp_face::config -- --nocapture`
Expected: FAIL to compile — `error[E0433]: failed to resolve: could not find `mcp_face` in `gateway`` (module does not exist yet).

- [ ] **Step 3: Write the implementation**

`src/gateway/mcp_face/mod.rs` (skeleton; P6.4 replaces the body):

```rust
//! Aleph as an MCP **server** (Streamable HTTP, `/mcp`) — spec §3.7.
//!
//! An interface face (R4): it translates `tools/list` / `tools/call` into the
//! same scoped tool dispatch a chat turn uses and nothing else. Wire payloads
//! come from `crate::mcp::protocol`; the JSON-RPC envelope is the gateway's
//! own `crate::gateway::protocol`. Nothing here is a second MCP implementation
//! (CLAUDE.md 禁用清单).

pub mod config;
```

`src/gateway/mcp_face/config.rs`:

```rust
//! `[mcp_server]` — what the MCP face exposes, and whether it is mounted.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[mcp_server]` in `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServerConfig {
    /// Mount `POST/GET/DELETE /mcp` on the gateway listener.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Tool names an MCP client may list and call. A whitelist: a name that
    /// is not here is "unknown tool" on the wire, whatever the caller's role.
    /// Absent ⇒ [`default_expose`] (side-effect-free builtins only).
    /// Applies live (`ReloadImpact` subsection `mcp_server.expose`, P6.9);
    /// `enabled` does not — the route is mounted at boot.
    #[serde(default = "default_expose")]
    pub expose: Vec<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            expose: default_expose(),
        }
    }
}

const fn default_enabled() -> bool {
    true
}

/// Read-only builtins the predicate WOULD admit and the default nonetheless
/// withholds, each with the reason a foreign agent should not get it unasked.
/// A rule (`is_default_exposable`) decides everything else; this list is only
/// for names the rule cannot tell apart from their neighbours. Every entry is
/// checked by `every_exclusion_is_load_bearing_and_reasoned`: a name the rule
/// already rejects is a stale line and turns that test red.
pub const DEFAULT_EXPOSE_EXCLUDES: &[(&str, &str)] = &[
    (
        "tool_usage",
        "has a `forget_orphans: true` write arm; its READ_ONLY_TOOLS entry says so",
    ),
    (
        "config_audit",
        "discloses this server's own configuration (provider names, key presence, \
         gateway posture) to whatever agent mounts Aleph",
    ),
    (
        "node_list",
        "discloses the cluster's machines; fleet reads are for the operator's own agents, \
         not a default for a foreign one",
    ),
    (
        "user_profile",
        "discloses the operator's personal profile; a foreign agent gets it only when the \
         operator adds it to `expose`",
    ),
];

/// The default exposure, DERIVED from the tables that already answer "is this
/// tool side-effect-free / operator-only / a builtin", minus the reasoned
/// by-name exclusions:
/// `READ_ONLY_TOOLS ∩ BUILTIN_TOOL_DEFINITIONS − OPERATOR_TOOLS − desktop_* − DEFAULT_EXPOSE_EXCLUDES`.
///
/// Why each subtraction: operator-tier names (`terminal`) are gated on a card
/// this face cannot always raise; `desktop_*` are registered only where a
/// desktop bridge exists AND disclose the host's live UI tree to a foreign
/// agent; [`DEFAULT_EXPOSE_EXCLUDES`] carries its own reasons. Tools outside
/// `BUILTIN_TOOL_DEFINITIONS` (`tool_search`, `get_tool_schema`, `mcp_*`,
/// `channel_directory`) are registered per request or conditionally and are
/// not a compile-time fact.
///
/// Pinned name-by-name in this module's tests: growing `READ_ONLY_TOOLS`
/// turns that test red on purpose, so admission here is a decision.
#[must_use]
pub fn default_expose() -> Vec<String> {
    crate::tools::adapters::registry_adapter::READ_ONLY_TOOLS
        .iter()
        .copied()
        .filter(|name| is_default_exposable(name))
        .map(str::to_string)
        .collect()
}

/// The predicate behind [`default_expose`]. Public so the doctor / tests can
/// explain WHY a name is absent from the default.
#[must_use]
pub fn is_default_exposable(name: &str) -> bool {
    crate::executor::BUILTIN_TOOL_DEFINITIONS
        .iter()
        .any(|d| d.name == name)
        && !crate::gateway::method_authz::tool_requires_operator(name)
        && !name.starts_with("desktop_")
        && !DEFAULT_EXPOSE_EXCLUDES.iter().any(|(n, _)| *n == name)
}

/// G5's runtime half: every configured name that the live catalogue does not
/// know. `known` is whatever the caller can execute right now (builtins +
/// active plugin tools + bridged MCP tools at boot). Empty means "all good";
/// the caller decides between a warning and a refusal.
#[must_use]
pub fn unknown_expose_names<'a>(expose: &'a [String], known: &BTreeSet<String>) -> Vec<&'a str> {
    expose
        .iter()
        .map(String::as_str)
        .filter(|name| !known.contains(*name))
        .collect()
}
```

`src/gateway/mod.rs` — insert between the two existing lines:

```rust
pub mod method_visibility;
pub mod mcp_face;
pub mod openai_api;
```

`src/config/structs.rs:170-172` — after the `a2a` field add:

```rust
    /// Aleph-as-MCP-server face (`POST/GET/DELETE /mcp`) — spec §3.7.
    /// Always emitted so the operator can read the exposure whitelist off the
    /// file. `expose` applies live (`mcp_server.expose` is a live subsection);
    /// `enabled` needs a restart.
    #[serde(default)]
    pub mcp_server: crate::gateway::mcp_face::config::McpServerConfig,
```

`src/config/structs.rs:397` — after `a2a: crate::a2a::config::A2AConfig::default(),` add:

```rust
            mcp_server: crate::gateway::mcp_face::config::McpServerConfig::default(),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::mcp_face::config`
Expected: PASS (8 tests). Also: `cargo test -p alephcore --lib config::` (the `Config` serialization tests still pass with the new always-emitted section).

- [ ] **Step 5: Mutation step (G5 unit half + the pin + the exclusion list)**

1. In `is_default_exposable` delete `&& !name.starts_with("desktop_")` → run `cargo test -p alephcore --lib gateway::mcp_face::config` → Expected red: `the_default_exposure_is_exactly_the_pinned_list` **and** `the_default_never_exposes_an_operator_tier_desktop_or_excluded_tool`. Revert.
2. Delete the `user_profile` row from `DEFAULT_EXPOSE_EXCLUDES` → Expected red: `the_default_exposure_is_exactly_the_pinned_list` (the pin does not list it). Revert.
3. Add a stale row `("terminal", "x")` to `DEFAULT_EXPOSE_EXCLUDES` → Expected red: `every_exclusion_is_load_bearing_and_reasoned` (`terminal` is operator-tier; the predicate already rejects it). Revert.
4. In `unknown_expose_names` change `!known.contains(*name)` to `known.contains(*name)` → Expected red: `unknown_expose_names_reports_exactly_the_strangers`. Revert.

Record both red names in the commit message.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/mcp_face/mod.rs src/gateway/mcp_face/config.rs src/gateway/mod.rs src/config/structs.rs
git commit -m "mcp_face: [mcp_server] config with a derived read-only default exposure

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.2: MCP session table — `Mcp-Session-Id` → Aleph session key + `McpClient`

**Files:**
- Create: `src/gateway/mcp_face/session.rs`
- Modify: `src/gateway/mcp_face/mod.rs` (add `pub mod session;`)
- Test: `src/gateway/mcp_face/session.rs`

**Interfaces:**
- Consumes: `crate::routing::session_key::SessionKey::task(agent_id, task_type, task_id)` (`src/routing/session_key.rs:246`; `"mcp"` is not a reserved marker — the reserved set is `peer|dm|subagent|ephemeral`), `crate::gateway::protocol::JsonRpcRequest` (the SSE notification envelope), `crate::sync_primitives::Mutex`, `tokio::sync::mpsc`.
- Produces:
  ```rust
  pub const MCP_SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
  pub const MCP_SESSION_TASK_TYPE: &str = "mcp";
  pub const SSE_QUEUE_DEPTH: usize = 16;
  #[derive(Debug, Clone, PartialEq, Eq)] pub struct McpClient { pub client_name: String, pub client_version: String }
  #[derive(Debug, Clone)] pub struct SessionView { pub id: String, pub client: McpClient, pub protocol_version: &'static str, pub aleph_key: SessionKey, pub initialized: bool }
  pub struct SessionTable;  // new(ttl) · create(client, version) -> SessionView · touch(id) -> Option<SessionView>
                            // mark_initialized(id) -> bool · remove(id) -> bool
                            // attach_stream(id) -> Option<mpsc::Receiver<JsonRpcRequest>> · broadcast(&JsonRpcRequest) -> usize · len()
  ```

Principal note (F7): `McpClient` is the record of *who the peer says it is* (`clientInfo`); it is NOT an authority. Authority is the per-request `McpCaller` (P6.3). The two are deliberately separate structs so a session id can never stand in for a credential.

- [ ] **Step 1: Write the failing tests**

```rust
// src/gateway/mcp_face/session.rs — bottom of the file
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn client() -> McpClient {
        McpClient {
            client_name: "dsh-mcp-client".to_string(),
            client_version: "0.0.1".to_string(),
        }
    }

    #[test]
    fn create_mints_a_uuid_and_a_task_session_key() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let view = table.create(client(), "2025-03-26");
        assert!(uuid::Uuid::parse_str(&view.id).is_ok(), "session id must be a uuid: {}", view.id);
        assert_eq!(view.protocol_version, "2025-03-26");
        assert!(!view.initialized);
        assert_eq!(
            view.aleph_key,
            SessionKey::task("main", MCP_SESSION_TASK_TYPE, &view.id)
        );
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn touch_returns_the_row_and_unknown_ids_are_none() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let view = table.create(client(), "2025-06-18");
        assert_eq!(table.touch(&view.id).map(|v| v.id), Some(view.id.clone()));
        assert!(table.touch("not-a-session").is_none());
    }

    #[test]
    fn an_idle_session_expires_and_a_touched_one_does_not() {
        let ttl = Duration::from_secs(60);
        let table = SessionTable::new(ttl);
        let t0 = Instant::now();
        let idle = table.create_at(client(), "2025-03-26", t0);
        let busy = table.create_at(client(), "2025-03-26", t0);
        // `busy` is used at t0+50s; `idle` never again.
        assert!(table.touch_at(&busy.id, t0 + Duration::from_secs(50)).is_some());
        // At t0+70s `idle` is past its TTL (last seen t0) and `busy` is not
        // (last seen t0+50s).
        assert!(table.touch_at(&idle.id, t0 + Duration::from_secs(70)).is_none());
        assert!(table.touch_at(&busy.id, t0 + Duration::from_secs(70)).is_some());
        assert_eq!(table.len(), 1, "the expired row must be removed, not just hidden");
    }

    #[test]
    fn mark_initialized_and_remove_report_whether_the_row_existed() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let view = table.create(client(), "2025-11-25");
        assert!(table.mark_initialized(&view.id));
        assert!(table.touch(&view.id).is_some_and(|v| v.initialized));
        assert!(!table.mark_initialized("nope"));
        assert!(table.remove(&view.id));
        assert!(!table.remove(&view.id));
        assert_eq!(table.len(), 0);
    }

    #[tokio::test]
    async fn broadcast_reaches_only_sessions_with_an_open_stream() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let with_stream = table.create(client(), "2025-03-26");
        let _without_stream = table.create(client(), "2025-03-26");
        let mut rx = table.attach_stream(&with_stream.id).expect("known session");
        assert!(table.attach_stream("nope").is_none());

        let note = JsonRpcRequest::notification("notifications/tools/list_changed", None);
        assert_eq!(table.broadcast(&note), 1, "exactly one stream is open");
        let got = rx.recv().await.expect("the open stream receives it");
        assert_eq!(got.method, "notifications/tools/list_changed");
        assert!(got.id.is_none(), "a notification carries no id");
    }

    #[tokio::test]
    async fn a_closed_stream_is_dropped_on_the_next_broadcast() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let s = table.create(client(), "2025-03-26");
        let rx = table.attach_stream(&s.id).expect("known session");
        drop(rx); // client went away
        let note = JsonRpcRequest::notification("notifications/tools/list_changed", None);
        assert_eq!(table.broadcast(&note), 0);
        // The row survives (the client may POST again); only the stream is gone.
        assert!(table.touch(&s.id).is_some());
    }

    #[tokio::test]
    async fn reattaching_replaces_the_previous_stream() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let s = table.create(client(), "2025-03-26");
        let mut old = table.attach_stream(&s.id).unwrap();
        let mut new = table.attach_stream(&s.id).unwrap();
        let note = JsonRpcRequest::notification("notifications/tools/list_changed", None);
        assert_eq!(table.broadcast(&note), 1);
        assert!(new.recv().await.is_some());
        assert!(old.recv().await.is_none(), "the replaced sender was dropped");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::mcp_face::session -- --nocapture`
Expected: FAIL to compile — `unresolved import` / `could not find session in mcp_face`.

- [ ] **Step 3: Write the implementation**

```rust
//! One row per `Mcp-Session-Id` (Streamable HTTP, revisions ≤ 2025-11-25).
//!
//! A row maps the peer's session id to (a) the Aleph session key every tool
//! call from it runs under — `SessionKey::task("main", "mcp", <id>)`, so the
//! result store, standing grants and `ctx_search` scope to this MCP session
//! and nothing else — and (b) what the peer said it was in `clientInfo`.
//!
//! **A row is not a credential.** `authorize()` (auth.rs) runs on every
//! request; the row only tells the face which Aleph session to use. A leaked
//! id therefore buys nothing without the bearer that minted it.
//!
//! Idle rows are swept opportunistically on `create` (same shape as
//! `ExecApprovalManager::register_pending`'s `cleanup_expired`): no background
//! task, bounded work. A swept session answers 404 to its next request and the
//! SDKs re-`initialize` on 404.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::gateway::protocol::JsonRpcRequest;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;

/// How long a session may sit without a request (POST, GET stream open, or
/// DELETE) before it is swept. pi-mcp-adapter idle-disconnects at 10 min and
/// re-initializes; dsh's SDK keeps its session for the process lifetime and
/// re-initializes on 404 — 30 min covers both without keeping abandoned rows
/// for hours.
pub const MCP_SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// `SessionKey::task` type for MCP sessions. Not one of the reserved routing
/// markers (`session_key.rs:246-260`).
pub const MCP_SESSION_TASK_TYPE: &str = "mcp";

/// Notifications buffered per open SSE stream. Lossy on purpose: a client
/// that cannot drain `list_changed` events will re-sync on the next one.
pub const SSE_QUEUE_DEPTH: usize = 16;

/// What the peer declared in `initialize.clientInfo`. Display / log / audit
/// text only — never an authority (see module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClient {
    pub client_name: String,
    pub client_version: String,
}

/// A copy of the row's public facts, handed out so callers never hold the
/// table lock across an `.await`.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub id: String,
    pub client: McpClient,
    pub protocol_version: &'static str,
    pub aleph_key: SessionKey,
    /// `notifications/initialized` has arrived.
    pub initialized: bool,
}

struct Row {
    view: SessionView,
    last_seen: Instant,
    notifier: Option<mpsc::Sender<JsonRpcRequest>>,
}

/// The session table. One per face.
pub struct SessionTable {
    rows: Mutex<HashMap<String, Row>>,
    ttl: Duration,
}

impl SessionTable {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            rows: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Mint a session. Sweeps idle rows first.
    pub fn create(&self, client: McpClient, protocol_version: &'static str) -> SessionView {
        self.create_at(client, protocol_version, Instant::now())
    }

    pub(crate) fn create_at(
        &self,
        client: McpClient,
        protocol_version: &'static str,
        now: Instant,
    ) -> SessionView {
        let id = uuid::Uuid::new_v4().to_string();
        let view = SessionView {
            aleph_key: SessionKey::task("main", MCP_SESSION_TASK_TYPE, &id),
            id: id.clone(),
            client,
            protocol_version,
            initialized: false,
        };
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let ttl = self.ttl;
        rows.retain(|_, row| now.duration_since(row.last_seen) < ttl);
        rows.insert(
            id,
            Row {
                view: view.clone(),
                last_seen: now,
                notifier: None,
            },
        );
        view
    }

    /// The row, with its idle clock reset. `None` for unknown or expired ids
    /// (an expired row is removed here rather than left to the next sweep).
    pub fn touch(&self, id: &str) -> Option<SessionView> {
        self.touch_at(id, Instant::now())
    }

    pub(crate) fn touch_at(&self, id: &str, now: Instant) -> Option<SessionView> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let expired = rows
            .get(id)
            .is_some_and(|row| now.duration_since(row.last_seen) >= self.ttl);
        if expired {
            rows.remove(id);
            return None;
        }
        let row = rows.get_mut(id)?;
        row.last_seen = now;
        Some(row.view.clone())
    }

    /// Record `notifications/initialized`. `false` when the id is unknown.
    pub fn mark_initialized(&self, id: &str) -> bool {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        match rows.get_mut(id) {
            Some(row) => {
                row.view.initialized = true;
                true
            }
            None => false,
        }
    }

    /// `DELETE /mcp`. `false` when the id is unknown.
    pub fn remove(&self, id: &str) -> bool {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .is_some()
    }

    /// Open (or replace) the session's server→client stream. The receiver is
    /// what `GET /mcp` turns into SSE. Replacing drops the previous sender, so
    /// a client that reconnects its stream never leaves a dead one behind.
    pub fn attach_stream(&self, id: &str) -> Option<mpsc::Receiver<JsonRpcRequest>> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.get_mut(id)?;
        let (tx, rx) = mpsc::channel(SSE_QUEUE_DEPTH);
        row.notifier = Some(tx);
        row.last_seen = Instant::now();
        Some(rx)
    }

    /// Push one notification to every open stream. Returns how many streams
    /// took it; senders whose receiver is gone are dropped as a side effect.
    pub fn broadcast(&self, notification: &JsonRpcRequest) -> usize {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let mut delivered = 0;
        for row in rows.values_mut() {
            let Some(tx) = row.notifier.as_ref() else { continue };
            match tx.try_send(notification.clone()) {
                Ok(()) => delivered += 1,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // Lossy by design (see `SSE_QUEUE_DEPTH`).
                    tracing::debug!(session = %row.view.id, "MCP notification dropped: stream backlog full");
                }
                Err(mpsc::error::TrySendError::Closed(_)) => row.notifier = None,
            }
        }
        delivered
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
```

Add to `src/gateway/mcp_face/mod.rs`:

```rust
pub mod config;
pub mod session;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::mcp_face::session`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/mcp_face/session.rs src/gateway/mcp_face/mod.rs
git commit -m "mcp_face: session table keyed by Mcp-Session-Id with idle sweep and SSE fan-out

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.3: Per-request authorization = the `connect` rules on one HTTP request; the caller-identity nest becomes reusable

**Files:**
- Modify: `src/gateway/caller_identity.rs` (append `with_caller_identity`; the file ends at the `caller_may_choose_directory_as` tests)
- Modify: `src/gateway/server/handler.rs:2075-2099` (`dispatch_with_caller_context` body → one call)
- Modify: `src/gateway/server/handler.rs:82` (`pub(super) fn refuse_insecure_remote` → `pub(crate)`)
- Create: `src/gateway/mcp_face/auth.rs`
- Modify: `src/gateway/mcp_face/mod.rs` (add `pub mod auth;`)
- Test: `src/gateway/caller_identity.rs`, `src/gateway/mcp_face/auth.rs`

**Interfaces:**
- Consumes (F3): `handlers::connect::{resolve_connect_auth, resolve_connection_identity, ConnectAuthOutcome}`, `openai_api::auth::extract_bearer_token`, `security::{DeviceTokenManager, SecurityStore}`, `trusted_proxy::ResolvedClient`.
- Produces:
  ```rust
  // caller_identity.rs
  pub(crate) async fn with_caller_identity<F, T>(caller_role: Option<String>, caller_user: Option<String>, caller_is_loopback: bool, caller_conn_id: Option<String>, fut: F) -> T where F: Future<Output = T>;
  // mcp_face/auth.rs
  pub type SharedTokenValidator = Arc<dyn Fn(&str) -> bool + Send + Sync>;
  #[derive(Debug, Clone, PartialEq, Eq)] pub struct McpCaller { pub role: &'static str, pub user: Option<String>, pub is_local: bool, pub device_id: Option<String> }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum AuthRefusal { NoCredential, BadCredential, Walled }
  pub fn authorize(headers: &HeaderMap, client: ResolvedClient, device_tokens: &DeviceTokenManager, store: &SecurityStore, validate_shared: &dyn Fn(&str) -> bool) -> Result<McpCaller, AuthRefusal>;
  pub fn production_shared_token_validator() -> SharedTokenValidator;   // the handler.rs:1280-1284 closure, in one place
  ```

- [ ] **Step 1: Write the failing tests**

`src/gateway/caller_identity.rs` — append to its existing `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn with_caller_identity_scopes_all_four_locals_and_the_scope_owner() {
        let seen = with_caller_identity(
            Some("member".to_string()),
            Some("u-alice".to_string()),
            false,
            Some("conn-7".to_string()),
            async {
                (
                    current_caller_role(),
                    current_caller_user(),
                    current_caller_is_loopback(),
                    current_caller_conn_id(),
                    crate::scope::ambient_owner(),
                )
            },
        )
        .await;
        assert_eq!(
            seen,
            (
                Some("member".to_string()),
                Some("u-alice".to_string()),
                false,
                Some("conn-7".to_string()),
                Some("u-alice".to_string()),
            )
        );
        // Nothing leaks past the scope.
        assert_eq!(current_caller_role(), None);
        assert_eq!(current_caller_user(), None);
    }
```

`src/gateway/mcp_face/auth.rs` — bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::{UserRole, OWNER_USER_ID};
    use axum::http::header::AUTHORIZATION;

    fn store() -> Arc<SecurityStore> {
        Arc::new(SecurityStore::in_memory().unwrap())
    }

    fn headers(bearer: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(t) = bearer {
            h.insert(AUTHORIZATION, format!("Bearer {t}").parse().unwrap());
        }
        h
    }

    fn local() -> ResolvedClient {
        ResolvedClient { ip: "127.0.0.1".parse().unwrap(), secure: false, local: true }
    }

    fn remote() -> ResolvedClient {
        ResolvedClient { ip: "203.0.113.7".parse().unwrap(), secure: true, local: false }
    }

    #[test]
    fn loopback_needs_no_credential_and_is_the_owner_operator() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        let caller = authorize(&headers(None), local(), &mgr, &store, &|_| false).unwrap();
        assert_eq!(caller.role, "operator");
        assert_eq!(caller.user.as_deref(), Some(OWNER_USER_ID));
        assert!(caller.is_local);
        assert!(caller.is_operator());
    }

    #[test]
    fn remote_without_a_bearer_is_refused_as_no_credential() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        assert_eq!(
            authorize(&headers(None), remote(), &mgr, &store, &|_| true),
            Err(AuthRefusal::NoCredential)
        );
    }

    #[test]
    fn remote_with_a_bearer_nobody_accepts_is_refused_as_bad_credential() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        assert_eq!(
            authorize(&headers(Some("aleph-nope")), remote(), &mgr, &store, &|_| false),
            Err(AuthRefusal::BadCredential)
        );
    }

    #[test]
    fn remote_with_the_shared_token_is_the_owner_operator() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        let caller = authorize(
            &headers(Some("aleph-good")),
            remote(),
            &mgr,
            &store,
            &|t| t == "aleph-good",
        )
        .unwrap();
        assert_eq!(caller.role, "operator");
        assert_eq!(caller.user.as_deref(), Some(OWNER_USER_ID));
        assert!(!caller.is_local);
        assert_eq!(caller.device_id, None);
    }

    #[test]
    fn remote_with_a_member_bound_device_token_is_that_member() {
        let store = store();
        store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
        let mgr = DeviceTokenManager::new(store.clone());
        let ticket = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
        let issued = mgr
            .exchange_bootstrap_ticket(&ticket, Some("dev-pi".to_string()), Some("pi".to_string()), None)
            .unwrap();
        let caller = authorize(
            &headers(Some(&issued.device_token)),
            remote(),
            &mgr,
            &store,
            &|_| false,
        )
        .unwrap();
        assert_eq!(caller.role, "member");
        assert_eq!(caller.user.as_deref(), Some("u-alice"));
        assert_eq!(caller.device_id.as_deref(), Some("dev-pi"));
        assert!(!caller.is_operator());
    }

    #[test]
    fn a_ticket_shaped_bearer_never_mints_a_device_token() {
        // The face passes no bootstrap ticket. A ticket code in the bearer slot
        // is just an unknown token — the one-shot pairing verb stays on `connect`.
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        assert_eq!(
            authorize(&headers(Some(&ticket)), remote(), &mgr, &store, &|_| false),
            Err(AuthRefusal::BadCredential)
        );
        assert!(mgr.list_panel_devices().unwrap().is_empty(), "no device row was created");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::caller_identity -- --nocapture` → Expected: FAIL to compile (`with_caller_identity` not found).
Run: `cargo test -p alephcore --lib gateway::mcp_face::auth -- --nocapture` → Expected: FAIL to compile (module missing).

- [ ] **Step 3: Write the implementation**

`src/gateway/caller_identity.rs` — append before `#[cfg(test)]`:

```rust
/// Run `fut` as one gateway caller: the SAME nest the WS dispatch scopes
/// around `process_request` (`server::handler::dispatch_with_caller_context`
/// is now a one-line call to this). Every consumer of `current_caller_*`,
/// every `role_is_operator` gate and the spend principal
/// (`scope::ambient_owner`) read the caller from here, so a second dispatch
/// path (the MCP face) that wants to be judged like a WS connection must use
/// this and nothing else — the gateway `CLAUDE.md` 地雷 on "new dispatch paths".
pub(crate) async fn with_caller_identity<F, T>(
    caller_role: Option<String>,
    caller_user: Option<String>,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    crate::scope::with_scope(
        caller_user
            .clone()
            .map(|u| crate::scope::ScopeAttribution::personal(&u)),
        CALLER_USER.scope(
            caller_user,
            CALLER_ROLE.scope(
                caller_role,
                CALLER_IS_LOOPBACK.scope(
                    caller_is_loopback,
                    CALLER_CONN_ID.scope(caller_conn_id, fut),
                ),
            ),
        ),
    )
    .await
}
```

`src/gateway/server/handler.rs:2075-2099` — replace the body (quoted at 3ddc1f2e7):

```rust
async fn dispatch_with_caller_context(
    text: &str,
    mc: &MiddlewareChain,
    caller_role: Option<String>,
    caller_user: Option<String>,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
) -> String {
    crate::scope::with_scope(
        caller_user
            .clone()
            .map(|u| crate::scope::ScopeAttribution::personal(&u)),
        crate::gateway::caller_identity::CALLER_USER.scope(
            caller_user,
            crate::gateway::caller_identity::CALLER_ROLE.scope(
                caller_role,
                crate::gateway::caller_identity::CALLER_IS_LOOPBACK.scope(
                    caller_is_loopback,
                    crate::gateway::caller_identity::CALLER_CONN_ID
                        .scope(caller_conn_id, process_request(text, mc)),
                ),
            ),
        ),
    )
    .await
}
```

with:

```rust
async fn dispatch_with_caller_context(
    text: &str,
    mc: &MiddlewareChain,
    caller_role: Option<String>,
    caller_user: Option<String>,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
) -> String {
    crate::gateway::caller_identity::with_caller_identity(
        caller_role,
        caller_user,
        caller_is_loopback,
        caller_conn_id,
        process_request(text, mc),
    )
    .await
}
```

`src/gateway/server/handler.rs:82` — `pub(super) fn refuse_insecure_remote(` → `pub(crate) fn refuse_insecure_remote(` (the MCP route lives outside `server::`).

`src/gateway/mcp_face/auth.rs`:

```rust
//! Per-request authorization for `/mcp`.
//!
//! One rule, borrowed rather than re-stated: what `connect` accepts, this
//! accepts, minus the one-shot pairing ticket (a bearer header is a static
//! credential, and pairing stays on `connect`). Loopback is the zero-config
//! operator; a remote request must carry `Authorization: Bearer <token>`
//! where the token is a paired device token or the shared gateway token —
//! exactly the two static credentials dsh's `headers:` and pi-mcp-adapter's
//! `headers:` can carry.
//!
//! The answer is a [`McpCaller`]: the same `(user, role)` pair a WS connection
//! with that credential would be stamped with, so every gate downstream
//! (`role_is_operator`, the config-tier card, the spend principal) judges an
//! MCP call exactly as it would judge that connection's `tools.invoke`.

use axum::http::HeaderMap;

use crate::gateway::handlers::connect::{
    resolve_connect_auth, resolve_connection_identity, ConnectAuthOutcome,
};
use crate::gateway::openai_api::auth::extract_bearer_token;
use crate::gateway::security::{DeviceTokenManager, SecurityStore};
use crate::gateway::trusted_proxy::ResolvedClient;
use crate::sync_primitives::Arc;

/// `|token| bool` over the shared gateway token. Injected so the route is
/// host-testable; production uses [`production_shared_token_validator`].
pub type SharedTokenValidator = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Who is calling, in the gateway's own vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCaller {
    /// `"operator"` | `"member"` — a walled `"guest"` never becomes a caller.
    pub role: &'static str,
    /// `users.user_id` behind the credential (the owner for loopback and the
    /// shared token).
    pub user: Option<String>,
    /// [`ResolvedClient::local`] — the authority bit, not the IP.
    pub is_local: bool,
    /// The paired device, when the credential was a device token.
    pub device_id: Option<String>,
}

impl McpCaller {
    #[must_use]
    pub fn is_operator(&self) -> bool {
        crate::tools::turn_context::role_is_operator(Some(self.role))
    }
}

/// Why a request was refused. All three become `401`; the split is for logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRefusal {
    /// Remote and no `Authorization: Bearer` at all.
    NoCredential,
    /// A bearer was presented and neither token manager accepts it.
    BadCredential,
    /// The credential is valid but resolves to a walled identity
    /// (deactivated user, dangling device→user link).
    Walled,
}

/// Authorize one request. See the module doc for the rule.
pub fn authorize(
    headers: &HeaderMap,
    client: ResolvedClient,
    device_tokens: &DeviceTokenManager,
    store: &SecurityStore,
    validate_shared: &dyn Fn(&str) -> bool,
) -> Result<McpCaller, AuthRefusal> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_bearer_token);

    // The bearer is offered to BOTH static-credential slots; the ticket slot
    // is never filled from here.
    let device_id = match resolve_connect_auth(
        client.local,
        bearer,
        bearer,
        None,
        None,
        None,
        |t| validate_shared(t),
        device_tokens,
    ) {
        ConnectAuthOutcome::Authorized { device_id } => device_id,
        // Unreachable by construction (no ticket is passed). Refusing rather
        // than accepting keeps a future edit from minting device tokens off
        // a bearer header.
        ConnectAuthOutcome::BootstrapExchanged { .. } => return Err(AuthRefusal::BadCredential),
        ConnectAuthOutcome::Unauthorized => {
            return Err(if bearer.is_none() {
                AuthRefusal::NoCredential
            } else {
                AuthRefusal::BadCredential
            })
        }
    };

    let (user, role) = resolve_connection_identity(client.local, device_id.as_deref(), store);
    if role == "guest" {
        return Err(AuthRefusal::Walled);
    }
    Ok(McpCaller {
        role,
        user,
        is_local: client.local,
        device_id,
    })
}

/// The shared-token validator the WS handshake uses
/// (`server::handler`, the `connect` arm), as a handle the route state can own.
#[must_use]
pub fn production_shared_token_validator() -> SharedTokenValidator {
    Arc::new(|t: &str| {
        crate::gateway::security::SharedTokenManager::global()
            .map(|m| m.validate(t).unwrap_or(false))
            .unwrap_or(false)
    })
}
```

`src/gateway/mcp_face/mod.rs` — add `pub mod auth;`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::caller_identity gateway::mcp_face::auth`
Expected: PASS. Also `cargo test -p alephcore --lib gateway::server::handler::tests::dispatch_with_caller_context` — the three existing `dispatch_with_caller_context_*` tests (`handler.rs:2966-3110`) stay green (behaviour-preserving extraction).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/caller_identity.rs src/gateway/server/handler.rs src/gateway/mcp_face/auth.rs src/gateway/mcp_face/mod.rs
git commit -m "mcp_face: per-request bearer/loopback auth via the connect rules; extract with_caller_identity

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.4: `McpFace` core — exposed surface, scoped dispatch, `isError` shaping, `list_changed` fan-out, process-global slot, and the one line in `after_transition`

**Files:**
- Modify: `src/gateway/mcp_face/mod.rs` (replace the skeleton with the face; keep `pub mod {auth, config, session};`)
- Modify: `src/extension/lifecycle.rs` — **P1.9's** `async fn after_transition(&self)` (post-P1 shape; quoted below from plan-P1.md): add the `try_mcp_face()` call at its end (R6.1)
- Modify: `src/extension/projection.rs` — **P1.14's** module-level `tests::G3_PINNED: &[(&str, &str)]` const slice gains one appended tuple `("notify_tools_list_changed(", "lifecycle.rs")` (R6.1 / P1's R1.5; no test-body edit)
- Modify: `src/tools/adapters/registry_adapter.rs:20, 377, 538, 569` (`R: ToolRegistry + 'static` → `R: ToolRegistry + ?Sized + 'static`, four sites — lets the face hold `Arc<dyn ToolRegistry>`; every existing caller passes a sized `R` and is unaffected)
- Modify: `src/gateway/execution_engine/mod.rs:37, 44, 63` (`mod tool_refresh;` → `pub(crate) mod tool_refresh;`; add `pub(crate) use turn_permissions::resolve_exec_tier;`)
- Modify: `src/gateway/execution_engine/tool_refresh.rs:11` (`pub(super) fn plugin_tool_to_unified_tool` → `pub(crate)`)
- Modify: `src/gateway/execution_engine/tool_service_builder.rs:113` (`pub(super) fn mcp_tool_registry` → `pub(crate)`)
- Modify: `src/gateway/execution_engine/turn_permissions.rs:167` (`pub(super) fn resolve_exec_tier` → `pub(crate)`)
- Modify: `src/capability/mod.rs:318-350` (`ALL_SLOTS` gains `crate::gateway::mcp_face::mcp_face_slot(),` after the `mcp_tool_registry_slot()` row at `:349`)
- Test: `src/gateway/mcp_face/mod.rs`; `src/extension/lifecycle.rs` (`mod tests`, reusing P1.9's `isolated_manager`); `src/extension/projection.rs` (G3 stays green with six tuples in `G3_PINNED`)

**Interfaces:**
- Consumes: P6.1 `McpServerConfig`, P6.2 `SessionTable/SessionView/McpClient`, P6.3 `McpCaller` + `caller_identity::with_caller_identity`; F5 `build_request_tool_service`, `build_registry_from_tools`, `McpRegistryTool::from_registry_entry`, `mcp_tool_registry()`, `plugin_tool_to_unified_tool`, `try_extension_manager`, `hook_executor_snapshot`; F8 `CapabilitySlot`; P1.9 `lifecycle.rs::after_transition`, P1.14 `projection.rs::tests::G3_PINNED`; `arc_swap::ArcSwap` (already a dependency, used by `ScopedToolService`).
- Produces:
  ```rust
  pub type OperatorPresence = Arc<dyn Fn() -> futures::future::BoxFuture<'static, bool> + Send + Sync>;
  pub struct McpFace;
  impl McpFace {
      pub fn new(config: &McpServerConfig, tool_registry: Arc<dyn ToolRegistry>, static_tools: Vec<UnifiedTool>,
                 app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>, tool_health: Option<Arc<ToolHealthCache>>,
                 operator_presence: OperatorPresence) -> Self;
      pub fn is_enabled(&self) -> bool;  pub fn sessions(&self) -> &SessionTable;
      pub fn expose(&self) -> Arc<BTreeSet<String>>;          // ArcSwap load — P6.9 swaps it live
      pub fn known_tool_names(&self) -> BTreeSet<String>;
      pub fn unknown_expose(&self) -> Vec<String>;             // G5 runtime half: expose − known_tool_names (boot + P6.9 share it)
      pub async fn list_tools(&self, caller: &McpCaller, session: &SessionView) -> Vec<crate::mcp::protocol::ToolDefinition>;
      pub async fn call_tool(&self, caller: &McpCaller, session: &SessionView, name: &str, arguments: Value) -> Result<ToolCallResult, UnknownTool>;
      pub fn notify_tools_list_changed(&self);   // no-op when !enabled
  }
  #[derive(Debug, PartialEq, Eq)] pub struct UnknownTool(pub String);
  pub(crate) const MCP_APPROVAL_HINT: &str;
  pub fn install_mcp_face(face: Arc<McpFace>);  pub fn decline_mcp_face(because: &'static str);  pub fn try_mcp_face() -> Option<&'static Arc<McpFace>>;
  pub(crate) const fn mcp_face_slot() -> &'static dyn SlotStatus;
  #[cfg(test)] pub(crate) mod test_support;     // stub face; reused by protocol.rs, http.rs, lifecycle.rs tests
  ```
  Contract note: `try_mcp_face() -> Option<&'static Arc<McpFace>>` (G-4 adopted). **The `after_transition` wire is this task's** (R6.1): `if let Some(face) = try_mcp_face() { face.notify_tools_list_changed(); }` goes at the end of P1.9's function and nowhere else — G3 (P1.14's `G3_PINNED`) pins the needle to `lifecycle.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
// src/gateway/mcp_face/mod.rs — bottom of the file. The helpers live in a
// `pub(crate)` support module so `protocol.rs` (P6.5), `http.rs` (P6.6) and
// `extension/lifecycle.rs` (the R6.1 wire test) drive the SAME stub face
// instead of growing more copies.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::error::Result as AlephResult;
    use crate::mcp::protocol::ToolResultContent;
    use crate::sync_primitives::Mutex;
    use crate::tool_metadata::{ToolSource, UnifiedTool};
    use serde_json::json;
    use std::collections::HashMap;

    /// Canned executor — the same shape `handlers/tools_invoke.rs` tests use.
    pub(crate) struct StubRegistry {
        results: Mutex<HashMap<String, AlephResult<Value>>>,
    }

    impl StubRegistry {
        pub(crate) fn with(pairs: Vec<(&str, AlephResult<Value>)>) -> Arc<dyn ToolRegistry> {
            let mut m = HashMap::new();
            for (k, v) in pairs {
                m.insert(k.to_string(), v);
            }
            Arc::new(Self { results: Mutex::new(m) })
        }
    }

    impl ToolRegistry for StubRegistry {
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None
        }
        fn execute_tool(
            &self,
            tool_name: &str,
            _arguments: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AlephResult<Value>> + Send + '_>>
        {
            let canned = self
                .results
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(tool_name)
                .map(|r| match r {
                    Ok(v) => Ok(v.clone()),
                    Err(e) => Err(crate::error::AlephError::tool(e.to_string())),
                })
                .unwrap_or_else(|| Err(crate::error::AlephError::tool(format!("unknown: {tool_name}"))));
            Box::pin(async move { canned })
        }
    }

    pub(crate) fn tool(name: &str) -> UnifiedTool {
        UnifiedTool::new(format!("builtin:{name}"), name, format!("{name} description"), ToolSource::Builtin)
            .with_parameters_schema(json!({"type": "object", "properties": {"text": {"type": "string"}}}))
    }

    /// A face over four stub tools: `echo` (string), `structured` (json),
    /// `hidden` (registered, usually not exposed), `broken` (fails).
    /// `operator_present` drives the attendance probe.
    pub(crate) fn face_with(expose: &[&str], enabled: bool, operator_present: bool) -> McpFace {
        let cfg = McpServerConfig {
            enabled,
            expose: expose.iter().map(|s| (*s).to_string()).collect(),
        };
        McpFace::new(
            &cfg,
            StubRegistry::with(vec![
                ("echo", Ok(json!("hello"))),
                ("structured", Ok(json!({"rows": [1, 2]}))),
                ("hidden", Ok(json!("should never be reachable"))),
                ("broken", Err(crate::error::AlephError::tool("boom"))),
            ]),
            vec![tool("echo"), tool("structured"), tool("hidden"), tool("broken")],
            None,
            None,
            Arc::new(move || Box::pin(async move { operator_present })),
        )
    }

    pub(crate) fn face(expose: &[&str], enabled: bool) -> McpFace {
        face_with(expose, enabled, false)
    }

    pub(crate) fn operator() -> McpCaller {
        McpCaller { role: "operator", user: Some("u-owner".to_string()), is_local: true, device_id: None }
    }

    pub(crate) fn session(face: &McpFace) -> SessionView {
        face.sessions().create(
            McpClient { client_name: "test".to_string(), client_version: "0".to_string() },
            "2025-03-26",
        )
    }

    pub(crate) fn text_of(result: &ToolCallResult) -> String {
        match &result.content[0] {
            ToolResultContent::Text { text } => text.clone(),
            other => panic!("expected text, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn list_tools_is_the_exposed_subset_with_full_schemas() {
        let f = face(&["echo", "structured"], true);
        let s = session(&f);
        let mut names: Vec<String> = f.list_tools(&operator(), &s).await.into_iter().map(|t| t.name).collect();
        names.sort();
        assert_eq!(names, vec!["echo".to_string(), "structured".to_string()]);
        let listed = f.list_tools(&operator(), &s).await;
        let echo = listed.iter().find(|t| t.name == "echo").unwrap();
        assert_eq!(echo.input_schema.as_ref().unwrap()["properties"]["text"]["type"], "string");
        assert_eq!(echo.description.as_deref(), Some("echo description"));
    }

    #[tokio::test]
    async fn known_tool_names_is_what_the_face_can_serve_not_what_it_exposes() {
        let f = face(&["echo"], true);
        let known = f.known_tool_names();
        assert!(known.contains("hidden"), "known = everything registered");
        assert!(f.expose().contains("echo") && !f.expose().contains("hidden"));
    }

    #[test]
    fn unknown_expose_names_the_configured_strangers_and_nothing_else() {
        // G5 runtime half, on the face: boot (P6.7) and live-apply (P6.9)
        // both read this, so there is one derivation of "unknown".
        let f = face(&["echo", "no_such_tool"], true);
        assert_eq!(f.unknown_expose(), vec!["no_such_tool".to_string()]);
        assert!(face(&["echo"], true).unknown_expose().is_empty());
    }

    #[tokio::test]
    async fn a_string_value_is_returned_verbatim_as_text() {
        let f = face(&["echo"], true);
        let s = session(&f);
        let r = f.call_tool(&operator(), &s, "echo", json!({"text": "x"})).await.unwrap();
        assert_eq!(text_of(&r), "hello");
        assert_eq!(r.is_error, Some(false));
    }

    #[tokio::test]
    async fn a_json_value_is_pretty_printed() {
        let f = face(&["structured"], true);
        let s = session(&f);
        let r = f.call_tool(&operator(), &s, "structured", json!({})).await.unwrap();
        assert_eq!(text_of(&r), serde_json::to_string_pretty(&json!({"rows": [1, 2]})).unwrap());
    }

    #[tokio::test]
    async fn a_tool_outside_expose_is_unknown_even_though_it_is_registered() {
        let f = face(&["echo"], true);
        let s = session(&f);
        assert_eq!(
            f.call_tool(&operator(), &s, "hidden", json!({})).await.unwrap_err(),
            UnknownTool("hidden".to_string())
        );
    }

    #[tokio::test]
    async fn a_failing_tool_is_an_is_error_result_not_a_protocol_error() {
        let f = face(&["broken"], true);
        let s = session(&f);
        let r = f.call_tool(&operator(), &s, "broken", json!({})).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text_of(&r).contains("boom"), "{}", text_of(&r));
        assert!(!text_of(&r).contains(MCP_APPROVAL_HINT), "a plain failure gets no approval hint");
    }

    #[tokio::test]
    async fn an_empty_expose_lists_nothing_and_calls_nothing() {
        let f = face(&[], true);
        let s = session(&f);
        assert!(f.list_tools(&operator(), &s).await.is_empty());
        assert!(f.call_tool(&operator(), &s, "echo", json!({})).await.is_err());
    }

    #[test]
    fn the_nobody_was_asked_marker_is_how_the_gate_spells_it() {
        // `ConfirmDenial::lead` (tools/scoped/dispatch.rs) prints `({outcome:?})`;
        // the face keys its approval hint on the `Unavailable` arm. Both halves
        // are pinned here so a rewording on either side turns this red.
        assert_eq!(
            format!("({:?})", crate::sandbox::exec_approval::gate::ApprovalOutcome::Unavailable),
            UNAVAILABLE_MARKER
        );
        let dispatch = include_str!("../../tools/scoped/dispatch.rs");
        assert!(dispatch.contains("({outcome:?})"), "ConfirmDenial::lead no longer prints the outcome");
        let shaped = error_to_result(&ToolError::Execution {
            name: "x".to_string(),
            cause: "running `x` was not authorized — nobody was asked (Unavailable). Do not retry.".to_string(),
        });
        assert!(text_of(&shaped).ends_with(MCP_APPROVAL_HINT));
    }

    #[tokio::test]
    async fn notify_reaches_open_streams_and_is_a_no_op_when_disabled() {
        let f = face(&["echo"], true);
        let s = session(&f);
        let mut rx = f.sessions().attach_stream(&s.id).unwrap();
        f.notify_tools_list_changed();
        assert_eq!(rx.recv().await.unwrap().method, "notifications/tools/list_changed");

        let off = face(&["echo"], false);
        let s2 = session(&off);
        let mut rx2 = off.sessions().attach_stream(&s2.id).unwrap();
        off.notify_tools_list_changed();
        assert!(rx2.try_recv().is_err(), "disabled face must not broadcast");
    }

    #[test]
    fn the_slot_is_on_the_roster_and_fails_closed() {
        let slot = mcp_face_slot();
        assert_eq!(slot.id(), "gateway/mcp-face");
        assert!(matches!(slot.missing(), crate::capability::MissingSemantics::FailsClosed));
        assert!(crate::capability::ALL_SLOTS.iter().any(|s| s.id() == "gateway/mcp-face"));
    }
}
```

`src/extension/lifecycle.rs` — append to **P1.9's** `mod tests` (it already has `isolated_manager(dir)`; 判据 §4: the assertion is that the notification *reached a stream*, not that a function was called):

```rust
    /// R6.1: the one line P6 owns inside `after_transition`. The slot is
    /// install-once and process-global, so this test is the lib binary's one
    /// installer and asserts it really was (a foreign installer would make
    /// the assertion below meaningless, not merely fail it).
    #[tokio::test]
    async fn after_transition_broadcasts_tools_list_changed_to_the_installed_face() {
        use crate::gateway::mcp_face::{install_mcp_face, test_support, try_mcp_face};

        let face = Arc::new(test_support::face(&["echo"], true));
        let session = test_support::session(&face);
        let mut rx = face.sessions().attach_stream(&session.id).expect("known session");
        install_mcp_face(face.clone());
        let installed = try_mcp_face().expect("installed");
        assert!(
            Arc::ptr_eq(installed, &face),
            "another test installed a face first; this test cannot observe its own wire"
        );

        let dir = tempfile::tempdir().unwrap();
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.after_transition().await;

        let note = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("a frame within 2s")
            .expect("stream open");
        assert_eq!(note.method, "notifications/tools/list_changed");
    }
```

`src/extension/projection.rs` — **P1.14's** module-level const (quoted from plan-P1.md P1.14; a slice, so appending is one line and the test body is untouched) gains its sixth tuple. The census scans `src/extension/` only, so the definition in `src/gateway/mcp_face/mod.rs` is out of scope by construction and the `fn ` exclusion is not needed for it:

```rust
    /// (needle, the only file allowed to contain a non-definition line with it).
    /// A module-level const on purpose: P6 appends
    /// `("notify_tools_list_changed(", "lifecycle.rs")` when `after_transition`
    /// gains the MCP-face broadcast — one line here, no test-body edit.
    pub(super) const G3_PINNED: &[(&str, &str)] = &[
        ("publish_plugin_skill_dirs(", "projection.rs"),
        ("publish_plugin_subagents(", "projection.rs"),
        // The view recomputation has one trigger: lifecycle.rs::after_transition.
        ("republish_plugin_projections(", "lifecycle.rs"),
        ("sync_hooks_from_registry(", "lifecycle.rs"),
        ("after_transition(", "lifecycle.rs"),
        // The MCP face learns of a transition from that same trigger and
        // nowhere else in src/extension/ (P6.4, R6.1).
        ("notify_tools_list_changed(", "lifecycle.rs"),
    ];
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::mcp_face -- --nocapture`
Expected: FAIL to compile (`McpFace`, `UnknownTool`, `mcp_face_slot` not found).
Run: `cargo test -p alephcore --lib extension::projection::tests::publishing_plugin_projections_has_exactly_one_author` (after the `G3_PINNED` append, before the lifecycle line) → Expected: FAIL — `expected `notify_tools_list_changed(` inside lifecycle.rs, found none` (the census's self-check over `G3_PINNED`).

- [ ] **Step 3: Write the implementation**

Visibility widenings (quote → replace):

- `src/tools/adapters/registry_adapter.rs:20` `struct RegistryToolAdapter<R: ToolRegistry + 'static> {` → `struct RegistryToolAdapter<R: ToolRegistry + ?Sized + 'static> {`
- `:377` `impl<R: ToolRegistry + 'static> LoopTool for RegistryToolAdapter<R> {` → `impl<R: ToolRegistry + ?Sized + 'static> LoopTool for RegistryToolAdapter<R> {`
- `:538` `pub fn build_tool_adapters_from_tools<R: ToolRegistry + 'static>(` → `... + ?Sized + 'static>(`
- `:569` `pub fn build_registry_from_tools<R: ToolRegistry + 'static>(` → `... + ?Sized + 'static>(`
- `src/gateway/execution_engine/mod.rs:37` `mod tool_refresh;` → `pub(crate) mod tool_refresh;`; after `:63` add `pub(crate) use turn_permissions::resolve_exec_tier;`
- `src/gateway/execution_engine/tool_refresh.rs:11` `pub(super) fn plugin_tool_to_unified_tool(` → `pub(crate) fn plugin_tool_to_unified_tool(`
- `src/gateway/execution_engine/tool_service_builder.rs:113` `pub(super) fn mcp_tool_registry()` → `pub(crate) fn mcp_tool_registry()`
- `src/gateway/execution_engine/turn_permissions.rs:167` `pub(super) fn resolve_exec_tier(` → `pub(crate) fn resolve_exec_tier(`
- `src/capability/mod.rs:349` after `crate::gateway::execution_engine::tool_service_builder::mcp_tool_registry_slot(),` add `crate::gateway::mcp_face::mcp_face_slot(),`

`src/gateway/mcp_face/mod.rs`:

```rust
//! Aleph as an MCP **server** (Streamable HTTP, `/mcp`) — spec §3.7.
//!
//! An interface face (R4): it translates `tools/list` / `tools/call` into the
//! same scoped tool dispatch a chat turn uses and nothing else. Wire payloads
//! come from `crate::mcp::protocol`; the JSON-RPC envelope is the gateway's
//! own `crate::gateway::protocol`. Nothing here is a second MCP
//! implementation (CLAUDE.md 禁用清单).
//!
//! Layout: [`config`] (`[mcp_server]`), [`session`] (`Mcp-Session-Id` rows),
//! [`auth`] (per-request bearer/loopback → `McpCaller`), [`protocol`] (method
//! dispatch), [`http`] (the axum routes). This module owns the face itself:
//! which tools are exposed, how a call reaches `ScopedToolService`, and the
//! process-global handle P1's `after_transition` notifies through.
//!
//! ## Why `ScopedToolService` and not `tools.invoke`'s path
//!
//! `tools.invoke` dispatches off the raw registry and re-implements four
//! floors by hand (`handlers/tools_invoke.rs`). This face builds the SAME
//! per-request service the run loop builds, so exec tier, `tool_permissions`,
//! the confirm/operator cards, hooks and the ledger all apply to an MCP call
//! exactly as they apply to a chat turn of the same caller (spec §3.7 执行).
//!
//! ## "No operator UI ⇒ deny", made true rather than assumed
//!
//! The confirm gate parks an ATTENDED call on a card with no deadline; an
//! UNATTENDED call fails closed at once. Whether anyone can answer a card is
//! a fact about the gateway's connection table, so the face asks an
//! [`OperatorPresence`] probe per call and passes `unattended = !present`.
//! `OperatorApprovalRequester`'s zero-subscriber deny is NOT that check —
//! boot subscribes internal consumers, so it never fires on a real server.
//!
//! ## The attended card can outlive the client (ruled 2026-09-20, U-d)
//!
//! When an operator IS connected, a confirmation-gated call raises the card
//! and parks with **no deadline** (the 2026-08-28 ruling: notify + wait). The
//! MCP client's own per-call timeout (dsh 60 s, pi-mcp-adapter 30 s) is
//! shorter than a human's attention: when it fires the client drops the
//! HTTP request, axum drops this handler's future, the tool future is
//! dropped with it — and the card stays pending in `ExecApprovalManager`
//! until answered, at which point nothing runs. That is the same shape an
//! aborted Panel run leaves behind today, and it is accepted: the operator
//! sees a card whose caller has gone, answers or dismisses it, and the client
//! retries. Retiring the card when the handler future drops is a recorded
//! follow-up, not built here.
//!
//! ## `expose` is live
//!
//! The whitelist lives in an `ArcSwap` so `[mcp_server].expose` can change
//! without a restart (`config::live_apply`, the `mcp_server.expose` arm);
//! `enabled` is boot-only — the route is mounted or not when the router is
//! built.

pub mod auth;
pub mod config;
pub mod http;
pub mod protocol;
pub mod session;

use std::collections::BTreeSet;

use arc_swap::ArcSwap;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::executor::ToolRegistry;
use crate::mcp::protocol::{ToolAnnotations, ToolCallResult, ToolDefinition, ToolResultContent};
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tool_metadata::{ToolHealthCache, UnifiedTool};
use crate::tools::service::{ToolError, ToolService};

pub use auth::McpCaller;
pub use config::McpServerConfig;
pub use session::{McpClient, SessionTable, SessionView};

/// "Is an operator surface connected right now?" — answered from the
/// gateway's connection table (`GatewayServer::operator_presence_probe`).
pub type OperatorPresence = Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>;

/// `tools/call` named something the face does not serve: not exposed, or
/// exposed but not registered on this host. A protocol error (`-32602`), not
/// an `isError` result — the client's model never called anything.
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownTool(pub String);

/// Appended to an `isError` text when the gate refused because nobody could
/// be asked. The gate's own text tells a chat model what to do next; an MCP
/// client's model needs to know where the card would have gone.
pub(crate) const MCP_APPROVAL_HINT: &str = "Approval could not be obtained: no Aleph operator \
surface (the Aleph Panel) is connected to receive the approval card. Ask the Aleph operator to \
open the Panel and retry, or to grant this action standing permission (`exec_grants` / \
`[policies.tool_permissions]`), or to expose only read-only tools to this MCP client.";

/// How `ConfirmDenial::lead` spells the arm where nobody was asked
/// (`({outcome:?})` of `ApprovalOutcome::Unavailable`). Pinned by a test.
const UNAVAILABLE_MARKER: &str = "(Unavailable)";

/// The face. One per process, installed by boot ([`install_mcp_face`]).
pub struct McpFace {
    enabled: bool,
    /// The whitelist. Swapped whole by `apply_expose` (P6.9); every call
    /// loads it once and works on that snapshot.
    expose: ArcSwap<BTreeSet<String>>,
    /// Non-plugin rows of the executor's runtime map, snapshotted at boot
    /// (builtins with full schemas). Plugin and MCP-bridged tools are read
    /// live per call — that is what `list_changed` promises.
    static_tools: Vec<UnifiedTool>,
    tool_registry: Arc<dyn ToolRegistry>,
    app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
    tool_health: Option<Arc<ToolHealthCache>>,
    operator_presence: OperatorPresence,
    sessions: SessionTable,
}

impl McpFace {
    #[must_use]
    pub fn new(
        config: &McpServerConfig,
        tool_registry: Arc<dyn ToolRegistry>,
        static_tools: Vec<UnifiedTool>,
        app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
        tool_health: Option<Arc<ToolHealthCache>>,
        operator_presence: OperatorPresence,
    ) -> Self {
        Self {
            enabled: config.enabled,
            expose: ArcSwap::from_pointee(config.expose.iter().cloned().collect::<BTreeSet<String>>()),
            static_tools: static_tools
                .into_iter()
                .filter(|t| !matches!(t.source, crate::tool_metadata::ToolSource::Plugin { .. }))
                .collect(),
            tool_registry,
            app_config,
            tool_health,
            operator_presence,
            sessions: SessionTable::new(session::MCP_SESSION_IDLE_TTL),
        }
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn sessions(&self) -> &SessionTable {
        &self.sessions
    }

    /// A snapshot of the whitelist (one `ArcSwap` load).
    #[must_use]
    pub fn expose(&self) -> Arc<BTreeSet<String>> {
        self.expose.load_full()
    }

    /// G5, runtime half: every exposed name the face cannot serve right now.
    /// ONE derivation, read by boot (P6.7, warns) and by live-apply (P6.9,
    /// warns again after a swap).
    #[must_use]
    pub fn unknown_expose(&self) -> Vec<String> {
        let known = self.known_tool_names();
        let expose = self.expose();
        let configured: Vec<String> = expose.iter().cloned().collect();
        config::unknown_expose_names(&configured, &known)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// Every name the face could serve right now, ignoring `expose`.
    #[must_use]
    pub fn known_tool_names(&self) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = self.static_tools.iter().map(|t| t.name.clone()).collect();
        if let Some(ext) = crate::extension::try_extension_manager() {
            names.extend(ext.active_plugin_tools_snapshot().into_iter().map(|t| t.name));
        }
        if let Some(reg) = crate::gateway::execution_engine::tool_service_builder::mcp_tool_registry() {
            names.extend(reg.snapshot().iter().map(|(name, _)| name.clone()));
        }
        names
    }

    /// The per-call registry: `expose ∩ (static builtins ∪ live plugin tools
    /// ∪ bridged MCP tools)`. Same three sources, same join order and same
    /// "existing names win" rule as `run_loop/inner.rs`.
    fn surface(&self) -> (Arc<crate::tools::runtime::LoopToolRegistry>, BTreeSet<String>) {
        let expose = self.expose();
        let mut unified: Vec<UnifiedTool> = self
            .static_tools
            .iter()
            .filter(|t| expose.contains(&t.name))
            .cloned()
            .collect();
        if let Some(ext) = crate::extension::try_extension_manager() {
            unified.extend(
                ext.active_plugin_tools_snapshot()
                    .into_iter()
                    .filter(|t| expose.contains(&t.name))
                    .map(crate::gateway::execution_engine::tool_refresh::plugin_tool_to_unified_tool),
            );
        }
        let mut registry =
            crate::tools::adapters::build_registry_from_tools(Arc::clone(&self.tool_registry), &unified);
        let mut allowed: BTreeSet<String> = unified.iter().map(|t| t.name.clone()).collect();
        if let Some(reg) = crate::gateway::execution_engine::tool_service_builder::mcp_tool_registry() {
            for (name, handler) in reg.snapshot().iter() {
                if !expose.contains(name) || registry.get(name).is_some() {
                    continue;
                }
                registry.register(Box::new(
                    crate::tools::adapters::McpRegistryTool::from_registry_entry(name, Arc::clone(handler)),
                ));
                allowed.insert(name.clone());
            }
        }
        (Arc::new(registry), allowed)
    }

    /// The scoped service for one call: tier and explicit policy read live
    /// from `[policies]` (global rung only — an MCP session has no
    /// per-session knob), hooks from the extension manager, attendance from
    /// the presence probe.
    async fn tool_service(&self, caller: &McpCaller, session: &SessionView) -> Arc<dyn ToolService> {
        let (registry, allowed) = self.surface();
        let (global_tier, explicit) = match self.app_config.as_ref() {
            Some(cfg) => {
                let guard = cfg.read().await;
                let perms = guard.policies.tool_permissions.clone();
                let all_default = perms.default == crate::extension::PermissionAction::Allow
                    && perms.overrides.is_empty();
                (guard.policies.exec_tier, (!all_default).then_some(perms))
            }
            None => Default::default(),
        };
        let exec_tier = crate::gateway::execution_engine::resolve_exec_tier(
            global_tier,
            None,
            None,
            Some(caller.role),
        );
        let hook_executor = match crate::extension::try_extension_manager() {
            Some(ext) => {
                let snapshot = ext.hook_executor_snapshot().await;
                (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
            }
            None => None,
        };
        let unattended = !(self.operator_presence)().await;
        let turn_context = crate::tools::turn_context::TurnContext {
            session_key: session.aleph_key.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some(caller.role.to_string()),
            channel_tool_permissions: None,
            unattended,
            plan_gate: None,
            side_question: false,
        };
        crate::gateway::execution_engine::build_request_tool_service(
            registry,
            allowed,
            None,
            Some(turn_context),
            hook_executor,
            session.aleph_key.to_key_string(),
            explicit,
            exec_tier,
            unattended,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            self.tool_health.clone(),
        )
    }

    /// `tools/list`: what this caller may call, with full schemas.
    pub async fn list_tools(&self, caller: &McpCaller, session: &SessionView) -> Vec<ToolDefinition> {
        let svc = self.tool_service(caller, session).await;
        svc.list()
            .await
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: Some(d.description),
                input_schema: Some(d.input_schema),
                annotations: d.metadata.idempotent.then(|| ToolAnnotations {
                    read_only_hint: Some(true),
                    ..ToolAnnotations::default()
                }),
            })
            .collect()
    }

    /// `tools/call`. `Err` only for a name the face does not serve; every
    /// execution outcome — including a gate refusal — is an `isError` result.
    pub async fn call_tool(
        &self,
        caller: &McpCaller,
        session: &SessionView,
        name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult, UnknownTool> {
        if !self.expose().contains(name) {
            return Err(UnknownTool(name.to_string()));
        }
        let svc = self.tool_service(caller, session).await;
        let outcome = crate::gateway::caller_identity::with_caller_identity(
            Some(caller.role.to_string()),
            caller.user.clone(),
            caller.is_local,
            None,
            svc.execute_with_cancel(name, arguments, CancellationToken::new()),
        )
        .await;
        match outcome {
            Ok(output) => Ok(output_to_result(output)),
            Err(ToolError::NotFound { name }) => Err(UnknownTool(name)),
            Err(err) => Ok(error_to_result(&err)),
        }
    }

    /// `notifications/tools/list_changed` to every open stream. Called by
    /// `lifecycle::after_transition` (P1). No-op when the face is disabled.
    pub fn notify_tools_list_changed(&self) {
        if !self.enabled {
            return;
        }
        let note = crate::gateway::protocol::JsonRpcRequest::notification(
            "notifications/tools/list_changed",
            None,
        );
        let delivered = self.sessions.broadcast(&note);
        tracing::debug!(delivered, "mcp_face: tools/list_changed broadcast");
    }
}

fn output_to_result(output: ToolOutput) -> ToolCallResult {
    let text = match output.value {
        Value::String(s) => s,
        other => serde_json::to_string_pretty(&other).unwrap_or_else(|_| other.to_string()),
    };
    let mut content = vec![ToolResultContent::Text { text }];
    content.extend(output.metadata.images.into_iter().map(|img| ToolResultContent::Image {
        data: img.data,
        mime_type: img.mime_type,
    }));
    ToolCallResult {
        content,
        is_error: Some(false),
    }
}

fn error_to_result(err: &ToolError) -> ToolCallResult {
    let mut text = err.to_string();
    if text.contains(UNAVAILABLE_MARKER) {
        text.push(' ');
        text.push_str(MCP_APPROVAL_HINT);
    }
    ToolCallResult {
        content: vec![ToolResultContent::Text { text }],
        is_error: Some(true),
    }
}

// ── process-global handle ────────────────────────────────────────────────

/// `FailsClosed`: without the face, `/mcp` is not mounted (404) and
/// `notify_tools_list_changed` has nobody to call — the feature is dead and
/// says nothing, which is the honest reading of "boot never installed it".
static MCP_FACE: CapabilitySlot<Arc<McpFace>> =
    CapabilitySlot::new("gateway/mcp-face", MissingSemantics::FailsClosed);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn mcp_face_slot() -> &'static dyn SlotStatus {
    &MCP_FACE
}

/// Install the process-wide face. Idempotent (mirrors `spend::install_ledger`).
pub fn install_mcp_face(face: Arc<McpFace>) {
    let _ = MCP_FACE.install(face);
}

/// Record that boot reached the face and had nothing to install
/// (`[mcp_server] enabled = false`, or simulated mode with no tool registry).
pub fn decline_mcp_face(because: &'static str) {
    MCP_FACE.decline(because);
}

/// The installed face, if any. Lifecycle notifies through this.
pub fn try_mcp_face() -> Option<&'static Arc<McpFace>> {
    MCP_FACE.get()
}
```

(`pub mod http;` and `pub mod protocol;` are declared now so the module tree is final; create both files as empty `//! (P6.5 / P6.6)` placeholders in this task so it compiles, and fill them in the next two tasks.)

`src/extension/lifecycle.rs` — **P1.9's** function (quoted from plan-P1.md P1.9; this is the post-P1 shape):

```rust
    /// Re-derive every view after a transition: hook executor (rebuilt from
    /// the registry, then user hooks re-layered) and the process-global
    /// projections (skill dirs, sub-agents, tool index). Called exactly once
    /// per public primitive.
    async fn after_transition(&self) {
        *self.hook_executor.write().await =
            HookExecutor::empty().with_consent(ShellHookConsent::shared());
        self.sync_hooks_from_registry().await;
        self.sync_user_hooks().await;
        let projection = self.republish_plugin_projections().await;
        tracing::debug!(
            plugin_skill_dirs = projection.plugin_skill_dirs.len(),
            plugin_subagents = projection.subagents.len(),
            "published plugin projections"
        );
    }
```

becomes:

```rust
    /// Re-derive every view after a transition: hook executor (rebuilt from
    /// the registry, then user hooks re-layered), the process-global
    /// projections (skill dirs, sub-agents, tool index), and the MCP face's
    /// clients (`notifications/tools/list_changed`). Called exactly once per
    /// public primitive.
    async fn after_transition(&self) {
        *self.hook_executor.write().await =
            HookExecutor::empty().with_consent(ShellHookConsent::shared());
        self.sync_hooks_from_registry().await;
        self.sync_user_hooks().await;
        let projection = self.republish_plugin_projections().await;
        tracing::debug!(
            plugin_skill_dirs = projection.plugin_skill_dirs.len(),
            plugin_subagents = projection.subagents.len(),
            "published plugin projections"
        );
        // The MCP face's clients see the same tool set; tell them it moved.
        // `None` = boot declined the face (disabled / simulated) — nobody to
        // tell. This is the ONLY call site in src/extension/ (G3 pins it).
        if let Some(face) = crate::gateway::mcp_face::try_mcp_face() {
            face.notify_tools_list_changed();
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::mcp_face`
Expected: PASS (11 new tests in `mcp_face::tests` + P6.1–P6.3). Also `cargo test -p alephcore --lib capability::` (the roster census now sees `gateway/mcp-face`), `cargo test -p alephcore --lib tools::adapters` (the `?Sized` widening changes nothing for sized callers), `cargo test -p alephcore --lib extension::projection` (G3 green with six `G3_PINNED` tuples) and `cargo test -p alephcore --lib extension::lifecycle::tests::after_transition_broadcasts_tools_list_changed_to_the_installed_face`.

- [ ] **Step 5: Mutation step**

1. Delete the `if !self.expose().contains(name)` guard in `call_tool` → `a_tool_outside_expose_is_unknown_even_though_it_is_registered` stays green because `surface()` never registered `hidden` — the registry is the real filter, the early return is the cheap one. Record instead the mutation that proves the filter: in `surface()` replace `.filter(|t| expose.contains(&t.name))` with `.filter(|_| true)` → Expected red: `a_tool_outside_expose_is_unknown_even_though_it_is_registered`, `an_empty_expose_lists_nothing_and_calls_nothing`, `list_tools_is_the_exposed_subset_with_full_schemas`. Revert.
2. Delete the `if let Some(face) = … notify_tools_list_changed()` block from `after_transition` → Expected red: `after_transition_broadcasts_tools_list_changed_to_the_installed_face` (no frame within 2 s) **and** `publishing_plugin_projections_has_exactly_one_author` (self-check: needle absent from its owner). Revert.
3. Move that block into `plugin_ops.rs::set_plugin_enabled` instead → Expected red: `publishing_plugin_projections_has_exactly_one_author` naming `plugin_ops.rs`. Revert.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/mcp_face/mod.rs src/gateway/mcp_face/http.rs src/gateway/mcp_face/protocol.rs src/tools/adapters/registry_adapter.rs src/gateway/execution_engine/mod.rs src/gateway/execution_engine/tool_refresh.rs src/gateway/execution_engine/tool_service_builder.rs src/gateway/execution_engine/turn_permissions.rs src/capability/mod.rs src/extension/lifecycle.rs src/extension/projection.rs
git commit -m "mcp_face: McpFace core over the scoped tool service, presence-gated attendance, rostered slot; after_transition notifies it

G3_PINNED grows to six tuples; mutation reds recorded: expose filter, missing notify, notify moved to plugin_ops.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.5: `protocol.rs` — version negotiation + method dispatch (`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`, unknown → -32601)

**Files:**
- Modify: `src/gateway/mcp_face/protocol.rs` (fill the P6.4 placeholder)
- Test: `src/gateway/mcp_face/protocol.rs`

**Interfaces:**
- Consumes: F4 envelope (`gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND}`), F4 payloads (`mcp::protocol::{InitializeParams, InitializeResult, ServerCapabilities, ServerInfo, ToolCapability, ToolCallParams, ToolsListResult, MCP_LEGACY_PROTOCOL_VERSION}`), P6.4 `McpFace::{list_tools, call_tool, sessions}`, `UnknownTool`.
- Produces:
  ```rust
  pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION];
  pub const SERVER_NAME: &str = "aleph";
  pub fn negotiate(requested: &str) -> &'static str;          // requested if supported, else SUPPORTED_PROTOCOL_VERSIONS[0]
  pub fn requires_session(method: &str) -> bool;               // everything but "initialize"
  pub enum Outcome { Reply { response: JsonRpcResponse, new_session: Option<SessionView> }, Accepted }
  pub async fn handle_message(face: &McpFace, caller: &McpCaller, session: Option<&SessionView>, msg: JsonRpcRequest) -> Outcome;
  ```

- [ ] **Step 1: Write the failing tests**

```rust
// src/gateway/mcp_face/protocol.rs — bottom of the file
#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use serde_json::json;

    fn req(method: &str, params: Value, id: Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), id)
    }

    fn init_params(version: &str) -> Value {
        json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "dsh-mcp-client", "version": "0.0.1"}
        })
    }

    async fn initialized(face: &McpFace, version: &str) -> (JsonRpcResponse, SessionView) {
        match handle_message(face, &operator(), None, req("initialize", init_params(version), json!(1))).await {
            Outcome::Reply { response, new_session: Some(s) } => (response, s),
            other => panic!("initialize must reply with a new session: {other:?}"),
        }
    }

    #[test]
    fn negotiation_accepts_every_supported_version_and_answers_the_newest_otherwise() {
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            assert_eq!(negotiate(v), v);
        }
        assert_eq!(negotiate("2024-11-05"), "2025-11-25");
        assert_eq!(negotiate("2026-07-28"), "2025-11-25", "the sessionless revision is not spoken here");
        assert_eq!(negotiate(""), "2025-11-25");
    }

    #[test]
    fn the_oldest_supported_version_is_the_client_stacks_own_legacy_constant() {
        // pi-mcp-adapter negotiates this one by default (scan-pi §9.1); it must
        // never drift away from what Aleph's own client proposes.
        assert_eq!(SUPPORTED_PROTOCOL_VERSIONS[2], MCP_LEGACY_PROTOCOL_VERSION);
        assert_eq!(MCP_LEGACY_PROTOCOL_VERSION, "2025-03-26");
    }

    #[tokio::test]
    async fn initialize_handshakes_on_all_three_versions() {
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            let f = face(&["echo"], true);
            let (resp, s) = initialized(&f, v).await;
            let result = resp.result.expect("success");
            assert_eq!(result["protocolVersion"], v);
            assert_eq!(result["capabilities"]["tools"]["listChanged"], true);
            assert!(result["capabilities"].get("resources").is_none());
            assert!(result["capabilities"].get("prompts").is_none());
            assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
            assert_eq!(result["serverInfo"]["version"], env!("ALEPH_VERSION"));
            assert!(result["instructions"].as_str().is_some_and(|i| !i.is_empty()));
            assert_eq!(s.protocol_version, v);
            assert_eq!(s.client.client_name, "dsh-mcp-client");
            assert_eq!(resp.id, Some(json!(1)));
        }
    }

    #[tokio::test]
    async fn an_unsupported_version_is_answered_with_the_newest_and_a_session() {
        let f = face(&["echo"], true);
        let (resp, s) = initialized(&f, "1999-01-01").await;
        assert_eq!(resp.result.unwrap()["protocolVersion"], "2025-11-25");
        assert_eq!(s.protocol_version, "2025-11-25");
    }

    #[tokio::test]
    async fn initialize_without_client_info_is_invalid_params() {
        let f = face(&["echo"], true);
        let out = handle_message(&f, &operator(), None, req("initialize", json!({"protocolVersion": "2025-03-26"}), json!(2))).await;
        let Outcome::Reply { response, new_session } = out else { panic!("must reply") };
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
        assert!(new_session.is_none());
        assert_eq!(f.sessions().len(), 0, "a refused initialize mints no session");
    }

    #[tokio::test]
    async fn notifications_initialized_marks_the_session_and_is_accepted() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let out = handle_message(&f, &operator(), Some(&s), JsonRpcRequest::notification("notifications/initialized", None)).await;
        assert!(matches!(out, Outcome::Accepted));
        assert!(f.sessions().touch(&s.id).unwrap().initialized);
        // Any other notification is accepted and ignored.
        let out = handle_message(&f, &operator(), Some(&s), JsonRpcRequest::notification("notifications/cancelled", Some(json!({"requestId": 9})))).await;
        assert!(matches!(out, Outcome::Accepted));
    }

    #[tokio::test]
    async fn a_request_without_an_id_is_a_notification_and_gets_no_reply() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let out = handle_message(&f, &operator(), Some(&s), JsonRpcRequest::notification("ping", None)).await;
        assert!(matches!(out, Outcome::Accepted));
    }

    #[tokio::test]
    async fn ping_answers_an_empty_object() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-06-18").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), req("ping", json!({}), json!("p-1"))).await else { panic!() };
        assert_eq!(response.result, Some(json!({})));
        assert_eq!(response.id, Some(json!("p-1")), "string ids are echoed as strings");
    }

    #[tokio::test]
    async fn tools_list_is_filtered_by_expose_and_carries_schemas() {
        let f = face(&["echo", "structured"], true);
        let (_, s) = initialized(&f, "2025-11-25").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), req("tools/list", json!({}), json!(3))).await else { panic!() };
        let tools = response.result.unwrap()["tools"].as_array().unwrap().clone();
        let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        assert_eq!(names, ["echo", "structured"]);
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert!(response.result.as_ref().unwrap().get("nextCursor").is_none());
    }

    #[tokio::test]
    async fn tools_call_returns_text_content_and_is_error_false() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), req("tools/call", json!({"name": "echo", "arguments": {"text": "hi"}}), json!(4))).await else { panic!() };
        let result = response.result.unwrap();
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "hello");
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn tools_call_without_arguments_still_dispatches() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), req("tools/call", json!({"name": "echo"}), json!(5))).await else { panic!() };
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn a_failing_tool_is_is_error_true_not_a_protocol_error() {
        let f = face(&["broken"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), req("tools/call", json!({"name": "broken"}), json!(6))).await else { panic!() };
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn an_unexposed_tool_is_invalid_params_unknown_tool() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), req("tools/call", json!({"name": "hidden"}), json!(7))).await else { panic!() };
        let err = response.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("hidden"));
    }

    #[tokio::test]
    async fn an_unknown_method_is_method_not_found() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        for m in ["resources/list", "prompts/list", "server/discover", "completion/complete"] {
            let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), req(m, json!({}), json!(8))).await else { panic!() };
            assert_eq!(response.error.as_ref().unwrap().code, METHOD_NOT_FOUND, "{m}");
        }
    }

    #[tokio::test]
    async fn a_sessionless_request_other_than_initialize_is_invalid_request() {
        // The HTTP layer answers 404/400 before this; the dispatcher still
        // refuses on its own so it can never be reached around.
        let f = face(&["echo"], true);
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), None, req("tools/list", json!({}), json!(9))).await else { panic!() };
        assert_eq!(response.error.unwrap().code, INVALID_REQUEST);
        assert!(!requires_session("initialize"));
        assert!(requires_session("tools/list"));
    }

    #[tokio::test]
    async fn a_wrong_jsonrpc_version_is_invalid_request() {
        let f = face(&["echo"], true);
        let mut m = req("ping", json!({}), json!(10));
        m.jsonrpc = "1.0".to_string();
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), m).await else { panic!() };
        assert_eq!(response.error.unwrap().code, INVALID_REQUEST);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::mcp_face::protocol -- --nocapture`
Expected: FAIL to compile (`handle_message`, `Outcome`, `negotiate` not found).

- [ ] **Step 3: Write the implementation**

```rust
//! MCP method dispatch for the handshake revisions (≤ 2025-11-25).
//!
//! HTTP-neutral: takes an already-authorized [`McpCaller`], an already-resolved
//! session (or none, for `initialize`) and one JSON-RPC message; answers an
//! [`Outcome`]. `http.rs` owns status codes and headers; this file owns the
//! JSON-RPC semantics so they are testable without a listener.

use serde_json::{json, Value};

use super::auth::McpCaller;
use super::session::{McpClient, SessionView};
use super::{McpFace, UnknownTool};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
};
use crate::mcp::protocol::{
    InitializeParams, InitializeResult, ServerCapabilities, ServerInfo, ToolCallParams,
    ToolCapability, ToolsListResult, MCP_LEGACY_PROTOCOL_VERSION,
};

/// Revisions this face speaks, newest first. All three use the `initialize`
/// handshake and `Mcp-Session-Id`. The oldest is the client stack's own
/// legacy constant so the two ends of Aleph cannot disagree about it.
/// `2026-07-28` (`modern::MCP_MODERN_PROTOCOL_VERSION`) is sessionless and
/// handshake-less and is deliberately not here.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] =
    ["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION];

/// `serverInfo.name`. Also the server name the README tells hosts to use, so
/// pi names tools `aleph_<tool>` and dsh `mcp__aleph__<tool>`.
pub const SERVER_NAME: &str = "aleph";

/// `initialize.instructions`. Short: pi never shows it and dsh does not
/// consume it (scan-pi §9.1, scan-dsh §7.2); usage guidance belongs in tool
/// descriptions.
pub const INSTRUCTIONS: &str = "Aleph exposes a whitelist of its own tools over MCP. The list \
comes from tools/list and is set by the Aleph operator in [mcp_server].expose. Tools that mutate \
state may require the operator's approval in the Aleph Panel.";

/// The spec's rule: accept the client's version if we speak it, otherwise
/// answer with the newest we do and let the client decide.
#[must_use]
pub fn negotiate(requested: &str) -> &'static str {
    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|v| *v == requested)
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0])
}

/// Every method but `initialize` runs inside a session.
#[must_use]
pub fn requires_session(method: &str) -> bool {
    method != "initialize"
}

/// What one message produced.
#[derive(Debug)]
pub enum Outcome {
    /// A response to send. `new_session` is `Some` exactly when this was a
    /// successful `initialize` (the HTTP layer echoes its id in
    /// `Mcp-Session-Id`).
    Reply {
        response: JsonRpcResponse,
        new_session: Option<SessionView>,
    },
    /// A notification was consumed; nothing to send (HTTP `202`).
    Accepted,
}

/// Dispatch one message. See the module doc.
pub async fn handle_message(
    face: &McpFace,
    caller: &McpCaller,
    session: Option<&SessionView>,
    msg: JsonRpcRequest,
) -> Outcome {
    if let Err(e) = msg.validate() {
        return reply(JsonRpcResponse::error(msg.id.clone(), INVALID_REQUEST, e.message));
    }

    // JSON-RPC: no id ⇒ notification ⇒ never answered, whatever the method.
    if msg.id.is_none() {
        if msg.method == "notifications/initialized" {
            if let Some(s) = session {
                face.sessions().mark_initialized(&s.id);
            }
        }
        return Outcome::Accepted;
    }
    let id = msg.id.clone();

    if msg.method == "initialize" {
        return initialize(face, id, msg.params);
    }

    let Some(session) = session else {
        return reply(JsonRpcResponse::error(
            id,
            INVALID_REQUEST,
            "missing or unknown Mcp-Session-Id; send initialize first",
        ));
    };

    match msg.method.as_str() {
        "ping" => reply(JsonRpcResponse::success(id, json!({}))),
        "tools/list" => {
            let result = ToolsListResult {
                tools: face.list_tools(caller, session).await,
                next_cursor: None,
            };
            reply(JsonRpcResponse::success(id, to_value(&result)))
        }
        "tools/call" => {
            let params: ToolCallParams = match msg.params.and_then(|p| serde_json::from_value(p).ok()) {
                Some(p) => p,
                None => {
                    return reply(JsonRpcResponse::error(
                        id,
                        INVALID_PARAMS,
                        "tools/call requires params { name, arguments? }",
                    ))
                }
            };
            let arguments = params.arguments.unwrap_or_else(|| json!({}));
            match face.call_tool(caller, session, &params.name, arguments).await {
                Ok(result) => reply(JsonRpcResponse::success(id, to_value(&result))),
                Err(UnknownTool(name)) => reply(JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    format!("Unknown tool: {name}"),
                )),
            }
        }
        other => reply(JsonRpcResponse::error(
            id,
            METHOD_NOT_FOUND,
            format!("Method not found: {other}"),
        )),
    }
}

fn initialize(face: &McpFace, id: Option<Value>, params: Option<Value>) -> Outcome {
    let params: InitializeParams = match params.and_then(|p| serde_json::from_value(p).ok()) {
        Some(p) => p,
        None => {
            return reply(JsonRpcResponse::error(
                id,
                INVALID_PARAMS,
                "initialize requires params { protocolVersion, capabilities, clientInfo }",
            ))
        }
    };
    let version = negotiate(&params.protocol_version);
    let session = face.sessions().create(
        McpClient {
            client_name: params.client_info.name,
            client_version: params.client_info.version,
        },
        version,
    );
    let result = InitializeResult {
        protocol_version: version.to_string(),
        capabilities: ServerCapabilities {
            tools: Some(ToolCapability {
                list_changed: Some(true),
            }),
            resources: None,
            prompts: None,
        },
        server_info: Some(ServerInfo {
            name: SERVER_NAME.to_string(),
            version: Some(env!("ALEPH_VERSION").to_string()),
        }),
        instructions: Some(INSTRUCTIONS.to_string()),
    };
    Outcome::Reply {
        response: JsonRpcResponse::success(id, to_value(&result)),
        new_session: Some(session),
    }
}

fn reply(response: JsonRpcResponse) -> Outcome {
    Outcome::Reply {
        response,
        new_session: None,
    }
}

/// Plain-data payloads cannot fail to serialize; `Value::Null` rather than a
/// panic if that ever stops being true (P7).
fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::mcp_face::protocol`
Expected: PASS (16 tests).

- [ ] **Step 5: Mutation step**

Change `negotiate`'s fallback to `SUPPORTED_PROTOCOL_VERSIONS[2]` → Expected red: `negotiation_accepts_every_supported_version_and_answers_the_newest_otherwise`, `an_unsupported_version_is_answered_with_the_newest_and_a_session`. Revert.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/mcp_face/protocol.rs
git commit -m "mcp_face: initialize/ping/tools list+call dispatch with three-version negotiation

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.6: `http.rs` — `POST/GET/DELETE /mcp` (Streamable HTTP) with the `/ws` guards, `Mcp-Session-Id`, 401/404/426, a remote-only 429

**Files:**
- Modify: `src/gateway/mcp_face/http.rs` (fill the P6.4 placeholder)
- Modify: `src/gateway/server/mod.rs:10` (`mod handler;` → `pub(crate) mod handler;` so `refuse_insecure_remote` is reachable from `mcp_face`)
- Test: `src/gateway/mcp_face/http.rs` (in-process router tests, `tower::ServiceExt::oneshot`, the `artifact_route.rs:462-545` fixture shape)

**Interfaces:**
- Consumes: F1/F2 (`resolve_client`, `refuse_insecure_remote` (now `pub(crate)`), `OriginPolicy`), P6.3 `authorize` / `SharedTokenValidator` / `AuthRefusal`, P6.5 `handle_message` / `requires_session` / `Outcome`, P6.2 `SessionTable::{touch, attach_stream, remove}`.
- Produces:
  ```rust
  pub const MCP_PATH: &str = "/mcp";
  pub const SESSION_HEADER: &str = "mcp-session-id";
  pub const MCP_REMOTE_POSTS_PER_MINUTE: u32 = 120;   // private bucket, remote clients only (R6.4)
  pub struct McpRouteState;  // new(face, origin_policy, trusted_proxy_enabled, trusted_proxy_ips, allow_insecure_remote, tls_enabled, device_tokens, security_store, validate_shared)
  pub fn mcp_routes(state: Arc<McpRouteState>) -> Router;
  ```
  Status map (spec §4 MCP rows): remote without/with-bad bearer → **401** (`WWW-Authenticate: Bearer realm="aleph"`); plaintext remote when not allowed → **426**; disallowed origin → **403**; remote `POST` past `MCP_REMOTE_POSTS_PER_MINUTE` → **429** + `Retry-After` (loopback exempt, like every other loopback privilege); body not JSON / not a JSON-RPC message → **400** with a JSON-RPC error body (`PARSE_ERROR` / `INVALID_REQUEST`, `id: null`); session header absent on a non-`initialize` request → **400**; unknown/expired session → **404**; notification → **202** empty; request → **200** `application/json`; `GET` → **200** `text/event-stream`; `DELETE` → **200** empty / **404**.
  The limiter is the artifact route's shape (`artifact_route.rs:137-176`): a private `RateLimiter` so MCP traffic and `chat.send` never share a bucket, keyed by the resolved client IP, `RpcHeavy` scope.

- [ ] **Step 1: Write the failing tests**

```rust
// src/gateway/mcp_face/http.rs — bottom of the file
#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::gateway::origin_policy::OriginPolicy;
    use crate::gateway::security::{DeviceTokenManager, SecurityStore};
    use axum::body::Body;
    use axum::http::{Method, Request};
    use futures::StreamExt;
    use serde_json::json;
    use tower::ServiceExt;

    const REMOTE: [u8; 4] = [203, 0, 113, 7];
    const LOCAL: [u8; 4] = [127, 0, 0, 1];

    struct Fixture {
        app: Router,
        face: Arc<McpFace>,
    }

    fn fixture_with(expose: &[&str], allow_insecure_remote: bool) -> Fixture {
        let face = Arc::new(face(expose, true));
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let state = Arc::new(McpRouteState::new(
            face.clone(),
            Arc::new(OriginPolicy::loopback_only()),
            false,
            Vec::new(),
            allow_insecure_remote,
            false,
            Arc::new(DeviceTokenManager::new(store.clone())),
            store,
            Arc::new(|t: &str| t == "aleph-good"),
        ));
        Fixture { app: mcp_routes(state), face }
    }

    fn fixture() -> Fixture {
        fixture_with(&["echo"], true)
    }

    fn request(method: Method, ip: [u8; 4], headers: &[(&str, &str)], body: Option<Value>) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(MCP_PATH);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(serde_json::to_vec(&v).unwrap())
            }
            None => Body::empty(),
        };
        let mut req = b.body(body).expect("request");
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from((ip, 40000))));
        req
    }

    fn init_body() -> Value {
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "pi-mcp-aleph", "version": "1.0.0"}}})
    }

    async fn json_of(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn header_of(response: &Response, name: &str) -> Option<String> {
        response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
    }

    async fn initialize(fx: &Fixture, ip: [u8; 4], headers: &[(&str, &str)]) -> (StatusCode, Option<String>, Value) {
        let r = fx.app.clone().oneshot(request(Method::POST, ip, headers, Some(init_body()))).await.unwrap();
        let status = r.status();
        let sid = header_of(&r, SESSION_HEADER);
        (status, sid, json_of(r).await)
    }

    #[tokio::test]
    async fn loopback_initialize_needs_no_bearer_and_assigns_a_session() {
        let fx = fixture();
        let (status, sid, body) = initialize(&fx, LOCAL, &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(sid.is_some());
        assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
        assert!(fx.face.sessions().touch(sid.as_deref().unwrap()).is_some());
    }

    #[tokio::test]
    async fn remote_without_a_bearer_is_401_with_www_authenticate() {
        let fx = fixture();
        let r = fx.app.clone().oneshot(request(Method::POST, REMOTE, &[], Some(init_body()))).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(header_of(&r, "www-authenticate").as_deref(), Some("Bearer realm=\"aleph\""));
        assert_eq!(fx.face.sessions().len(), 0, "a refused request mints no session");
    }

    #[tokio::test]
    async fn remote_with_a_bad_bearer_is_401_and_with_the_shared_token_is_admitted() {
        let fx = fixture();
        let (status, _, _) = initialize(&fx, REMOTE, &[("authorization", "Bearer aleph-nope")]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, sid, _) = initialize(&fx, REMOTE, &[("authorization", "Bearer aleph-good")]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(sid.is_some());
    }

    #[tokio::test]
    async fn a_plaintext_remote_is_426_when_not_allowed() {
        let fx = fixture_with(&["echo"], false);
        let r = fx.app.clone().oneshot(request(Method::POST, REMOTE, &[("authorization", "Bearer aleph-good")], Some(init_body()))).await.unwrap();
        assert_eq!(r.status(), StatusCode::UPGRADE_REQUIRED);
    }

    #[tokio::test]
    async fn remote_posts_are_rate_limited_and_loopback_is_exempt() {
        let fx = fixture();
        let auth = [("authorization", "Bearer aleph-good")];
        // N admitted, the N+1th refused with Retry-After.
        let mut limited = None;
        for _ in 0..=MCP_REMOTE_POSTS_PER_MINUTE {
            let r = fx.app.clone().oneshot(request(Method::POST, REMOTE, &auth, Some(init_body()))).await.unwrap();
            if r.status() == StatusCode::TOO_MANY_REQUESTS {
                limited = Some(r);
                break;
            }
            assert_eq!(r.status(), StatusCode::OK);
        }
        let r = limited.expect("the bucket must close on the N+1th remote request");
        assert!(header_of(&r, "retry-after").is_some());
        // Loopback never pays: the same count and one more, all 200.
        for _ in 0..=MCP_REMOTE_POSTS_PER_MINUTE {
            let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[], Some(init_body()))).await.unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn a_cross_origin_request_is_403() {
        let fx = fixture();
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[("origin", "https://evil.example"), ("host", "127.0.0.1:18790")], Some(init_body()))).await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_missing_session_header_is_400_and_an_unknown_one_is_404() {
        let fx = fixture();
        let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[], Some(list.clone()))).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[(SESSION_HEADER, "not-a-session")], Some(list))).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_request_in_a_live_session_is_200_json_and_echoes_the_session() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let call = json!({"jsonrpc": "2.0", "id": "c-1", "method": "tools/call", "params": {"name": "echo", "arguments": {}}});
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[(SESSION_HEADER, &sid)], Some(call))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(header_of(&r, "content-type").unwrap().starts_with("application/json"));
        assert_eq!(header_of(&r, SESSION_HEADER).as_deref(), Some(sid.as_str()));
        let body = json_of(r).await;
        assert_eq!(body["id"], "c-1");
        assert_eq!(body["result"]["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn a_notification_is_202_with_an_empty_body() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[(SESSION_HEADER, &sid)], Some(note))).await.unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
        assert!(bytes.is_empty());
        assert!(fx.face.sessions().touch(&sid).unwrap().initialized);
    }

    #[tokio::test]
    async fn a_malformed_body_is_400_with_a_jsonrpc_error() {
        let fx = fixture();
        let mut req = request(Method::POST, LOCAL, &[], None);
        *req.body_mut() = Body::from("{not json");
        let r = fx.app.clone().oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let body = json_of(r).await;
        assert_eq!(body["error"]["code"], crate::gateway::protocol::PARSE_ERROR);
        assert_eq!(body["id"], Value::Null);

        // Valid JSON that is not a JSON-RPC message.
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[], Some(json!({"hello": "world"})))).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_of(r).await["error"]["code"], crate::gateway::protocol::INVALID_REQUEST);
    }

    #[tokio::test]
    async fn a_batch_from_a_2025_03_26_client_is_answered_as_an_array() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let batch = json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}
        ]);
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[(SESSION_HEADER, &sid)], Some(batch))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = json_of(r).await;
        let arr = body.as_array().expect("array");
        assert_eq!(arr.len(), 2, "the notification produces no entry");
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[1]["id"], 2);
        // An all-notification batch is 202.
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[(SESSION_HEADER, &sid)], Some(json!([{"jsonrpc":"2.0","method":"notifications/initialized"}])))).await.unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
        // An empty batch is a bad request.
        let r = fx.app.clone().oneshot(request(Method::POST, LOCAL, &[(SESSION_HEADER, &sid)], Some(json!([])))).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_ends_the_session_and_a_second_delete_is_404() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let r = fx.app.clone().oneshot(request(Method::DELETE, LOCAL, &[(SESSION_HEADER, &sid)], None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let r = fx.app.clone().oneshot(request(Method::DELETE, LOCAL, &[(SESSION_HEADER, &sid)], None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = fx.app.clone().oneshot(request(Method::DELETE, LOCAL, &[], None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_opens_an_sse_stream_that_carries_list_changed() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let r = fx.app.clone().oneshot(request(Method::GET, LOCAL, &[(SESSION_HEADER, &sid), ("accept", "text/event-stream")], None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(header_of(&r, "content-type").unwrap().starts_with("text/event-stream"));
        assert_eq!(header_of(&r, SESSION_HEADER).as_deref(), Some(sid.as_str()));

        let mut frames = r.into_body().into_data_stream();
        fx.face.notify_tools_list_changed();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
            .await
            .expect("a frame arrives within 2s")
            .expect("stream open")
            .expect("bytes");
        let text = String::from_utf8(first.to_vec()).unwrap();
        assert!(text.contains("event: message"), "{text}");
        assert!(text.contains("notifications/tools/list_changed"), "{text}");

        // An unknown session cannot open a stream.
        let r = fx.app.clone().oneshot(request(Method::GET, LOCAL, &[(SESSION_HEADER, "nope")], None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::mcp_face::http -- --nocapture`
Expected: FAIL to compile (`McpRouteState`, `mcp_routes` not found).

- [ ] **Step 3: Write the implementation**

```rust
//! `/mcp` — the Streamable HTTP transport (MCP revisions 2025-03-26 … 2025-11-25).
//!
//! `POST /mcp` carries one JSON-RPC message (or, for 2025-03-26 clients, a
//! batch); a request is answered `200 application/json`, a notification
//! `202`. `GET /mcp` opens the server→client SSE stream a session uses for
//! `notifications/tools/list_changed`. `DELETE /mcp` ends a session. The
//! session id travels in `Mcp-Session-Id`, minted by `initialize`.
//!
//! Every request passes the same three guards as `/ws` and the artifact byte
//! route (trusted-proxy client resolution → plaintext-remote refusal →
//! Origin policy) and then `auth::authorize`, so a leaked session id buys
//! nothing without the bearer. Like `artifact_route`, the state is narrow on
//! purpose: nothing here can reach the connection table or the RPC registry.
//!
//! Not implemented, on purpose: JSON-RPC responses sent by the client (this
//! face makes no server→client requests, so there is nothing to answer),
//! SSE resumability (`Last-Event-ID`), and the `MCP-Protocol-Version` header
//! check (a check that only ever logged would be a check in name only).

use std::net::{IpAddr, SocketAddr};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::Value;
use tokio_stream::wrappers::ReceiverStream;

use super::auth::{authorize, AuthRefusal, McpCaller, SharedTokenValidator};
use super::protocol::{handle_message, requires_session, Outcome};
use super::session::SessionView;
use super::McpFace;
use crate::gateway::origin_policy::OriginPolicy;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_REQUEST, PARSE_ERROR};
use crate::gateway::rate_limiter::{
    RateLimitConfig, RateLimitKey, RateLimitScope, RateLimiter, WindowConfig,
};
use crate::gateway::security::{DeviceTokenManager, SecurityStore};
use crate::gateway::server::handler::refuse_insecure_remote;
use crate::gateway::trusted_proxy::resolve_client;
use crate::sync_primitives::Arc;

/// The one endpoint. Also what `packages/pi-aleph/mcp.json` points at.
pub const MCP_PATH: &str = "/mcp";
/// Header name, lower-case (HTTP headers are case-insensitive; `HeaderMap`
/// normalizes lookups).
pub const SESSION_HEADER: &str = "mcp-session-id";
/// Remote `POST /mcp` per client IP per minute. A private bucket (not the
/// gateway's) so an MCP host in a tight loop cannot stall `chat.send`, and
/// loopback-exempt like every other loopback privilege. dsh/pi issue one
/// POST per tool call plus a `tools/list` per resync; 120/min is two a
/// second, far above any agent's real cadence and far below a flood.
pub const MCP_REMOTE_POSTS_PER_MINUTE: u32 = 120;

/// Narrow state for the three handlers — see the module doc.
pub struct McpRouteState {
    face: Arc<McpFace>,
    origin_policy: Arc<OriginPolicy>,
    trusted_proxy_enabled: bool,
    trusted_proxy_ips: Vec<IpAddr>,
    allow_insecure_remote: bool,
    tls_enabled: bool,
    device_tokens: Arc<DeviceTokenManager>,
    security_store: Arc<SecurityStore>,
    validate_shared: SharedTokenValidator,
    /// See [`MCP_REMOTE_POSTS_PER_MINUTE`]. `RateLimitConfig::max_entries`
    /// bounds it without a pruning task (same as the artifact route).
    rate_limiter: RateLimiter,
}

/// What `admit` hands the handlers: who, and which bucket they draw from.
struct Admitted {
    caller: McpCaller,
    client_ip: IpAddr,
}

impl McpRouteState {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        face: Arc<McpFace>,
        origin_policy: Arc<OriginPolicy>,
        trusted_proxy_enabled: bool,
        trusted_proxy_ips: Vec<IpAddr>,
        allow_insecure_remote: bool,
        tls_enabled: bool,
        device_tokens: Arc<DeviceTokenManager>,
        security_store: Arc<SecurityStore>,
        validate_shared: SharedTokenValidator,
    ) -> Self {
        Self {
            face,
            origin_policy,
            trusted_proxy_enabled,
            trusted_proxy_ips,
            allow_insecure_remote,
            tls_enabled,
            device_tokens,
            security_store,
            validate_shared,
            rate_limiter: RateLimiter::new(RateLimitConfig {
                rpc_heavy: WindowConfig {
                    max_requests: MCP_REMOTE_POSTS_PER_MINUTE,
                    window_secs: 60,
                    lockout_secs: None,
                },
                ..RateLimitConfig::default()
            }),
        }
    }
}

/// The route, ready to `merge` into the gateway router.
pub fn mcp_routes(state: Arc<McpRouteState>) -> Router {
    Router::new()
        .route(MCP_PATH, get(handle_get).post(handle_post).delete(handle_delete))
        .with_state(state)
}

/// Guards 1–3 (transport, origin) then authorization. `Err` is the refusal
/// response, ready to return.
fn admit(state: &McpRouteState, peer: SocketAddr, headers: &HeaderMap) -> Result<Admitted, Response> {
    let resolved = resolve_client(
        peer.ip(),
        headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let secure = state.tls_enabled || resolved.secure;
    if refuse_insecure_remote(resolved.local, secure, state.allow_insecure_remote) {
        tracing::warn!(client = %resolved.ip, "refused /mcp: insecure transport to a remote client");
        return Err((StatusCode::UPGRADE_REQUIRED, "TLS required for remote MCP clients").into_response());
    }
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !state.origin_policy.is_allowed(origin, host) {
        tracing::warn!(client = %resolved.ip, "refused /mcp: disallowed origin");
        return Err((StatusCode::FORBIDDEN, "origin not allowed").into_response());
    }
    authorize(
        headers,
        resolved,
        &state.device_tokens,
        &state.security_store,
        &*state.validate_shared,
    )
    .map_err(|refusal| {
        tracing::info!(client = %resolved.ip, ?refusal, "refused /mcp: unauthorized");
        let why = match refusal {
            AuthRefusal::NoCredential => "Authorization: Bearer <device token | gateway token> required",
            AuthRefusal::BadCredential => "invalid bearer token",
            AuthRefusal::Walled => "credential is valid but its user is not active",
        };
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"aleph\""))],
            why,
        )
            .into_response()
    })
    .map(|caller| Admitted {
        caller,
        client_ip: resolved.ip,
    })
}

/// The remote-only bucket (R6.4). `Ok(())` for loopback without touching the
/// limiter — the exemption is structural, not a zero-cost check.
fn charge_remote_post(state: &McpRouteState, admitted: &Admitted) -> Result<(), Response> {
    if admitted.caller.is_local {
        return Ok(());
    }
    let key = RateLimitKey::new(&admitted.client_ip.to_string(), RateLimitScope::RpcHeavy);
    state.rate_limiter.check_and_record(&key).map_err(|e| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, e.retry_after_secs().to_string())],
            "MCP request rate limit exceeded",
        )
            .into_response()
    })
}

/// The session named by the header, or the refusal to send.
fn session_from_header(state: &McpRouteState, headers: &HeaderMap) -> Result<SessionView, Response> {
    let Some(id) = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok()) else {
        return Err((StatusCode::BAD_REQUEST, "Mcp-Session-Id header required").into_response());
    };
    state
        .face
        .sessions()
        .touch(id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "unknown or expired Mcp-Session-Id; send initialize again").into_response())
}

fn with_session_header(mut response: Response, id: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(id) {
        response.headers_mut().insert(SESSION_HEADER, v);
    }
    response
}

fn bad_request(code: i32, message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(JsonRpcResponse::error(None, code, message))).into_response()
}

async fn handle_post(
    State(state): State<Arc<McpRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admitted = match admit(&state, peer, &headers) {
        Ok(a) => a,
        Err(refused) => return refused,
    };
    if let Err(refused) = charge_remote_post(&state, &admitted) {
        return refused;
    }
    let caller = admitted.caller;
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return bad_request(PARSE_ERROR, &format!("Parse error: {e}")),
    };
    let (messages, batched) = match parsed {
        Value::Array(items) if items.is_empty() => {
            return bad_request(INVALID_REQUEST, "empty batch");
        }
        Value::Array(items) => (items, true),
        other => (vec![other], false),
    };

    // One session for the whole POST: the header names it, or `initialize`
    // mints it. A batch is served in order under that one session.
    let mut session: Option<SessionView> = None;
    let mut replies: Vec<JsonRpcResponse> = Vec::new();
    for item in messages {
        let msg: JsonRpcRequest = match serde_json::from_value(item) {
            Ok(m) => m,
            Err(_) if batched => {
                replies.push(JsonRpcResponse::error(None, INVALID_REQUEST, "not a JSON-RPC message"));
                continue;
            }
            Err(_) => return bad_request(INVALID_REQUEST, "not a JSON-RPC 2.0 message"),
        };
        if requires_session(&msg.method) && session.is_none() {
            match session_from_header(&state, &headers) {
                Ok(s) => session = Some(s),
                Err(refused) => return refused,
            }
        }
        match handle_message(&state.face, &caller, session.as_ref(), msg).await {
            Outcome::Reply { response, new_session } => {
                if let Some(s) = new_session {
                    session = Some(s);
                }
                replies.push(response);
            }
            Outcome::Accepted => {}
        }
    }

    let response = if replies.is_empty() {
        StatusCode::ACCEPTED.into_response()
    } else if batched {
        (StatusCode::OK, Json(replies)).into_response()
    } else {
        (StatusCode::OK, Json(replies.remove(0))).into_response()
    };
    match session {
        Some(s) => with_session_header(response, &s.id),
        None => response,
    }
}

async fn handle_get(
    State(state): State<Arc<McpRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = admit(&state, peer, &headers) {
        return refused;
    }
    let session = match session_from_header(&state, &headers) {
        Ok(s) => s,
        Err(refused) => return refused,
    };
    let Some(rx) = state.face.sessions().attach_stream(&session.id) else {
        return (StatusCode::NOT_FOUND, "session vanished").into_response();
    };
    let stream = ReceiverStream::new(rx)
        .map(|note| Event::default().event("message").json_data(note));
    let response = Sse::new(stream).keep_alive(KeepAlive::new()).into_response();
    with_session_header(response, &session.id)
}

async fn handle_delete(
    State(state): State<Arc<McpRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = admit(&state, peer, &headers) {
        return refused;
    }
    let Some(id) = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok()) else {
        return (StatusCode::BAD_REQUEST, "Mcp-Session-Id header required").into_response();
    };
    if state.face.sessions().remove(id) {
        StatusCode::OK.into_response()
    } else {
        (StatusCode::NOT_FOUND, "unknown Mcp-Session-Id").into_response()
    }
}
```

(`handler` must be reachable as `crate::gateway::server::handler` — it is `mod handler;` inside `server/mod.rs:9`; if it is private, change that line to `pub(crate) mod handler;` and keep `refuse_insecure_remote` `pub(crate)`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::mcp_face::http`
Expected: PASS (13 tests).

- [ ] **Step 5: Mutation step**

In `admit`, replace `refuse_insecure_remote(resolved.local, secure, state.allow_insecure_remote)` with `false` → Expected red: `a_plaintext_remote_is_426_when_not_allowed`. Replace `authorize(...)` with `Ok(McpCaller { role: "operator", user: None, is_local: true, device_id: None })` → Expected red: `remote_without_a_bearer_is_401_with_www_authenticate`, `remote_with_a_bad_bearer_is_401_and_with_the_shared_token_is_admitted`. Revert both. Then in `charge_remote_post` delete `if admitted.caller.is_local { return Ok(()); }` → Expected red: `remote_posts_are_rate_limited_and_loopback_is_exempt` (loopback pays); revert. Then change `check_and_record` to `Ok(())` → the same test red (the bucket never closes); revert.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/mcp_face/http.rs src/gateway/server/mod.rs
git commit -m "mcp_face: Streamable HTTP routes on /mcp with the /ws guards, sessions and SSE

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.7: Wire the face into `GatewayServer` (route + operator-presence probe) and boot (`install_mcp_face` / `decline_mcp_face`, G5 runtime warning)

**Files:**
- Modify: `src/gateway/server/mod.rs:455-461` (new field after `canvas_store`), `:529` and `:590` (both constructors: `mcp_face: None,`), `:713-715` (new `set_mcp_face` + `operator_presence_probe` after `set_canvas_store`), `:924-926` (mount after the canvas-asset merge in `build_router`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs:2006` (insert the install block right after the `if let Some(tool_registry) = agent_result.tool_registry.as_ref() { … }` block that ends there, before `// Panel voice channel`), `:3955-3999` (sibling census test)
- Modify: `docs/reference/GATEWAY.md:1154-1163` (the "Alongside WebSocket, Gateway serves:" bullet list gains one bullet — R6.8 / G-6, same commit; P8.5 writes the full "MCP 面" section later)
- Test: `src/gateway/server/mod.rs` (`#[cfg(test)] mod tests`, next to `webhook_prefix_is_always_routed_and_404s_when_nothing_is_mounted` at `:1638`), `src/bin/aleph-server/commands/start/mod.rs` tests

**Interfaces:**
- Consumes: P6.4 `McpFace::new`, `install_mcp_face`, `decline_mcp_face`, `OperatorPresence`, `unknown_expose`; P6.6 `mcp_routes`, `McpRouteState::new`; P6.3 `production_shared_token_validator`.
- Produces:
  ```rust
  impl GatewayServer {
      pub fn set_mcp_face(&mut self, face: Arc<McpFace>);
      pub fn operator_presence_probe(&self) -> OperatorPresence;   // any handshaken connection whose caller_role is operator
  }
  ```

- [ ] **Step 1: Write the failing tests**

`src/gateway/server/mod.rs` tests (append inside the existing `mod tests`):

```rust
    /// `/mcp` is a real route only when boot installed a face AND the two
    /// auth handles it needs. Without them the path falls through to the SPA
    /// fallback and never carries a session header.
    #[tokio::test]
    async fn mcp_route_is_mounted_only_when_a_face_and_auth_handles_are_set() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let init = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "t", "version": "0"}}});
        let post = || {
            let mut req = Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&init).unwrap()))
                .unwrap();
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
            req
        };

        // Unmounted: nothing answers with a session.
        let bare = GatewayServer::new("127.0.0.1:0".parse().unwrap());
        let resp = bare.build_router().oneshot(post()).await.unwrap();
        assert!(resp.headers().get("mcp-session-id").is_none());

        // Mounted.
        let mut server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
        let store = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        server.set_security_store(store.clone());
        server.set_device_token_manager(Arc::new(
            crate::gateway::security::DeviceTokenManager::new(store),
        ));
        let cfg = crate::gateway::mcp_face::McpServerConfig { enabled: true, expose: vec![] };
        let face = Arc::new(crate::gateway::mcp_face::McpFace::new(
            &cfg,
            Arc::new(crate::executor::BuiltinToolRegistry::with_config(Default::default()).await.unwrap())
                as Arc<dyn crate::executor::ToolRegistry>,
            Vec::new(),
            None,
            None,
            server.operator_presence_probe(),
        ));
        server.set_mcp_face(face);
        let resp = server.build_router().oneshot(post()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get("mcp-session-id").is_some());
    }

    /// The presence probe answers "operator" only for a handshaken connection
    /// whose resolved role is operator — the bit the MCP face turns into
    /// `unattended`.
    #[tokio::test]
    async fn operator_presence_probe_reads_the_connection_table() {
        let server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
        let probe = server.operator_presence_probe();
        assert!(!probe().await, "empty table: nobody is present");

        let mut walled = ConnectionState::new("203.0.113.7".parse().unwrap(), false);
        walled.first_message = false;
        walled.caller_role = "guest".to_string();
        server.connections.write().await.insert("c-guest".to_string(), walled);
        assert!(!probe().await, "a walled connection is not an operator");

        let mut pre_handshake = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
        pre_handshake.first_message = true;
        server.connections.write().await.insert("c-early".to_string(), pre_handshake);
        assert!(!probe().await, "a connection that has not completed connect does not count");

        let mut op = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
        op.first_message = false;
        op.caller_role = "operator".to_string();
        server.connections.write().await.insert("c-op".to_string(), op);
        assert!(probe().await);
    }
```

`src/bin/aleph-server/commands/start/mod.rs` tests (sibling of `boot_installs_the_spend_policy_and_the_spend_ledger`):

```rust
    /// Same shape as the spend census above: the face has a process-global
    /// handle, and a handle boot never installs is a route that never mounts
    /// and a `list_changed` nobody sends — with nothing red. Both arms must
    /// exist in production text: an install, and a decline with a reason.
    #[test]
    fn boot_installs_or_declines_the_mcp_face() {
        let src = include_str!("mod.rs").replace('\r', "");
        let production = alephcore::utils::source_scan::production_prefix(&src);
        assert!(production.len() < src.len());
        let production = alephcore::utils::source_scan::code_text(&production);
        for call in ["install_mcp_face(", "decline_mcp_face(", "set_mcp_face("] {
            assert!(
                production.contains(call),
                "start/mod.rs must contain a production call to {call}"
            );
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::server::tests::mcp_route -- --nocapture` → Expected: FAIL to compile (`set_mcp_face` / `operator_presence_probe` not found).
Run: `cargo test -p alephcore --bins boot_installs_or_declines_the_mcp_face` → Expected: FAIL (`install_mcp_face(` not in production text).

- [ ] **Step 3: Write the implementation**

`src/gateway/server/mod.rs` — field, after `canvas_store` (`:461`):

```rust
    /// The MCP server face, installed by [`GatewayServer::set_mcp_face`]
    /// (boot, when `[mcp_server] enabled` and a tool registry exists).
    /// `Some` mounts `/mcp` in `build_router`; `None` leaves the path to the
    /// SPA fallback. The same `Arc` is process-global via
    /// `mcp_face::install_mcp_face` so lifecycle can broadcast through it.
    mcp_face: Option<Arc<crate::gateway::mcp_face::McpFace>>,
```

both constructors (`:529`, `:590`): add `mcp_face: None,` after `canvas_store: None,`.

after `set_canvas_store` (`:715`):

```rust
    /// Install the MCP server face (mounts `/mcp` in `build_router`). Pass the
    /// same `Arc` that `mcp_face::install_mcp_face` holds.
    pub fn set_mcp_face(&mut self, face: Arc<crate::gateway::mcp_face::McpFace>) {
        self.mcp_face = Some(face);
    }

    /// "Is an operator surface connected?" for the MCP face's attendance
    /// decision: any connection past its `connect` handshake whose resolved
    /// role is operator. Reads the live table on every call — a Panel that
    /// closes between two MCP calls flips the next call to unattended.
    pub fn operator_presence_probe(&self) -> crate::gateway::mcp_face::OperatorPresence {
        let connections = self.connections.clone();
        Arc::new(move || {
            let connections = connections.clone();
            Box::pin(async move {
                connections.read().await.values().any(|c| {
                    !c.first_message
                        && crate::tools::turn_context::role_is_operator(Some(c.caller_role.as_str()))
                })
            })
        })
    }
```

`build_router`, after the canvas-asset `merge` (`:924-926`):

```rust
        // MCP server face (`/mcp`, spec §3.7). A real route so the SPA
        // fallback never answers it; its own copy of the transport / origin
        // guards, like the artifact and canvas routes. Needs the two auth
        // handles the `connect` rules read — without them the route is not
        // mounted rather than mounted open.
        match (&self.mcp_face, &self.device_token_mgr, &self.security_store) {
            (Some(face), Some(device_tokens), Some(store)) => {
                let mcp = crate::gateway::mcp_face::http::mcp_routes(Arc::new(
                    crate::gateway::mcp_face::http::McpRouteState::new(
                        face.clone(),
                        shared.origin_policy.clone(),
                        shared.trusted_proxy_enabled,
                        shared.trusted_proxy_ips.clone(),
                        shared.allow_insecure_remote,
                        shared.tls_enabled,
                        device_tokens.clone(),
                        store.clone(),
                        crate::gateway::mcp_face::auth::production_shared_token_validator(),
                    ),
                ));
                router = router.merge(mcp);
            }
            (Some(_), _, _) => {
                warn!("mcp face installed but the device-token manager or security store is missing; /mcp not mounted");
            }
            _ => {}
        }
```

`src/bin/aleph-server/commands/start/mod.rs`, inserted after line 2006 (the `}` closing the `set_config_broadcaster` block):

```rust
    // MCP server face (`/mcp`, spec §3.7). Built over the SAME
    // `BuiltinToolRegistry` the run loop dispatches through, mounted on the
    // gateway router, and installed process-wide so
    // `extension::lifecycle::after_transition` can broadcast
    // `notifications/tools/list_changed`. Declined — with the reason on the
    // capability roster — when the operator turned it off or there is no
    // registry to serve (simulated mode).
    {
        let mcp_server_cfg = app_config.read().await.mcp_server.clone();
        match (mcp_server_cfg.enabled, agent_result.tool_registry.as_ref()) {
            (false, _) => {
                alephcore::gateway::mcp_face::decline_mcp_face("[mcp_server] enabled = false");
            }
            (true, None) => {
                alephcore::gateway::mcp_face::decline_mcp_face(
                    "no tool registry: simulated mode (no provider API key), nothing to serve on /mcp",
                );
            }
            (true, Some(tool_registry)) => {
                let registry: Arc<dyn alephcore::executor::ToolRegistry> = tool_registry.clone();
                let face = Arc::new(alephcore::gateway::mcp_face::McpFace::new(
                    &mcp_server_cfg,
                    registry,
                    tool_registry.unified_tools().cloned().collect(),
                    Some(app_config.clone()),
                    agent_result.tool_catalog.as_ref().map(|c| c.health()),
                    server.operator_presence_probe(),
                ));
                // G5, runtime half: every configured name the catalogue does
                // not know at boot. A warning, not a refusal — bridged MCP
                // tools register asynchronously and a name that belongs to
                // one serves as soon as its server is up. The same derivation
                // runs again after a live `expose` change (P6.9).
                let unknown = face.unknown_expose();
                for name in &unknown {
                    tracing::warn!(
                        tool = %name,
                        "[mcp_server].expose names a tool this server does not know at boot; \
                         it will not appear in tools/list unless a plugin or MCP server registers it"
                    );
                }
                if !args.daemon {
                    println!(
                        "  MCP server face: /mcp ({} tools exposed{})",
                        mcp_server_cfg.expose.len(),
                        if unknown.is_empty() {
                            String::new()
                        } else {
                            format!(", {} unknown at boot — see log", unknown.len())
                        }
                    );
                }
                server.set_mcp_face(face.clone());
                alephcore::gateway::mcp_face::install_mcp_face(face);
            }
        }
    }
```

`docs/reference/GATEWAY.md:1154-1163` — after the `- Metrics endpoint (`/metrics`) …` bullet (which ends with `src/gateway/middleware/latency.rs`.) add:

```markdown
- MCP server face (`POST/GET/DELETE /mcp`, Streamable HTTP) — Aleph as an MCP
  *server* for dsh / pi-mcp-adapter / Claude Code: `tools/list` + `tools/call`
  over the same scoped dispatch a chat turn uses, whitelist from
  `[mcp_server].expose`, loopback free / remote bearer (device or gateway
  token), `notifications/tools/list_changed` on plugin transitions. Implemented
  in `src/gateway/mcp_face/`; real-machine `qa/mcp_face/run.sh`.
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::server::tests` → Expected: PASS (incl. the two new tests and the existing `every_registry_the_chain_inserts_into_has_a_background_pruner` — the face adds no registry to the chain).
Run: `cargo test -p alephcore --bins` → Expected: PASS (`boot_installs_or_declines_the_mcp_face` + the existing boot censuses).
Run: `cargo test -p alephcore --lib capability::` → Expected: PASS (roster + census).

- [ ] **Step 5: Mutation step**

In `operator_presence_probe` drop `!c.first_message &&` → Expected red: `operator_presence_probe_reads_the_connection_table` (the pre-handshake loopback row defaults to `caller_role = "operator"` and would count). Revert.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/server/mod.rs src/bin/aleph-server/commands/start/mod.rs docs/reference/GATEWAY.md
git commit -m "mcp_face: mount /mcp from GatewayServer and install/decline the face at boot with the G5 expose check; GATEWAY.md route bullet

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.8: `packages/pi-aleph/` — a JS-free pi package (skill + `mcp.json`) and the three one-row host configs, pinned to the default port

**Files:**
- Create: `packages/pi-aleph/package.json`, `packages/pi-aleph/mcp.json`, `packages/pi-aleph/skills/aleph/SKILL.md`, `packages/pi-aleph/README.md`
- Modify: `src/gateway/mcp_face/mod.rs` (append a `#[cfg(test)] mod pi_aleph_package` next to `mod tests`)
- Modify: `docs/reference/EXTENSION_SYSTEM.md` — the "Node plugins run as MCP stdio servers" section **as P5.7 writes it** (plan-P5P8 P5.7; P5 precedes P6): one pointer sentence after its first paragraph (R6.8 / G-6). The GATEWAY.md "MCP 面" first-paragraph pointer named in plan-P5P8's matrix belongs to P8.5 — that section does not exist at P6 time.
- Test: `src/gateway/mcp_face/mod.rs`

**Interfaces:**
- Consumes: `crate::gateway::config::GatewayServerConfig::default().port` (F10 — the only source of `18790`), P6.6 `http::MCP_PATH`, P6.5 `protocol::SERVER_NAME`, F12 manifest shapes.
- Produces: the package files; no Rust API. `packages/` is referenced by no build tooling (`justfile`, CI, `Cargo.toml` — checked at 3ddc1f2e7), so the files are inert to the Rust build except through the `include_str!` test below.

- [ ] **Step 1: Write the failing test**

```rust
// src/gateway/mcp_face/mod.rs — after `mod tests`
/// `packages/pi-aleph/` is a wire contract written as data files; this test is
/// the one place that ties them to the constants they must agree with
/// (判据 §1: the port and the path are each written ONCE in Rust and the
/// package is checked against them, never the other way round).
#[cfg(test)]
mod pi_aleph_package {
    #[test]
    fn the_package_points_at_the_default_gateway_port_and_the_mcp_path() {
        let default_port = crate::gateway::config::GatewayServerConfig::default().port;
        let expected_url = format!("http://127.0.0.1:{default_port}{}", super::http::MCP_PATH);

        let mcp: serde_json::Value =
            serde_json::from_str(include_str!("../../../packages/pi-aleph/mcp.json")).unwrap();
        assert_eq!(mcp["mcpServers"][super::protocol::SERVER_NAME]["url"], expected_url);

        let pkg: serde_json::Value =
            serde_json::from_str(include_str!("../../../packages/pi-aleph/package.json")).unwrap();
        assert_eq!(pkg["name"], "pi-aleph");
        assert_eq!(pkg["pi"]["mcp"], "./mcp.json");
        assert!(
            pkg["pi"]["skills"].as_array().is_some_and(|a| a.iter().any(|s| s == "./skills")),
            "pi's manifest wants `skills` as an array (PiManifest.skills?: string[])"
        );
        assert!(pkg.get("main").is_none() && pkg.get("scripts").is_none(), "no JS in this package");

        let readme = include_str!("../../../packages/pi-aleph/README.md");
        assert!(readme.contains(&expected_url), "every host row must use the default URL");
        assert!(readme.contains("serverName: aleph"), "dsh row");
        assert!(readme.contains("\"type\": \"http\""), "Claude Code row");
        assert!(readme.contains("~/.pi/agent/mcp.json"), "pi row");

        let skill = include_str!("../../../packages/pi-aleph/skills/aleph/SKILL.md");
        assert!(skill.starts_with("---\nname: aleph\n"), "Agent Skills frontmatter, name first");
        let description = skill
            .lines()
            .find_map(|l| l.strip_prefix("description: "))
            .expect("a description line");
        assert!(description.chars().count() <= 1024, "pi rejects descriptions over 1024 chars");
        assert!(description.contains("tools/list"), "the skill must say the list is discovered, not fixed");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::mcp_face::pi_aleph_package -- --nocapture`
Expected: FAIL to compile — `couldn't read src/gateway/mcp_face/../../../packages/pi-aleph/mcp.json: No such file or directory`.

- [ ] **Step 3: Write the files**

`packages/pi-aleph/package.json`:

```json
{
  "name": "pi-aleph",
  "version": "0.1.0",
  "description": "Mount a running Aleph as an MCP server in pi (through pi-mcp-adapter): one skill that teaches the model what Aleph exposes, and an mcp.json that points at Aleph's /mcp. No JavaScript.",
  "license": "AGPL-3.0-or-later",
  "repository": "github:rootazero/Aleph",
  "files": [
    "skills",
    "mcp.json",
    "README.md"
  ],
  "pi": {
    "skills": ["./skills"],
    "mcp": "./mcp.json"
  }
}
```

`packages/pi-aleph/mcp.json`:

```json
{
  "mcpServers": {
    "aleph": {
      "url": "http://127.0.0.1:18790/mcp"
    }
  }
}
```

`packages/pi-aleph/skills/aleph/SKILL.md` (the `description` value is ONE line; the test measures it against pi's 1024-char limit — do not wrap it):

```markdown
---
name: aleph
description: Use when a task needs the user's Aleph assistant — its long-term memory (memory_search, recall_context, note_orient), saved sessions (session_search, session_read), web search and page fetch (search, web_fetch), workspace file reads (file_read, grep, find) or installed skills (skill_list, skill_read). Aleph is mounted as the MCP server named "aleph". The tool list is NOT fixed: it comes from tools/list and is chosen by the Aleph operator ([mcp_server].expose), so discover before calling (in pi, mcp({search:"…"}) then mcp({tool, args})). Tools that change state may require the operator's approval in the Aleph Panel; when nobody can approve, the call answers isError with the reason.
---

# Aleph over MCP

Aleph is a long-running personal assistant daemon. This skill is for the case
where **you** (pi, Claude Code, dsh, …) are the agent and Aleph is a tool
provider: its memory, notes, saved conversations, search backends and workspace
files, reached through one MCP server named `aleph`.

## Discover, then call

The server advertises `tools.listChanged`. Do not assume a tool exists — list
first. In pi the adapter exposes one proxy tool, so the sequence is:

1. `mcp({ search: "memory" })` — find candidate tools with their schemas.
2. `mcp({ describe: "aleph_memory_search" })` — read the full input schema.
3. `mcp({ tool: "aleph_memory_search", args: { query: "…" } })` — call it.

Results are text (`content[0].text`); JSON results are pretty-printed. Large
results are truncated by the host, so ask for narrower queries rather than
paging blindly.

## What is exposed by default

Read-only tools only: memory and note reads, session reads, `search`,
`web_fetch`, `file_read`/`grep`/`find` inside Aleph's own workspace, skill and
task listings, team/heartbeat status. Anything that writes — `bash`, file
writes, browser control, plugin management — is absent unless the Aleph
operator adds it to `[mcp_server].expose`.

## Approvals

A tool that needs confirmation asks the **Aleph operator**, not you. If an
operator surface (the Aleph Panel) is open, the call waits for their answer;
if none is connected the call returns `isError: true` with a reason that says
so. Do not retry such a call in a loop — report it.

## Authentication

Same machine: none. Remote: `Authorization: Bearer <token>` where the token is
a paired-device token or the shared gateway token; the host config carries it
as a static header. See the package README for the three host configurations.
```

`packages/pi-aleph/README.md`:

````markdown
# pi-aleph

Mount a running **Aleph** as an MCP server. Aleph serves the Streamable HTTP
transport on its gateway port at `/mcp` (default `http://127.0.0.1:18790/mcp`),
speaks MCP `2025-11-25` / `2025-06-18` / `2025-03-26` (the client picks; an
unsupported version is answered with `2025-11-25`), advertises only `tools`
(with `listChanged`), and exposes the tool whitelist from `[mcp_server].expose`
in Aleph's `config.toml` (default: read-only tools).

This package contains no JavaScript: a skill that tells the model what Aleph
offers, and an `mcp.json`.

## Authentication

* Same machine (loopback): no credential.
* Remote: `Authorization: Bearer <token>` — a paired device token
  (`aleph-dt-…`) or the shared gateway token (`aleph-…`, from the Panel's
  Settings → Gateway, or `gateway.token.current` over RPC). Plaintext remote is
  refused with `426` unless `[gateway] allow_insecure_remote = true`; the
  documented remote tier is TLS.

## pi (via `pi-mcp-adapter`)

Install the package (skill + `mcp.json`):

```sh
pi install /path/to/Aleph/packages/pi-aleph
```

Servers loaded from a package manifest are prefixed by the package name
(`pi_aleph__aleph`). To keep the bare `aleph` prefix — so tools read
`aleph_memory_search` — put the row in `~/.pi/agent/mcp.json` (or the
project's `.mcp.json`) instead:

```json
{
  "mcpServers": {
    "aleph": {
      "url": "http://127.0.0.1:18790/mcp",
      "headers": { "Authorization": "Bearer ${ALEPH_TOKEN}" }
    }
  }
}
```

Drop `headers` on the same machine. The adapter's default `protocolVersion`
(`legacy`) negotiates `2025-03-26`, which Aleph accepts as-is.

## Claude Code (`.mcp.json`)

```json
{
  "mcpServers": {
    "aleph": {
      "type": "http",
      "url": "http://127.0.0.1:18790/mcp",
      "headers": { "Authorization": "Bearer ${ALEPH_TOKEN}" }
    }
  }
}
```

Tools appear as `mcp__aleph__<tool>`; hook matchers use that name.

## dsh (`cordis.yml`)

```yaml
- id: mcp-aleph
  name: '@deepseek-ai/dsh-mcp-client'
  config:
    serverName: aleph
    transport: streamable-http
    url: http://127.0.0.1:18790/mcp
    headers:
      Authorization: !!js `Bearer ${process.env.ALEPH_TOKEN}`
```

dsh names tools `mcp__aleph__<tool>`, honours `tools/list_changed`, and turns
`isError: true` into a failed tool result (60 s per-call timeout by default).

## Approvals and timeouts

A tool that needs confirmation raises the card to the **Aleph operator**
(Panel). When an operator surface is connected the call **waits for the
answer with no deadline on Aleph's side** (Aleph's rule for attended
approvals is notify-and-wait). Your host's per-call timeout (dsh 60 s,
pi-mcp-adapter 30 s) is what bounds the wait on your side, and it is shorter
than a human's attention — so this is what happens when it fires first:

* the host reports the call as timed out / errored;
* the card **stays pending** in the Aleph Panel until the operator answers or
  dismisses it; once answered, nothing runs (the caller is gone);
* retry the call after the operator has answered — a standing grant
  (`exec_grants`, "allow for this session") makes the retry skip the card.

This dangling-card shape is accepted and documented rather than hidden; ask
the operator to keep the Panel open and answer promptly, or to grant the
action standing permission (`exec_grants` / `[policies.tool_permissions]`)
before an agent depends on it. When **no** operator surface is connected the
call returns `isError: true` at once, with a reason naming the Panel.

## Exposing more

```toml
[mcp_server]
enabled = true
expose = ["memory_search", "file_read", "grep", "find", "team_disband"]
```

`expose` applies live: Aleph re-reads it on save and sends
`notifications/tools/list_changed` to every connected MCP client (hosts that
honour it re-list). `enabled` needs a restart. A name Aleph does not know is
warned about and never listed.
````

`docs/reference/EXTENSION_SYSTEM.md` — in P5.7's "Node plugins run as MCP stdio servers" section, after its first paragraph (the one ending `nothing on the host side answers it.`) add:

```markdown
The other direction — another agent (pi, Claude Code, dsh) mounting **Aleph** as an MCP server —
is `src/gateway/mcp_face/` (`/mcp`, GATEWAY.md); the JS-free `packages/pi-aleph/` package carries
the pi skill and the three one-row host configs.
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::mcp_face::pi_aleph_package`
Expected: PASS.

- [ ] **Step 5: Mutation step**

Change the port in `packages/pi-aleph/mcp.json` to `18791` → Expected red: `the_package_points_at_the_default_gateway_port_and_the_mcp_path`. Revert.

- [ ] **Step 6: Commit**

```bash
git add packages/pi-aleph src/gateway/mcp_face/mod.rs docs/reference/EXTENSION_SYSTEM.md
git commit -m "pi-aleph: JS-free pi package (skill + mcp.json), the pi / Claude Code / dsh one-row mounts, EXTENSION_SYSTEM pointer

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P6.9: live-apply for `[mcp_server].expose` — swap the whitelist, re-run G5, broadcast `list_changed` (R6.3; spec §3.7 通知)

**Files:**
- Modify: `src/config/reload_impact.rs:108` (`LIVE_SUBSECTIONS` gains `"mcp_server.expose"`; its doc at `:76-107` gets one sentence) and its tests (`:283-300` region — add `mcp_server_expose_is_live_and_enabled_is_not`)
- Modify: `src/config/live_apply.rs:51-210` (`apply_live_sections` gains the `"mcp_server.expose"` arm) and `:233-256` (`every_live_section_has_an_apply_arm`'s `known_arms` gains the same name)
- Modify: `src/gateway/mcp_face/mod.rs` (add `apply_expose`)
- Test: `src/gateway/mcp_face/mod.rs`, `src/config/reload_impact.rs`, `src/config/live_apply.rs`

**Interfaces:**
- Consumes: P6.4 `McpFace { expose: ArcSwap<BTreeSet<String>> }`, `unknown_expose`, `notify_tools_list_changed`, `try_mcp_face`; `config::reload_impact::{LIVE_SUBSECTIONS, dotted_prefix_matches}`; `config::live_apply::apply_live_sections` (the single-patch caller passes the **top-level** section name — `patcher.rs` — and `dotted_prefix_matches("mcp_server", "mcp_server.expose")` covers the subsection exactly as `policies` covers `policies.spend`, `live_apply.rs:55-70`).
- Produces:
  ```rust
  #[derive(Debug, PartialEq, Eq)] pub struct ExposeApplied { pub changed: bool, pub unknown: Vec<String> }
  impl McpFace { pub fn apply_expose(&self, config: &McpServerConfig) -> ExposeApplied; }
  // reload_impact: LIVE_SUBSECTIONS = ["policies.spend", "policies.terminal", "mcp_server.expose"]
  ```
  `enabled` is deliberately NOT live: the route is mounted in `build_router`; flipping it means a restart, and `ReloadImpact::classify("mcp_server.enabled")` says so (`Restart`).

- [ ] **Step 1: Write the failing tests**

`src/gateway/mcp_face/mod.rs` — inside `mod tests`:

```rust
    #[tokio::test]
    async fn apply_expose_swaps_the_list_and_notifies_each_open_stream_once() {
        let f = face(&["echo"], true);
        let s1 = session(&f);
        let s2 = session(&f);
        let _s3_without_stream = session(&f);
        let mut rx1 = f.sessions().attach_stream(&s1.id).unwrap();
        let mut rx2 = f.sessions().attach_stream(&s2.id).unwrap();
        assert_eq!(f.list_tools(&operator(), &s1).await.len(), 1);

        let wider = McpServerConfig {
            enabled: true,
            expose: vec!["echo".to_string(), "structured".to_string(), "no_such_tool".to_string()],
        };
        let applied = f.apply_expose(&wider);
        assert_eq!(
            applied,
            ExposeApplied { changed: true, unknown: vec!["no_such_tool".to_string()] }
        );

        // tools/list differs...
        let mut names: Vec<String> = f.list_tools(&operator(), &s1).await.into_iter().map(|t| t.name).collect();
        names.sort();
        assert_eq!(names, vec!["echo".to_string(), "structured".to_string()]);
        // ...and every live stream got exactly one notification.
        for rx in [&mut rx1, &mut rx2] {
            let note = rx.recv().await.expect("one frame");
            assert_eq!(note.method, "notifications/tools/list_changed");
            assert!(rx.try_recv().is_err(), "exactly one, not one per name");
        }

        // The same list again is not a change and sends nothing.
        let again = f.apply_expose(&wider);
        assert!(!again.changed);
        assert!(rx1.try_recv().is_err() && rx2.try_recv().is_err());
    }
```

`src/config/reload_impact.rs` — inside its `mod tests`:

```rust
    #[test]
    fn mcp_server_expose_is_live_and_enabled_is_not() {
        // The face swaps the whitelist on write (`live_apply`'s arm); the
        // route is mounted at boot, so `enabled` still needs a restart — and
        // the bare parent must not promise Live for its `enabled` child.
        assert_eq!(ReloadImpact::classify("mcp_server.expose"), ReloadImpact::Live);
        assert_eq!(ReloadImpact::classify("mcp_server.enabled"), ReloadImpact::Restart);
        assert_eq!(ReloadImpact::classify("mcp_server"), ReloadImpact::Restart);
    }
```

`src/config/live_apply.rs` — inside its `mod tests`, and add `"mcp_server.expose"` to `known_arms` in `every_live_section_has_an_apply_arm` (`:235-241`):

```rust
    /// The arm lands exactly when a face is installed. The slot is
    /// process-global and install-once, and another test in this binary
    /// (`extension::lifecycle`) installs one — so the expectation is read
    /// from the slot rather than assumed, which is the arm's actual contract:
    /// "declared live, downgraded to restart when the runtime handle is
    /// absent" (`apply_live_sections`'s own doc).
    #[test]
    fn the_mcp_server_expose_arm_lands_iff_a_face_is_installed() {
        let mut cfg = Config::default();
        cfg.mcp_server.expose = vec!["grep".to_string()];
        let applied = apply_live_sections(&cfg, &["mcp_server"]);
        let installed = crate::gateway::mcp_face::try_mcp_face().is_some();
        assert_eq!(
            applied.contains(&"mcp_server.expose"),
            installed,
            "the arm must land iff a face is installed (installed={installed}, applied={applied:?})"
        );
        // The coarse top-level name reaches the subsection, like `policies` → `policies.spend`.
        assert!(
            crate::config::reload_impact::dotted_prefix_matches("mcp_server", "mcp_server.expose")
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::mcp_face::tests::apply_expose -- --nocapture` → Expected: FAIL to compile (`apply_expose`, `ExposeApplied` not found).
Run: `cargo test -p alephcore --lib config::reload_impact::tests::mcp_server_expose_is_live` → Expected: FAIL (`classify("mcp_server.expose")` is `Restart`).
Run: `cargo test -p alephcore --lib config::live_apply` → Expected: FAIL — `every_live_section_has_an_apply_arm` once `LIVE_SUBSECTIONS` grows (`'mcp_server.expose' is declared live … no arm`), then the new test.

- [ ] **Step 3: Write the implementation**

`src/gateway/mcp_face/mod.rs` — after `notify_tools_list_changed`:

```rust
/// What a live `[mcp_server].expose` write did.
#[derive(Debug, PartialEq, Eq)]
pub struct ExposeApplied {
    /// The whitelist actually differs from the previous one (and every open
    /// stream was told). Re-saving the same list is not a change.
    pub changed: bool,
    /// G5, runtime half, re-run against the new list — see
    /// [`McpFace::unknown_expose`].
    pub unknown: Vec<String>,
}

impl McpFace {
    /// Live-apply for `[mcp_server].expose` (`config::live_apply`, the
    /// `mcp_server.expose` arm). Swaps the whitelist whole, re-runs the G5
    /// check, and — only when the set changed — broadcasts
    /// `notifications/tools/list_changed`, once per open stream. `enabled`
    /// is not read here: the route is mounted at boot.
    pub fn apply_expose(&self, config: &McpServerConfig) -> ExposeApplied {
        let next: BTreeSet<String> = config.expose.iter().cloned().collect();
        let previous = self.expose.swap(Arc::new(next));
        let changed = *previous != *self.expose();
        let unknown = self.unknown_expose();
        for name in &unknown {
            tracing::warn!(
                tool = %name,
                "[mcp_server].expose names a tool this server does not know; it will not appear in tools/list"
            );
        }
        if changed {
            self.notify_tools_list_changed();
        }
        ExposeApplied { changed, unknown }
    }
}
```

`src/config/reload_impact.rs:108` (quoted at 3ddc1f2e7):

```rust
pub(crate) const LIVE_SUBSECTIONS: &[&str] = &["policies.spend", "policies.terminal"];
```

becomes:

```rust
pub(crate) const LIVE_SUBSECTIONS: &[&str] =
    &["policies.spend", "policies.terminal", "mcp_server.expose"];
```

and its doc comment gains: `/// - `mcp_server.expose`: the MCP face swaps its whitelist and tells every connected client; `mcp_server.enabled` mounts the route at boot and is not live, so the parent stays out of `LIVE_SECTIONS`.`

`src/config/live_apply.rs` — a new arm before `"search" => …` in `apply_live_sections`:

```rust
            // The MCP face swaps its whitelist whole and tells every
            // connected client; an absent face (declined at boot: disabled,
            // or simulated mode) means the change is persisted and takes
            // effect at the next boot — the same downgrade as `search`.
            "mcp_server.expose" => match crate::gateway::mcp_face::try_mcp_face() {
                Some(face) => {
                    let applied = face.apply_expose(&cfg.mcp_server);
                    tracing::info!(
                        changed = applied.changed,
                        unknown = applied.unknown.len(),
                        "[mcp_server].expose applied live"
                    );
                    true
                }
                None => false,
            },
```

and in `every_live_section_has_an_apply_arm`, `known_arms` gains `"mcp_server.expose",`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::mcp_face config::reload_impact config::live_apply`
Expected: PASS. Also `cargo test -p alephcore --lib config::` (the `every_dedicated_config_handler_that_saves_a_live_section_calls_apply_live_sections` census at `live_apply.rs:692` is unaffected: `[mcp_server]` has no dedicated handler, it is written through the generic `config.patch` / `self_config` path that already calls `apply_live_sections`).

- [ ] **Step 5: Mutation step**

1. In `apply_expose` drop `if changed` (always notify) → Expected red: `apply_expose_swaps_the_list_and_notifies_each_open_stream_once` ("the same list again … sends nothing"). Revert.
2. Remove `"mcp_server.expose"` from `LIVE_SUBSECTIONS` only → Expected red: `mcp_server_expose_is_live_and_enabled_is_not` and `every_live_section_has_an_apply_arm` ("handles 'mcp_server.expose' but ReloadImpact does not call it live"). Revert.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/mcp_face/mod.rs src/config/reload_impact.rs src/config/live_apply.rs
git commit -m "mcp_face: [mcp_server].expose applies live — swap, re-check, one list_changed per open stream

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P7.1: `qa/mcp_face/run.sh` — real-machine stages `handshake` · `tools` · `auth` · `list_changed` · `deny`

**Files:**
- Create: `qa/mcp_face/run.sh`, `qa/mcp_face/drive.py`, `qa/mcp_face/patch_mcp.py`
- Modify: `qa/README.md:618-625` (add the `mcp_face` rows after the `qa/plugins/run.sh trust` row) and `qa/README.md:1558-1600` (one bullet in「每个装置在证明什么」after the `browser_managed` bullet)
- Test: the fixture itself (`./qa/mcp_face/run.sh <stage>`; exit code = failure count; `SKIP` is printed, never counted as PASS)

**Interfaces:**
- Consumes (F11): `qa/lib/{scratch_home.sh,build.sh}`, `qa/busy_input/patch_config.py`, `qa/browser_managed/qa_rpc.py::Ledger`, `websockets`, the boot banner line `MCP server face: /mcp` (P6.7), the tool face `plugin_manage` over `tools.invoke`, `gateway.token.current|rotate`.
- Produces: the fixture. The `list_changed` stage exercises P6.4's own line inside P1's `after_transition` (R6.1) on top of P1's lifecycle (a disabled plugin leaves `active_plugin_tools`); P1 precedes P6, so the stage is PASS once P6 lands (R6.7).

What each stage proves (this text is also the README bullet):

| stage | proves | reads |
|---|---|---|
| `handshake` | the three supported versions each negotiate to themselves; an unsupported one is answered with `2025-11-25`; `capabilities` is exactly `tools.listChanged`; `serverInfo.name == "aleph"`; `Mcp-Session-Id` is minted; `notifications/initialized` is `202`; `ping` is `{}`; string ids come back as strings | HTTP status, headers, JSON-RPC bodies |
| `tools` | `tools/list ⊆ expose` on a REAL registry (not the stub), `grep`/`file_read`/`agent_list` are present with `inputSchema`, a read-only call (`agent_list`) yields `content[0].type == "text"` and `isError == false` | JSON-RPC bodies |
| `auth` | bound to `0.0.0.0` + `allow_insecure_remote`, a request from the host's LAN address without a bearer is `401` + `WWW-Authenticate`, a wrong bearer is `401`, the shared gateway token (fetched over loopback WS) is admitted; loopback needs nothing | status + headers, from a non-loopback source address |
| `list_changed` | a planted static plugin's `[[tools]]` entry is listed while enabled; `plugin_manage{disable}` produces an SSE `notifications/tools/list_changed` frame within 10 s and the tool leaves `tools/list`; `enable` brings both back | SSE frames on `GET /mcp`, `tools/list` before/after |
| `deny` | a registered-but-unexposed tool (`bash`) is JSON-RPC `-32602`; a confirmation-gated tool added to `expose` (`agent_delete`) with **no operator surface connected** answers `isError: true` whose text carries `(Unavailable)` and names the Aleph Panel — the fixture holds NO WebSocket open during this stage, because a loopback WS client IS an operator surface | JSON-RPC bodies |

- [ ] **Step 1: Write `qa/mcp_face/patch_mcp.py`**

```python
#!/usr/bin/env python3
"""Rewrite `[mcp_server]` in a generated Aleph config, and optionally open the
gateway to the LAN for the `auth` stage.

`toml::to_string` writes `expose = [...]` on one line, but nothing here should
depend on that: the whole `[mcp_server]` section is dropped and re-appended,
the same way `qa/busy_input/patch_config.py` drops `[channels]`/`[providers]`.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--expose", required=True, help="comma-separated tool names")
p.add_argument("--lan", action="store_true", help='[gateway] host="0.0.0.0" + allow_insecure_remote=true')
args = p.parse_args()

src = open(args.path).read()


def drop_section(text, name):
    out, keep = [], True
    for line in text.splitlines():
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            keep = m.group(1).strip() != name
        if keep:
            out.append(line)
    return "\n".join(out) + "\n"


def set_key(text, section, key, value):
    lines = text.splitlines()
    out, cur, inserted = [], None, False
    for line in lines:
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            cur = m.group(1).strip()
            out.append(line)
            if cur == section and not inserted:
                out.append(f"{key} = {value}")
                inserted = True
            continue
        if cur == section and re.match(rf"^\s*{re.escape(key)}\s*=", line):
            continue
        out.append(line)
    text = "\n".join(out) + "\n"
    if not inserted:
        text += f"\n[{section}]\n{key} = {value}\n"
    return text


src = drop_section(src, "mcp_server")
names = [n.strip() for n in args.expose.split(",") if n.strip()]
src += "\n[mcp_server]\nenabled = true\nexpose = [" + ", ".join(f'"{n}"' for n in names) + "]\n"

if args.lan:
    src = set_key(src, "gateway", "host", '"0.0.0.0"')
    src = set_key(src, "gateway", "allow_insecure_remote", "true")

open(args.path, "w").write(src)
print(f"[mcp_server].expose = {names}" + (" (LAN open)" if args.lan else ""))
```

- [ ] **Step 2: Write `qa/mcp_face/drive.py`**

```python
#!/usr/bin/env python3
"""Real-machine driver for the MCP server face (spec §3.7, QA §6).

    drive.py <stage> <gateway_port> <expose-csv> [lan_ip]

Every assertion reads a WIRE effect — a status code, a header, a JSON-RPC body,
an SSE frame — never "the call returned". `Ledger` prints PASS/FAIL as it goes
so a run that dies half-way still shows what was settled.

Only `list_changed` and `auth` open a WebSocket, and only for the RPCs that
have no HTTP face (plugin toggling, fetching the shared token). `deny` opens
none on purpose: a loopback WS client is an operator surface, and the claim is
about what happens when there is none.
"""
import asyncio
import json
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger  # noqa: E402

import websockets  # noqa: E402

STAGE = sys.argv[1]
PORT = int(sys.argv[2])
EXPOSE = [n for n in sys.argv[3].split(",") if n]
LAN_IP = sys.argv[4] if len(sys.argv) > 4 else ""
LOCAL = f"http://127.0.0.1:{PORT}"
WS = f"ws://127.0.0.1:{PORT}/ws"
SUPPORTED = ["2025-11-25", "2025-06-18", "2025-03-26"]
L = Ledger()


class Mcp:
    """One MCP client over Streamable HTTP. Holds the session id once
    `initialize` mints it and sends it on every later request."""

    def __init__(self, base, bearer=None):
        self.base, self.bearer, self.session, self.n = base, bearer, None, 0

    def _headers(self, extra=None):
        h = {"content-type": "application/json", "accept": "application/json, text/event-stream"}
        if self.bearer:
            h["authorization"] = f"Bearer {self.bearer}"
        if self.session:
            h["mcp-session-id"] = self.session
        h.update(extra or {})
        return h

    def request(self, method, body=None, headers=None, timeout=45):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + "/mcp", data=data, method=method, headers=self._headers(headers))
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                raw = r.read()
                return r.status, {k.lower(): v for k, v in r.headers.items()}, (json.loads(raw) if raw else None)
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                parsed = json.loads(raw) if raw else None
            except ValueError:
                parsed = raw.decode(errors="replace")
            return e.code, {k.lower(): v for k, v in e.headers.items()}, parsed

    def call(self, method, params=None, id=None):
        self.n += 1
        msg = {"jsonrpc": "2.0", "id": self.n if id is None else id, "method": method}
        if params is not None:
            msg["params"] = params
        return self.request("POST", msg)

    def notify(self, method, params=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        return self.request("POST", msg)

    def initialize(self, version="2025-03-26", client="qa-mcp-face"):
        st, hd, body = self.call("initialize", {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": client, "version": "0"}})
        sid = hd.get("mcp-session-id")
        if st == 200 and sid:
            self.session = sid
        return st, sid, body

    def tools(self):
        st, _, body = self.call("tools/list", {})
        return st, body, {t["name"]: t for t in (body or {}).get("result", {}).get("tools", [])}

    def sse(self):
        """Open GET /mcp on a thread; returns (frames, close). `frames` fills
        with every `data:` line's JSON as it arrives."""
        frames, stop = [], threading.Event()
        req = urllib.request.Request(self.base + "/mcp", method="GET", headers=self._headers({"accept": "text/event-stream"}))
        resp = urllib.request.urlopen(req, timeout=120)
        status = resp.status

        def pump():
            try:
                while not stop.is_set():
                    line = resp.readline()
                    if not line:
                        break
                    line = line.decode(errors="replace").rstrip("\r\n")
                    if line.startswith("data:"):
                        try:
                            frames.append(json.loads(line[5:].strip()))
                        except ValueError:
                            frames.append({"raw": line})
            except Exception as e:  # noqa: BLE001 — the pump's job is to keep reading until closed
                frames.append({"pump_error": str(e)})

        t = threading.Thread(target=pump, daemon=True)
        t.start()

        def close():
            stop.set()
            try:
                resp.close()
            except Exception:  # noqa: BLE001
                pass

        return status, frames, close


def wait_for(frames, method, secs=10.0):
    deadline = time.time() + secs
    while time.time() < deadline:
        if any(f.get("method") == method for f in frames):
            return True
        time.sleep(0.2)
    return False


async def ws_rpc(method, params):
    async with websockets.connect(WS, max_size=None, ping_interval=None) as ws:
        n = 0

        async def call(m, p):
            nonlocal n
            n += 1
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": n, "method": m, "params": p}))
            while True:
                msg = json.loads(await ws.recv())
                if msg.get("id") == n:
                    return msg

        await call("connect", {"client": "qa-mcp-face", "version": "1"})
        return await call(method, params)


def rpc(method, params):
    return asyncio.run(ws_rpc(method, params))


def text_of(body):
    try:
        return body["result"]["content"][0]["text"]
    except (KeyError, IndexError, TypeError):
        return json.dumps(body)[:400]


# ── stages ─────────────────────────────────────────────────────────────────

def stage_handshake():
    for v in SUPPORTED:
        c = Mcp(LOCAL)
        st, sid, body = c.initialize(v)
        r = (body or {}).get("result", {})
        L.check(f"initialize {v}: 200 with a session id", st == 200 and bool(sid), f"status={st} sid={sid}")
        L.check(f"initialize {v}: protocolVersion echoed", r.get("protocolVersion") == v, json.dumps(r)[:200])
        L.check(f"initialize {v}: capabilities is exactly tools.listChanged",
                r.get("capabilities") == {"tools": {"listChanged": True}}, json.dumps(r.get("capabilities")))
        L.check(f"initialize {v}: serverInfo.name == aleph", r.get("serverInfo", {}).get("name") == "aleph")
        st, _, _ = c.notify("notifications/initialized")
        L.check(f"{v}: notifications/initialized is 202", st == 202, f"status={st}")
        st, _, body = c.call("ping", {}, id="ping-1")
        L.check(f"{v}: ping answers {{}} and echoes the string id",
                st == 200 and body.get("result") == {} and body.get("id") == "ping-1", json.dumps(body)[:200])
        st, _, _ = c.request("DELETE")
        L.check(f"{v}: DELETE ends the session", st == 200, f"status={st}")

    c = Mcp(LOCAL)
    st, sid, body = c.initialize("1999-01-01")
    L.check("an unsupported version is answered with 2025-11-25",
            st == 200 and body["result"]["protocolVersion"] == "2025-11-25", json.dumps(body)[:200])
    st, _, body = Mcp(LOCAL).call("tools/list", {})
    L.check("a request without a session is 400", st == 400, f"status={st}")
    c2 = Mcp(LOCAL)
    c2.session = "00000000-0000-0000-0000-000000000000"
    st, _, _ = c2.call("tools/list", {})
    L.check("an unknown session is 404", st == 404, f"status={st}")


def stage_tools():
    c = Mcp(LOCAL)
    st, _, _ = c.initialize("2025-06-18")
    L.check("initialize", st == 200)
    st, body, tools = c.tools()
    L.check("tools/list is 200", st == 200, f"status={st}")
    strangers = sorted(set(tools) - set(EXPOSE))
    L.check("tools/list ⊆ expose", not strangers, f"not in expose: {strangers}")
    for name in ("grep", "file_read", "agent_list"):
        L.check(f"{name} is listed with an object inputSchema",
                tools.get(name, {}).get("inputSchema", {}).get("type") == "object",
                json.dumps(tools.get(name))[:200])
    L.check("bash is NOT listed", "bash" not in tools)
    st, _, body = c.call("tools/call", {"name": "agent_list", "arguments": {}})
    r = (body or {}).get("result", {})
    L.check("agent_list returns a text content block, isError false",
            st == 200 and r.get("isError") is False and r.get("content", [{}])[0].get("type") == "text",
            json.dumps(body)[:300])
    L.check("the text names the main agent", "main" in text_of(body), text_of(body)[:200])


def stage_auth():
    if not LAN_IP:
        print("SKIP  auth: no non-loopback address on this host (every remote assertion is UNRUN, not PASS)")
        return
    remote = f"http://{LAN_IP}:{PORT}"
    st, hd, _ = Mcp(remote).initialize()
    L.check("remote without a bearer is 401", st == 401, f"status={st}")
    L.check("…with WWW-Authenticate: Bearer", hd.get("www-authenticate", "").startswith("Bearer"), str(hd.get("www-authenticate")))
    st, _, _ = Mcp(remote, bearer="aleph-not-a-token").initialize()
    L.check("remote with a wrong bearer is 401", st == 401, f"status={st}")

    cur = rpc("gateway.token.current", {})
    token = (cur.get("result") or {}).get("token")
    if not token:
        token = rpc("gateway.token.rotate", {})["result"]["token"]
    L.check("a shared gateway token exists", bool(token))
    c = Mcp(remote, bearer=token)
    st, sid, body = c.initialize()
    L.check("remote with the shared token is admitted with a session", st == 200 and bool(sid), f"status={st} body={json.dumps(body)[:200]}")
    st, _, tools = c.tools()
    L.check("…and can list tools", st == 200 and bool(tools), f"status={st} n={len(tools)}")
    st, sid, _ = Mcp(LOCAL).initialize()
    L.check("loopback still needs nothing", st == 200 and bool(sid), f"status={st}")


def stage_list_changed():
    c = Mcp(LOCAL)
    st, _, _ = c.initialize("2025-11-25")
    L.check("initialize", st == 200)
    st, _, tools = c.tools()
    L.check("the planted plugin tool is listed while enabled", "qa_mcp_probe" in tools, sorted(tools)[:20])
    sse_status, frames, close = c.sse()
    L.check("GET /mcp opens an event stream", sse_status == 200, f"status={sse_status}")
    try:
        off = rpc("tools.invoke", {"tool_name": "plugin_manage", "arguments": {"action": "disable", "name": "qa-mcp-probe"}})
        L.check("plugin_manage disable reported ok", "error" not in off, json.dumps(off)[:300])
        L.check("an SSE notifications/tools/list_changed frame arrives within 10 s",
                wait_for(frames, "notifications/tools/list_changed"), json.dumps(frames)[:300])
        st, _, tools = c.tools()
        L.check("the tool left tools/list", "qa_mcp_probe" not in tools, sorted(tools)[:20])
        frames.clear()
        on = rpc("tools.invoke", {"tool_name": "plugin_manage", "arguments": {"action": "enable", "name": "qa-mcp-probe"}})
        L.check("plugin_manage enable reported ok", "error" not in on, json.dumps(on)[:300])
        L.check("a second list_changed frame arrives", wait_for(frames, "notifications/tools/list_changed"), json.dumps(frames)[:300])
        st, _, tools = c.tools()
        L.check("the tool is back in tools/list", "qa_mcp_probe" in tools, sorted(tools)[:20])
    finally:
        close()


def stage_deny():
    c = Mcp(LOCAL)
    st, _, _ = c.initialize()
    L.check("initialize", st == 200)
    st, _, body = c.call("tools/call", {"name": "bash", "arguments": {"command": "id"}})
    err = (body or {}).get("error") or {}
    L.check("an unexposed tool is JSON-RPC -32602, not executed", st == 200 and err.get("code") == -32602, json.dumps(body)[:300])
    st, _, tools = c.tools()
    L.check("precondition: agent_delete is exposed AND registered on this host", "agent_delete" in tools, sorted(tools)[:30])
    t0 = time.time()
    st, _, body = c.call("tools/call", {"name": "agent_delete", "arguments": {"agent_id": "qa-no-such-agent"}})
    took = time.time() - t0
    r = (body or {}).get("result") or {}
    L.check("a confirmation-gated tool with no operator surface is isError:true", st == 200 and r.get("isError") is True, json.dumps(body)[:400])
    L.check("…and answered at once, not after a parked card", took < 5, f"took {took:.1f}s")
    L.check("…the reason says nobody was asked", "(Unavailable)" in text_of(body), text_of(body)[:300])
    L.check("…and names the Aleph Panel as where to grant it", "Aleph Panel" in text_of(body), text_of(body)[:300])


STAGES = {
    "handshake": stage_handshake,
    "tools": stage_tools,
    "auth": stage_auth,
    "list_changed": stage_list_changed,
    "deny": stage_deny,
}
STAGES[STAGE]()
sys.exit(len(L.failures))
```

- [ ] **Step 3: Write `qa/mcp_face/run.sh`**

```bash
#!/usr/bin/env bash
# Real-machine QA for the MCP server face (spec §3.7 / §6).
#
#   ./qa/mcp_face/run.sh handshake     # three versions negotiate; unsupported → newest;
#                                      # sessions minted / 400 / 404; 202 for notifications
#   ./qa/mcp_face/run.sh tools         # tools/list ⊆ expose on the REAL registry; a read-only
#                                      # call answers text, isError:false
#   ./qa/mcp_face/run.sh auth          # bound to 0.0.0.0: LAN request 401 without / with a
#                                      # wrong bearer; the shared gateway token is admitted
#                                      # (SKIP when the host has no non-loopback address)
#   ./qa/mcp_face/run.sh list_changed  # plugin_manage disable/enable → SSE list_changed +
#                                      # the plugin's tool leaves/re-enters tools/list
#   ./qa/mcp_face/run.sh deny          # unexposed tool → -32602; a confirmation-gated tool with
#                                      # NO operator surface → isError:true naming the Panel
#
# Why real-machine: the unit tests drive a stub registry through the router
# in-process. Only a booted daemon can show that the face was actually
# INSTALLED (a slot boot never fills is a 404 with nothing red), that the
# expose list survives `config.toml`, that the real registry's schemas arrive,
# and that the attendance probe reads the real connection table.
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT (two processes
# on one vault is the documented way to lose vault data — PROCESS_MANAGEMENT.md).
set -uo pipefail

STAGE="${1:-handshake}"
case "$STAGE" in handshake|tools|auth|list_changed|deny) ;; *)
  echo "unknown stage '$STAGE' (handshake|tools|auth|list_changed|deny)" >&2; exit 64;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-mcp-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18831}"
MOCK_PORT="${MOCK_PORT:-18832}"   # nothing listens; the config must merely not name a real provider

# Build BEFORE HOME is redirected (cargo's caches live under the real HOME).
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
INSTALLED="$ALEPH_HOME/plugins/installed"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
say() { printf '\n=== %s ===\n' "$*"; }
stop_server() {
  [ -n "$SERVER_PID" ] || return 0
  kill "$SERVER_PID" 2>/dev/null
  for _ in $(seq 1 30); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}
cleanup() {
  stop_server
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! qa_build -p alephcore --bin aleph-server; then
    echo "build failed" >&2; exit 1
  fi
fi
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
# `--port` on the GENERATION boot: without it this boot binds the built-in
# default port and dies if anything already holds it (see qa/plugins/run.sh).
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

# The exposure under test. ONE spelling, shared with the driver, so
# `tools/list ⊆ expose` is asserted against the same list the server read.
EXPOSE="agent_list,agent_info,grep,find,file_read,session_list,skill_list"
LAN_FLAG=""
case "$STAGE" in
  list_changed) EXPOSE="$EXPOSE,qa_mcp_probe" ;;
  deny)         EXPOSE="$EXPOSE,agent_delete" ;;   # confirmation-gated (CONFIRMATION_REQUIRED_TOOLS)
  auth)         LAN_FLAG="--lan" ;;
esac
python3 "$HERE/patch_mcp.py" "$CONFIG" --expose "$EXPOSE" $LAN_FLAG || exit 1

if [ "$STAGE" = "list_changed" ]; then
  say "plant a static plugin with one [[tools]] entry"
  mkdir -p "$INSTALLED/qa-mcp-probe"
  cat >"$INSTALLED/qa-mcp-probe/aleph.plugin.toml" <<'TOML'
[plugin]
id = "qa-mcp-probe"
name = "QA MCP probe"
version = "0.0.1"
kind = "static"
entry = "SKILL.md"

[[tools]]
name = "qa_mcp_probe"
description = "a tool whose only job is to appear and disappear in tools/list"
handler = "probe"
TOML
  printf '# QA probe\n' >"$INSTALLED/qa-mcp-probe/SKILL.md"
fi

LAN_IP=""
if [ "$STAGE" = "auth" ]; then
  # A UDP "connect" picks the interface the kernel would route through
  # without sending a packet (same trick as qa/spend_budget/run.sh).
  LAN_IP="$(python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    s.connect(("8.8.8.8", 80)); ip = s.getsockname()[0]
except OSError:
    ip = ""
finally:
    s.close()
print("" if ip.startswith("127.") else ip)
PY
)"
  [ -n "$LAN_IP" ] || echo "no non-loopback address on this host; the remote assertions will report SKIP" >&2
fi

say "start server"
# stdout is not a TTY here, so tracing goes to $ALEPH_HOME/logs/; the
# redirect below catches only the startup banner.
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "the face was INSTALLED (not merely compiled)"
# The banner line is printed by start/mod.rs only on the install arm; a
# declined face (simulated mode, enabled=false) prints nothing here and every
# stage below would 404 with nothing else red.
if grep -q "MCP server face: /mcp" "$QA_ROOT/server.log"; then
  echo "  [PASS] boot installed the MCP face"
else
  echo "  [FAIL] boot did not install the MCP face (Mode: Simulated? enabled=false?)"
  grep -n "Mode:" "$QA_ROOT/server.log" | head -3
  exit 1
fi
if grep -q "does not know at boot" "$ALEPH_HOME"/logs/*.log 2>/dev/null; then
  echo "  [WARN] boot reported an unknown expose name:"; grep -h "does not know at boot" "$ALEPH_HOME"/logs/*.log | head -3
fi

say "drive: $STAGE"
RC=0
python3 -u "$HERE/drive.py" "$STAGE" "$GATEWAY_PORT" "$EXPOSE" "$LAN_IP" || RC=$?

say "server log tail"
tail -5 "$QA_ROOT/server.log"
exit "$RC"
```

`chmod +x qa/mcp_face/run.sh qa/mcp_face/drive.py qa/mcp_face/patch_mcp.py`.

- [ ] **Step 4: `qa/README.md` entries**

After the `./qa/plugins/run.sh trust` row (`qa/README.md:623-624`) add:

```
./qa/mcp_face/run.sh handshake   # MCP server face: three versions negotiate to themselves,
                                 # unsupported → 2025-11-25, sessions minted / 400 / 404
./qa/mcp_face/run.sh tools       # tools/list ⊆ [mcp_server].expose on the REAL registry;
                                 # a read-only call answers text, isError:false
./qa/mcp_face/run.sh auth        # LAN request: 401 without / with a wrong bearer, the shared
                                 # gateway token admitted (SKIP without a LAN address)
./qa/mcp_face/run.sh list_changed # plugin_manage disable/enable → SSE list_changed and the
                                 # plugin's tool leaves / re-enters tools/list
./qa/mcp_face/run.sh deny        # unexposed → -32602; confirmation-gated with NO operator
                                 # surface → isError:true naming the Panel
```

In「每个装置在证明什么」(after the `browser_managed` bullet) add:

```
- **`mcp_face`** — 改 `src/gateway/mcp_face/` 或 `[mcp_server]` 前跑 `{handshake,tools,auth,list_changed,deny}`。
  单测把一个 stub 注册表推过路由器就能全绿；只有真机能证明**面被安装了**（slot 没装 = 404 且没有一处变红——
  `run.sh` 先 grep 启动横幅再驱动）、`expose` 真从 `config.toml` 读进来、真注册表的 schema 到达了
  `tools/list`、以及**出席探针读的是真连接表**。`deny` 阶段**刻意不开任何 WebSocket**——loopback 的 WS
  客户端本身就是一个 operator 面，开着它去断言「无人可批」等于断言一个没被武装的闸。`list_changed`
  证的是 `lifecycle.rs::after_transition` 里那一行 `try_mcp_face()` 真的把通知送到了一条打开的 SSE 流上——
  单测装的是 stub 面，只有真机能证明 boot 装的那个面就是 lifecycle 通知的那个。
```

- [ ] **Step 5: Run the fixture**

Run, in order, from the worktree (its own `target/`; no `CARGO_TARGET_DIR`):

```
./qa/mcp_face/run.sh handshake
./qa/mcp_face/run.sh tools
./qa/mcp_face/run.sh auth
./qa/mcp_face/run.sh deny
./qa/mcp_face/run.sh list_changed
```

Expected: exit 0 and every line `[PASS]`; `auth` prints one `SKIP` line (and exits 0) on a host with no LAN address. Record the first run's findings in the commit message — first runs of this repo's fixtures have found a defect every time (qa/README.md「What each scenario has actually proved」).

- [ ] **Step 6: Mutation step (the fixture must be able to go red)**

With `KEEP=1 SKIP_BUILD=1`, edit the scratch `config.toml` to `expose = ["grep"]` only and re-run `tools` → Expected: `[FAIL] file_read is listed…`, `[FAIL] agent_list…` (the driver's `EXPOSE` no longer matches the server's). Restore. Then temporarily change `MCP_APPROVAL_HINT` to drop the words "Aleph Panel", rebuild, run `deny` → Expected: `[FAIL] …and names the Aleph Panel as where to grant it`. Revert.

- [ ] **Step 7: Commit**

```bash
git add qa/mcp_face/run.sh qa/mcp_face/drive.py qa/mcp_face/patch_mcp.py qa/README.md
git commit -m "qa/mcp_face: real-machine stages handshake/tools/auth/list_changed/deny for the MCP server face

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
## Contract deltas

1. `try_mcp_face() -> Option<&'static Arc<McpFace>>` (G-4 adopted); `install_mcp_face(Arc<McpFace>)` + `decline_mcp_face(&'static str)` added (F8's `CapabilitySlot` needs a decline arm and a roster row `gateway/mcp-face`).
2. `McpServerConfig` does NOT derive `Default`; it implements it (`enabled: true`, `expose: default_expose()`), and derives `JsonSchema` + `PartialEq, Eq` like every other config section. `default_expose()` is derived (`READ_ONLY_TOOLS ∩ BUILTIN_TOOL_DEFINITIONS − OPERATOR_TOOLS − desktop_* − DEFAULT_EXPOSE_EXCLUDES`), the four by-name exclusions carry reasons (R6.6), and the result is pinned name-by-name in a test — not a hand-written list in the config type.
3. HTTP library = **axum 0.8** (`Router::merge`, `axum::response::sse`); no new dependency (`arc-swap` and `tokio-stream` are already dependencies).
4. JSON-RPC envelope = `crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse}` (ids are `Option<Value>`), not `src/mcp/jsonrpc.rs` (its request id is `u64`). Payload types = `crate::mcp::protocol::*` as the contract says.
5. Protocol versions = `["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION]`; the newest literal in `src/mcp/` (`2026-07-28`, `modern/`) is handshake-less/sessionless and is **not** spoken (R6.2: deferred; `server/discover` → `-32601`, so an `auto`-probing adapter falls back to `initialize`).
6. Principal: there is no principal enum to extend. `McpClient { client_name, client_version }` is a record on the MCP session; authority is `McpCaller { role, user, is_local, device_id }` resolved per request by the `connect` rules (`resolve_connect_auth` + `resolve_connection_identity`), scoped through the extracted `caller_identity::with_caller_identity`. The spend ledger charges the resolved `user`; no ledger/gate `match` arm was extended because none exists to extend.
7. `McpFace` holds `Arc<dyn ToolRegistry>`; `registry_adapter::{RegistryToolAdapter, build_tool_adapters_from_tools, build_registry_from_tools}` widen their bound to `R: ToolRegistry + ?Sized + 'static` (4 lines, sized callers unaffected).
8. Visibility widenings: `server::handler` → `pub(crate) mod`; `handler::refuse_insecure_remote`, `execution_engine::tool_refresh` (+ `plugin_tool_to_unified_tool`), `tool_service_builder::mcp_tool_registry`, `turn_permissions::resolve_exec_tier` → `pub(crate)`; `mcp_face::test_support` is `#[cfg(test)] pub(crate)` so the lifecycle wire test reuses the stub face.
9. `McpFace::new` takes an `OperatorPresence` probe (`GatewayServer::operator_presence_probe()` over the connection table) and derives `unattended = !present` per call — "approval-needed-without-operator → isError" goes through the existing unattended arm, not through `OperatorApprovalRequester`'s zero-subscriber deny (which never fires on a real server, F6).
10. `[mcp_server].expose` is **live** (P6.9): `LIVE_SUBSECTIONS` gains `"mcp_server.expose"`, `McpFace.expose` is an `ArcSwap<BTreeSet<String>>`, `apply_expose` re-runs G5 and broadcasts once per open stream. `[mcp_server].enabled` stays `Restart`.
11. The `after_transition` → `notify_tools_list_changed` line is **P6.4's** (R6.1), anchored on plan-P1's `lifecycle.rs::after_transition` (P1.9); P6.4 appends one tuple to P1.14's module-level `projection.rs::tests::G3_PINNED` slice in the same commit.
12. `packages/pi-aleph/package.json#pi.skills` is an **array** (`["./skills"]`), per `PiManifest.skills?: string[]`.
13. `qa/mcp_face` drivers use `python3` + `websockets` (the sibling fixtures' choice), not `jq`.
14. Remote `POST /mcp` carries a private `RateLimiter` bucket (`MCP_REMOTE_POSTS_PER_MINUTE = 120`, loopback exempt) in the artifact route's shape (R6.4).

## Open questions for the lead

Answered by the rulings and folded into the tasks: Q1 (`2026-07-28` deferred, R6.2), Q2 (dangling card accepted + documented, U-d — P6.4 module doc, P6.8 README), Q3 (live-apply → P6.9), Q4 (limiter → P6.6), Q5 (hint stays; structural variant is a follow-up, R6.5), Q6 (three names subtracted with reasons, R6.6), Q8 (the wire is P6.4's, R6.1).

Still flagged, not built (recorded follow-ups, per the rulings):
1. Retire an in-flight approval card when the `/mcp` handler future drops (client timeout) — U-d says accept; noted in P6.4's module doc and P6.8's README.
2. A structural `ToolError::ApprovalUnavailable` so the face replaces the unattended arm's chat-model hint instead of pattern-matching `(Unavailable)` — R6.5 follow-up; the `UNAVAILABLE_MARKER` pin test is what stands in for it until then.
3. `2026-07-28` (`server/discover` + per-request `_meta`, no sessions) — R6.2 deferred.
4. The GATEWAY.md "MCP 面" first-paragraph pointer to `packages/pi-aleph/` (plan-P5P8's matrix row) is left to **P8.5**: that section does not exist when P6.8 commits; P6.8 writes the EXTENSION_SYSTEM sentence and P6.7 the HTTP-route bullet.

## Coverage map

| Spec item | Task(s) |
|---|---|
| §3.7 传输 (Streamable HTTP, `/mcp`, POST + SSE + `Mcp-Session-Id`) | P6.2, P6.6, P6.7 |
| §3.7 协议版本 (negotiate; ≥ `2025-03-26` + newest handshake revision) | P6.5 |
| §3.7 能力 (`tools.listChanged` only, short `instructions`) | P6.5 |
| §3.7 认证 (loopback free; remote bearer; one trust model) + remote-only 429 | P6.3, P6.6 |
| §3.7 暴露 (`[mcp_server] enabled / expose`, default side-effect-free set, named, reasoned excludes) | P6.1 |
| §3.7 执行 (session ↔ Aleph session; principal; same scoped dispatch; approval → card or `isError`; dangling card documented) | P6.2, P6.4, P6.8 |
| §3.7 名字 (≤ 64 chars for dsh) | P6.1 (`default_names_stay_under_dshs_truncation_threshold`) |
| §3.7 通知 (lifecycle transition → `list_changed`) | P6.4 (`notify_tools_list_changed` + the `after_transition` line + the `G3_PINNED` tuple), P6.2 (fan-out) |
| §3.7 通知 (`expose` 配置变更 → `list_changed`) | P6.9 |
| §3.5 G5 (`expose ⊆ 工具目录`, startup validation + test, mutation) | P6.1 (unit half + mutation), P6.4 (`unknown_expose`), P6.7 (boot warn), P6.9 (re-run on live change) |
| §3.8 `packages/pi-aleph/` (JS-free; pi + Claude Code + dsh rows) | P6.8 |
| §4 MCP error rows (`isError` for execution errors; 401; 404; not-found for unexposed; version fallback) | P6.4, P6.5, P6.6 |
| §6 `qa/mcp_face/run.sh` {handshake, tools, auth, list_changed, deny} + `qa/README.md` | P7.1 |
| G-6 doc-code 同笔 rows for P6 (GATEWAY.md HTTP bullet; EXTENSION_SYSTEM pi-aleph pointer) | P6.7, P6.8 |
| CLAUDE.md 禁用清单 "第二个 MCP server 实现" + `src/gateway/mcp_face/` 路由行; GATEWAY.md "MCP 面" section; FL §5.27 | P8 (P8.5 / P8.7 / P8.8); the module docs in P6.4 state the rule |
