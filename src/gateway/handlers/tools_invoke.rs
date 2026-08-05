//! Tools Invoke Handler
//!
//! Provides `tools.invoke` JSON-RPC method for direct execution of a builtin
//! tool by name, bypassing the LLM agent loop. Intended for E2E tests and
//! deterministic tool exercising — production callers should still go through
//! the agent loop (R8 LLM Sovereignty).
//!
//! ## Request
//! ```json
//! {"tool_name": "memory_search", "arguments": {"query": "foo"}, "agent_id": "main"}
//! ```
//!
//! ## Response (success)
//! ```json
//! {"ok": true, "tool_name": "memory_search", "result": {...}}
//! ```
//!
//! ## Response (tool error)
//! Returns RPC error with `INTERNAL_ERROR` code and the tool's error message.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, INTERNAL_ERROR, INVALID_PARAMS,
};
use super::parse_params;
use crate::agents::AgentRegistry;
use crate::executor::ToolRegistry;

/// Parameters for `tools.invoke`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InvokeParams {
    /// Tool name as registered in the `BuiltinToolRegistry` (e.g. "`memory_search`", "`note_manage`").
    pub tool_name: String,
    /// Arguments forwarded to the tool. Schema depends on the tool.
    #[serde(default)]
    pub arguments: Value,
    /// Optional `agent_id`; merged into `arguments.agent_id` when present and the
    /// arguments object doesn't already carry one. Tools that read `agent_id`
    /// (e.g. `note_manage`) pick it up automatically.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Real handler — executes the tool directly via the registry trait.
///
/// `agents` is optional: when present, the request's `agent_id` (default
/// `"main"`) must resolve to an `AgentDef` and the requested `tool_name`
/// must pass `AgentDef::is_tool_allowed`. When `None` the allowlist gate
/// is skipped (test mode / legacy callers). The production boot path
/// always supplies the live registry — see `agent_init.rs`.
pub async fn handle_invoke<R>(
    request: JsonRpcRequest,
    registry: Arc<R>,
    agents: Option<Arc<AgentRegistry>>,
) -> JsonRpcResponse
where
    R: ToolRegistry + ?Sized,
{
    let params: InvokeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if params.tool_name.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "tool_name must not be empty");
    }

    // Transport hard floor. This handler dispatches straight off the raw
    // `ToolRegistry`, so most of the loop's gates (exec tier, tool_permissions,
    // the confirmation card) do not run here — and this surface has no
    // approval transport to raise a card with. Two classes are therefore
    // refused outright: RCE / host-mutation / self-reconfiguration tools
    // (openclaw `dangerous-tools` parity) and tools that self-declare
    // `requires_confirmation`. Production agents reach both through the agent
    // loop, which does have the gates. Re-enable a specific tool via the
    // `ALEPH_GATEWAY_TOOLS_ALLOW` env var. (The operator gate is the one
    // exception — see the third hard floor below, P1 Task 9.)
    if crate::security::dangerous_tools::is_denied_on_gateway_surface(
        &params.tool_name,
        &params.arguments,
    ) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "tool '{}' is denied on the gateway tools.invoke surface \
                 (dangerous, confirmation-gated, or an argument-level approval \
                 this surface cannot raise; set {} to override)",
                params.tool_name,
                crate::security::dangerous_tools::GATEWAY_TOOLS_ALLOW_ENV
            ),
        );
    }

    // Second hard floor: continuation-driven tools (`loop` / `goal`). This
    // handler returns before any post-run continuation hook, exactly like the
    // L0 direct-tool fast path both slash surfaces already exclude them from
    // (`is_continuation_driven_slash`, whose contract says "on ANY surface").
    // Invoked here, `loop(action='start')` registers state whose first tick is
    // never scheduled — and with no task-local session key in play the tool
    // cannot even name the session it registered against. Fail closed with the
    // reason; the same `ALEPH_GATEWAY_TOOLS_ALLOW` escape hatch applies.
    if crate::gateway::execution_engine::is_continuation_driven_slash(&params.tool_name)
        && !crate::security::dangerous_tools::gateway_surface_override(&params.tool_name)
    {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "tool '{}' is continuation-driven and denied on the gateway \
                 tools.invoke surface: this surface returns before the post-run \
                 hook, so the loop/goal would register but never be scheduled. \
                 Drive it through the agent loop (set {} to override)",
                params.tool_name,
                crate::security::dangerous_tools::GATEWAY_TOOLS_ALLOW_ENV
            ),
        );
    }

    // Third hard floor: operator-tier tools (`OPERATOR_TOOLS`,
    // `method_authz.rs` — self-config, cron, agent identity, cluster
    // membership, …). This handler dispatches straight off the raw
    // `ToolRegistry` (see the module doc above) and never reaches
    // `ScopedToolService::check_operator_gate`, so without this check a
    // member-authorized Panel connection could invoke e.g. `cron_manage`
    // directly — the exact C2 escalation `method_admin.rs` used to fend off
    // by blanket-gating the whole `tools.` family at the RPC layer. That
    // blanket gate is now narrowed to carve `tools.invoke` open (P1 member
    // hardening, Task 9); this is the enforcement that makes the carve-out
    // safe. Reuses the SAME predicate the agent loop's own gate uses —
    // `method_authz::tool_requires_operator` +
    // `turn_context::role_is_operator` — against `caller_role`, which is
    // already ambient here (scoped around every dispatched request at
    // `process_request`, P0/Task 3): nothing new is stamped, this handler
    // simply never consulted what was already available. Absent role (no
    // gateway connection — cron/internal/local no-auth daemon) is trusted,
    // exactly like every other operator gate in this codebase.
    if crate::gateway::method_authz::tool_requires_operator(&params.tool_name) {
        let caller_role = crate::gateway::caller_identity::current_caller_role();
        if !crate::tools::turn_context::role_is_operator(caller_role.as_deref()) {
            return JsonRpcResponse::error(
                request.id,
                AUTH_REQUIRED,
                format!(
                    "tool '{}' changes Aleph's own configuration and requires an \
                     operator-authorized connection; this caller is not operator-tier. \
                     Do not retry.",
                    params.tool_name
                ),
            );
        }
    }

    // Allowlist gate — applied only when caller supplied an agent registry.
    if let Some(ref agents) = agents {
        let resolved_id = params
            .agent_id
            .clone()
            .unwrap_or_else(|| "main".to_string());
        let agent_def = match agents.get(&resolved_id) {
            Some(d) => d,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("unknown agent_id: {resolved_id}"),
                );
            }
        };
        if !agent_def.is_tool_allowed(&params.tool_name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "tool '{}' not allowed for agent '{}'",
                    params.tool_name, resolved_id
                ),
            );
        }
    }

    let arguments = merge_agent_id(params.arguments, params.agent_id.as_deref());

    match registry.execute_tool(&params.tool_name, arguments).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "tool_name": params.tool_name,
                "result": result,
            }),
        ),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("tool '{}' failed: {}", params.tool_name, err),
        ),
    }
}

/// If the caller supplied a top-level `agent_id`, fold it into the JSON
/// arguments object under the `agent_id` key. Existing values win so the
/// caller can still override per-call. Non-object arguments pass through
/// unchanged (the tool will reject them on its own schema check).
fn merge_agent_id(mut arguments: Value, agent_id: Option<&str>) -> Value {
    let Some(agent_id) = agent_id else {
        return arguments;
    };
    if let Value::Object(ref mut map) = arguments {
        map.entry("agent_id".to_string())
            .or_insert(Value::String(agent_id.to_string()));
    }
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result as AlephResult;
    use crate::sync_primitives::Mutex;
    use crate::tool_metadata::UnifiedTool;
    use std::collections::HashMap;

    /// Minimal in-test ToolRegistry returning canned values keyed by tool name.
    /// Returns an error for unknown tools so we can exercise the error path.
    struct StubRegistry {
        results: Mutex<HashMap<String, AlephResult<Value>>>,
        last_args: Mutex<Option<(String, Value)>>,
    }

    impl StubRegistry {
        fn new() -> Self {
            Self {
                results: Mutex::new(HashMap::new()),
                last_args: Mutex::new(None),
            }
        }
        fn with_ok(self, name: &str, value: Value) -> Self {
            self.results
                .lock()
                .unwrap()
                .insert(name.to_string(), Ok(value));
            self
        }
        fn last_call(&self) -> Option<(String, Value)> {
            self.last_args
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl ToolRegistry for StubRegistry {
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None
        }
        fn execute_tool(
            &self,
            tool_name: &str,
            arguments: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AlephResult<Value>> + Send + '_>>
        {
            *self.last_args.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((tool_name.to_string(), arguments.clone()));
            let canned = self
                .results
                .lock()
                .unwrap()
                .get(tool_name)
                .map(|r| match r {
                    Ok(v) => Ok(v.clone()),
                    Err(e) => Err(crate::error::AlephError::tool(e.to_string())),
                })
                .unwrap_or_else(|| {
                    Err(crate::error::AlephError::tool(format!(
                        "unknown tool: {tool_name}"
                    )))
                });
            Box::pin(async move { canned })
        }
    }

    #[tokio::test]
    async fn rejects_missing_params() {
        let reg = Arc::new(StubRegistry::new());
        let req = JsonRpcRequest::with_id("tools.invoke", None, json!(1));
        let resp = handle_invoke(req, reg, None).await;
        assert!(!resp.is_success(), "expected error response");
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn rejects_empty_tool_name() {
        let reg = Arc::new(StubRegistry::new());
        let params = json!({"tool_name": "  ", "arguments": {}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg, None).await;
        assert!(!resp.is_success());
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn denies_dangerous_tool_on_gateway_surface() {
        // Transport hard floor: an RCE tool is refused before the registry
        // is ever touched, even with no agent allowlist supplied.
        std::env::remove_var(crate::security::dangerous_tools::GATEWAY_TOOLS_ALLOW_ENV);
        let reg = Arc::new(StubRegistry::new().with_ok("bash", json!({"ok": true})));
        let params = json!({"tool_name": "bash", "arguments": {}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg.clone(), None).await;
        assert!(!resp.is_success(), "dangerous tool must be denied");
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
        assert!(
            reg.last_call().is_none(),
            "registry must not be touched when the hard floor denies"
        );
    }

    /// `loop` and `goal` register long-running state whose FIRST tick is
    /// claimed by the post-run continuation hook — which this surface returns
    /// before ever reaching. Both slash surfaces already exclude them via
    /// `is_continuation_driven_slash` ("on ANY surface"); this was the third
    /// fast surface that never consulted it.
    #[tokio::test]
    async fn denies_continuation_driven_tools_on_gateway_surface() {
        std::env::remove_var(crate::security::dangerous_tools::GATEWAY_TOOLS_ALLOW_ENV);
        for tool in ["loop", "goal"] {
            let reg = Arc::new(StubRegistry::new().with_ok(tool, json!({"ok": true})));
            let params = json!({"tool_name": tool, "arguments": {"action": "status"}});
            let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
            let resp = handle_invoke(req, reg.clone(), None).await;
            assert!(!resp.is_success(), "{tool} must be denied");
            assert!(
                resp.error.unwrap().message.contains("continuation-driven"),
                "{tool}: the reason must name the actual defect"
            );
            assert!(
                reg.last_call().is_none(),
                "{tool}: registry must not be touched"
            );
        }
    }

    /// `agent_delete` DECLARES `requires_confirmation`, and the loop answers
    /// that declaration with an approval card. This surface has no approval
    /// transport, so it must refuse rather than delete an agent with no card
    /// at any tier — including `ask`.
    #[tokio::test]
    async fn denies_confirmation_gated_tool_on_gateway_surface() {
        std::env::remove_var(crate::security::dangerous_tools::GATEWAY_TOOLS_ALLOW_ENV);
        for tool in ["vault_store", "team_disband"] {
            let reg = Arc::new(StubRegistry::new().with_ok(tool, json!({"ok": true})));
            let params = json!({"tool_name": tool, "arguments": {}});
            let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
            let resp = handle_invoke(req, reg.clone(), None).await;
            assert!(!resp.is_success(), "{tool} must be denied");
            assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
            assert!(
                reg.last_call().is_none(),
                "{tool} must not reach the registry: it needs a card this surface cannot raise"
            );
        }
    }

    #[tokio::test]
    async fn returns_internal_error_when_tool_fails() {
        let reg = Arc::new(StubRegistry::new());
        let params = json!({"tool_name": "missing_tool", "arguments": {}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg, None).await;
        assert!(!resp.is_success());
        let err = resp.error.unwrap();
        assert_eq!(err.code, INTERNAL_ERROR);
        assert!(
            err.message.contains("missing_tool"),
            "error should mention tool name: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn forwards_arguments_and_returns_tool_result() {
        let reg = Arc::new(StubRegistry::new().with_ok(
            "memory_search",
            json!({"hits": [{"id": "n1", "snippet": "hello"}]}),
        ));
        let params = json!({
            "tool_name": "memory_search",
            "arguments": {"query": "hello"},
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg.clone(), None).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["tool_name"], "memory_search");
        assert_eq!(result["result"]["hits"][0]["id"], "n1");
        let (called_name, called_args) = reg.last_call().expect("execute_tool was called");
        assert_eq!(called_name, "memory_search");
        assert_eq!(called_args["query"], "hello");
    }

    #[tokio::test]
    async fn folds_top_level_agent_id_into_arguments() {
        let reg = Arc::new(StubRegistry::new().with_ok("note_manage", json!({"status": "ok"})));
        let params = json!({
            "tool_name": "note_manage",
            "arguments": {"action": "list"},
            "agent_id": "research",
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg.clone(), None).await;
        assert!(resp.is_success());
        let (_, called_args) = reg.last_call().unwrap();
        assert_eq!(called_args["agent_id"], "research");
        assert_eq!(called_args["action"], "list");
    }

    #[tokio::test]
    async fn merges_agent_id_into_arguments_when_absent() {
        let merged = merge_agent_id(json!({"query": "x"}), Some("research"));
        assert_eq!(merged["agent_id"], "research");
    }

    #[tokio::test]
    async fn does_not_overwrite_existing_agent_id() {
        let merged = merge_agent_id(json!({"agent_id": "explicit"}), Some("ignored"));
        assert_eq!(merged["agent_id"], "explicit");
    }

    #[tokio::test]
    async fn passes_through_non_object_arguments() {
        let merged = merge_agent_id(json!("plain string"), Some("any"));
        assert_eq!(merged, json!("plain string"));
    }

    // ---------------------------------------------------------------------
    // D2/P3 — allowlist-gate tests (Some(agents) path)
    // ---------------------------------------------------------------------

    use crate::agents::{AgentDef, AgentMode};

    fn registry_with_restricted_agent() -> Arc<AgentRegistry> {
        let r = AgentRegistry::new();
        r.register(
            AgentDef::new("restricted", AgentMode::SubAgent)
                .with_allowed_tools(vec!["allowed_one".into()]),
        );
        Arc::new(r)
    }

    #[tokio::test]
    async fn blocks_tool_outside_agent_allowlist() {
        let tool_reg = Arc::new(StubRegistry::new().with_ok("blocked_one", json!({})));
        let agents = registry_with_restricted_agent();

        let params = json!({
            "tool_name": "blocked_one",
            "agent_id": "restricted",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg.clone(), Some(agents)).await;

        assert!(
            !resp.is_success(),
            "expected error for out-of-allowlist tool"
        );
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
        assert!(
            tool_reg.last_call().is_none(),
            "registry must not be touched when allowlist denies"
        );
    }

    #[tokio::test]
    async fn permits_tool_inside_agent_allowlist() {
        let tool_reg = Arc::new(StubRegistry::new().with_ok("allowed_one", json!({"hits": 1})));
        let agents = registry_with_restricted_agent();

        let params = json!({
            "tool_name": "allowed_one",
            "agent_id": "restricted",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg, Some(agents)).await;

        assert!(resp.is_success(), "expected success: {:?}", resp.error);
    }

    #[tokio::test]
    async fn skips_allowlist_when_agents_none() {
        // Pre-gate behavior preserved when caller passes None — used by the
        // existing tests above and by simulated-mode wiring.
        let tool_reg = Arc::new(StubRegistry::new().with_ok("anything", json!({})));
        let params = json!({
            "tool_name": "anything",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg, None).await;
        assert!(resp.is_success());
    }

    // ---------------------------------------------------------------------
    // P1 member hardening (Task 9): the operator-tier tool gate.
    //
    // `tools.invoke` dispatches straight off the raw `ToolRegistry` and never
    // passes through `ScopedToolService::check_operator_gate` — verified by
    // reading `execute_inner`/`dispatch.rs`, which this handler simply does
    // not call. The C2 escalation: a member-authorized Panel connection
    // (P0 identity, `CALLER_ROLE == "member"`) could invoke `cron_manage`
    // (an `OPERATOR_TOOLS` entry, `method_authz.rs`) directly, bypassing the
    // gate the agent loop already enforces for that same tool. The fix
    // reuses the identical predicate the loop's gate uses
    // (`method_authz::tool_requires_operator` +
    // `turn_context::role_is_operator`) against the `caller_role` already
    // ambient here (scoped around every dispatched request by
    // `server::handler::dispatch_with_caller_context` — P0/Task 3, nothing
    // new to stamp), so `tools.invoke` can be carved open in
    // `method_admin::MEMBER_CARVE_OUTS` without reopening the escalation.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn member_role_is_denied_an_operator_tier_tool() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("member".to_string()), async {
                let reg = Arc::new(StubRegistry::new().with_ok("cron_manage", json!({"ok": true})));
                let params = json!({"tool_name": "cron_manage", "arguments": {"action": "list"}});
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(
                    !resp.is_success(),
                    "a member must be denied an operator-tier tool"
                );
                assert_eq!(resp.error.unwrap().code, AUTH_REQUIRED);
                assert!(
                    reg.last_call().is_none(),
                    "registry must not be touched when the operator gate denies"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn member_role_may_invoke_an_ordinary_read_tool() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("member".to_string()), async {
                let reg =
                    Arc::new(StubRegistry::new().with_ok("memory_search", json!({"hits": []})));
                let params = json!({"tool_name": "memory_search", "arguments": {"query": "x"}});
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(resp.is_success(), "expected success: {:?}", resp.error);
                assert!(reg.last_call().is_some());
            })
            .await;
    }

    #[tokio::test]
    async fn operator_role_may_invoke_an_operator_tier_tool() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("operator".to_string()), async {
                let reg = Arc::new(StubRegistry::new().with_ok("cron_manage", json!({"ok": true})));
                let params = json!({"tool_name": "cron_manage", "arguments": {"action": "list"}});
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(
                    resp.is_success(),
                    "operator must be allowed: {:?}",
                    resp.error
                );
                assert!(reg.last_call().is_some());
            })
            .await;
    }

    /// Absent role (no `CALLER_ROLE` scoped — cron/internal/local no-auth
    /// daemon callers) is trusted, exactly like every other operator gate in
    /// this codebase (`role_is_operator(None) == true`). Byte-identical to
    /// pre-Task-9 behavior for every test above this point in the file that
    /// exercises `OPERATOR_TOOLS` members like `vault_store` with no scoped
    /// role.
    #[tokio::test]
    async fn absent_role_is_treated_as_operator_for_the_gate() {
        let reg = Arc::new(StubRegistry::new().with_ok("cron_manage", json!({"ok": true})));
        let params = json!({"tool_name": "cron_manage", "arguments": {"action": "list"}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg.clone(), None).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
    }

    #[tokio::test]
    async fn rejects_unknown_agent_id() {
        let tool_reg = Arc::new(StubRegistry::new());
        let agents = registry_with_restricted_agent();

        let params = json!({
            "tool_name": "allowed_one",
            "agent_id": "no_such_agent",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg, Some(agents)).await;
        assert!(!resp.is_success());
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }
}
