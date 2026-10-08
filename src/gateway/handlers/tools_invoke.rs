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
//!
//! ## Terminal observation capabilities
//! The legacy `terminal{action}` shape is rewritten to its canonical
//! `terminal_sessions_<action>` identity at the ingress
//! (`normalize_terminal_compat_call`), before every floor below. The six
//! canonical names are registered on the canonical `ToolHandlerRegistry`
//! only (they are not executor-registry tools), so they dispatch through a
//! per-request `ScopedToolService` over it — see `invoke_canonical`. Every
//! other tool keeps the raw `ToolRegistry` path.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, INTERNAL_ERROR, INVALID_PARAMS,
};
use super::parse_params;
use crate::agents::{AgentDef, AgentRegistry};
use crate::builtin_tools::terminal::capabilities::{
    is_observation_capability, normalize_terminal_compat_call,
};
use crate::executor::ToolRegistry;
use crate::tools::ToolHandlerRegistry;

/// Tool names explicitly permitted for non-operator (member) callers on this
/// surface (P1 member hardening, Task 9 review fix round 1).
///
/// (a) Why it exists: `tools.invoke` was blanket-gated to operators by
/// `method_admin.rs` until this task narrowly carved it open, because the
/// Panel's `create_from_template` (`interfaces/webchat/src/api/teams.rs`)
/// calls `tools.invoke{tool_name: "team_from_template"}` to materialize a
/// team from a template — a member-facing feature with no other RPC path.
///
/// (b) Why every addition is load-bearing: `tools.invoke` does NOT route
/// through `ScopedToolService` for ordinary tools (only the canonical
/// terminal observation capabilities do — see the module doc above) — so
/// nothing on this surface gets exec-tier approval, `tool_permissions`, hooks, or the
/// operation ledger. A name added here executes IMMEDIATELY for every
/// member with none of those gates, regardless of what the tool actually
/// does. This is why `OPERATOR_TOOLS` (a narrow curated self-config/cluster
/// list — see the third hard floor below) is NOT used as this allowlist:
/// most of this surface's tools, including the entire desktop-action family
/// and `channel_message`, are not in `OPERATOR_TOOLS` either, and letting
/// "not operator-tier" stand in for "safe for a member to invoke ungated"
/// was the actual hole this round closes.
///
/// (c) Widen-on-demand rule: add a name here ONLY with a named production
/// consumer (like `create_from_template` above) AND a test. Do not add a
/// tool "because it seems harmless" — prove it with a consumer and pin it
/// with a regression test the way `member_role_may_invoke_the_allowed_tool`
/// and `member_role_is_denied_a_tool_not_on_the_allowlist` do below.
const MEMBER_ALLOWED_TOOLS: &[&str] = &["team_from_template"];

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
    handle_invoke_with_canonical(request, registry, agents, None).await
}

/// [`handle_invoke`] plus the canonical `ToolHandlerRegistry` the terminal
/// observation capabilities are registered on. With `Some`, the canonical
/// terminal names dispatch through [`invoke_canonical`]; with `None` (test
/// mode / legacy callers) terminal observations fail closed without falling
/// back to the raw `registry`. Non-terminal tools retain their existing path.
pub async fn handle_invoke_with_canonical<R>(
    request: JsonRpcRequest,
    registry: Arc<R>,
    agents: Option<Arc<AgentRegistry>>,
    canonical: Option<Arc<ToolHandlerRegistry>>,
) -> JsonRpcResponse
where
    R: ToolRegistry + ?Sized,
{
    let mut params: InvokeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Compat ingress: legacy `terminal{action,..}` becomes its canonical
    // `terminal_sessions_<action>` call BEFORE any floor, so every floor, the
    // agent allowlist and the dispatch all see the one real identity. An
    // action that does not normalize is refused here and never reaches a
    // registry under any name.
    match normalize_terminal_compat_call(&params.tool_name, std::mem::take(&mut params.arguments)) {
        Ok((name, arguments)) => {
            params.tool_name = name;
            params.arguments = arguments;
        }
        Err(err) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, err.to_string()),
    }

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
    // `ALEPH_GATEWAY_TOOLS_ALLOW` env var. (The operator-tier gate and the
    // member allowlist are the two exceptions — see the third and fourth
    // hard floors below, P1 Task 9.)
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

    // `caller_role` is already ambient here with no new stamping needed:
    // `scope::with_scope`/`CALLER_ROLE`/`CALLER_USER` are scoped around
    // EVERY dispatched request (`server::connection::dispatch::dispatch_with_caller_context`,
    // P0/Task 3), and `tools.invoke` dispatches through `process_request`
    // like any other RPC. Computed once and shared by the two floors below.
    let caller_role = crate::gateway::caller_identity::current_caller_role();
    let caller_is_operator = crate::tools::turn_context::role_is_operator(caller_role.as_deref());

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
    // `turn_context::role_is_operator`. Absent role (no gateway connection
    // — cron/internal/local no-auth daemon) is trusted, exactly like every
    // other operator gate in this codebase.
    if crate::gateway::method_authz::tool_requires_operator(&params.tool_name)
        && !caller_is_operator
    {
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

    // Fourth hard floor (review fix, P1 Task 9 round 1): a member allowlist.
    // `OPERATOR_TOOLS` above is a NARROW curated self-config/cluster list —
    // it is NOT a general destructive-tool gate, and most of this surface's
    // tools (including the entire desktop-action family — `gui_click`,
    // `type_text`, `key_button`, `desktop_action`, … — and `channel_message`)
    // are in none of `OPERATOR_TOOLS` / `dangerous_tools`'s deny list /
    // `requires_confirmation`. Because this handler dispatches straight off
    // the raw `ToolRegistry`, NOTHING on this surface gets exec-tier
    // approval, `tool_permissions`, hooks, or the operation ledger — so a
    // member reaching any of those tools here would act immediately with
    // NONE of the gates the `chat.send`/`agent.run` path enforces for that
    // same caller, and members were fully walled off `tools.` before this
    // task. "Carve open everything except OPERATOR_TOOLS" was never the
    // intent — the plan's own text says the carve-out exists because "Panel
    // `team_from_template` needs only invoke; widen later on demand." So a
    // non-operator caller may invoke ONLY a tool named in
    // `MEMBER_ALLOWED_TOOLS`, independent of whether it also passed the
    // `OPERATOR_TOOLS` check above (defense in depth — both floors stay).
    // WIDEN-ON-DEMAND RULE: add a name here only with a named production
    // consumer AND a test proving the specific escalation it does or does
    // not enable — each addition is a load-bearing security decision, not a
    // convenience.
    if !caller_is_operator && !MEMBER_ALLOWED_TOOLS.contains(&params.tool_name.as_str()) {
        return JsonRpcResponse::error(
            request.id,
            AUTH_REQUIRED,
            format!(
                "tool '{}' is not on the member allowlist for the gateway \
                 tools.invoke surface (this surface bypasses exec-tier \
                 approval, tool_permissions, and hooks — only explicitly \
                 reviewed tools are open to non-operator callers). \
                 Do not retry.",
                params.tool_name
            ),
        );
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
        if !is_tool_allowed_with_legacy_terminal_alias(&agent_def, &params.tool_name) {
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

    let outcome = match canonical {
        Some(canonical) if is_observation_capability(&params.tool_name) => {
            invoke_canonical(
                canonical,
                &request.id,
                params.agent_id.as_deref(),
                &params.tool_name,
                arguments,
            )
            .await
        }
        None if is_observation_capability(&params.tool_name) => {
            Err("canonical terminal registry is not bound".to_string())
        }
        _ => registry
            .execute_tool(&params.tool_name, arguments)
            .await
            .map_err(|err| err.to_string()),
    };

    match outcome {
        Ok(mut result) => {
            mask_presentation_in_place(&mut result);
            JsonRpcResponse::success(
                request.id,
                json!({
                    "ok": true,
                    "tool_name": params.tool_name,
                    "result": result,
                }),
            )
        }
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("tool '{}' failed: {}", params.tool_name, err),
        ),
    }
}

/// Dispatch a canonical capability through a per-request `ScopedToolService`
/// over the canonical registry, so the call reaches the handler the only way
/// canonical handlers can be reached: with a dispatch verdict.
///
/// The per-request `TurnContext` is ALWAYS present and carries the request's
/// own `CALLER_ROLE` — an absent turn context would read as operator at the
/// gate (`current_turn_context().is_none_or(..)`), which is exactly the
/// widening this must not cause for a member/guest caller. (The floors above
/// already refuse non-operators; this keeps the inner gate honest too.) The
/// call identity `rpc:tools.invoke:<request id>` is the existing
/// `with_call_identity` carrier, not a new task-local.
async fn invoke_canonical(
    canonical: Arc<ToolHandlerRegistry>,
    request_id: &Option<Value>,
    agent_id: Option<&str>,
    tool_name: &str,
    arguments: Value,
) -> Result<Value, String> {
    use crate::approval::tool_call::{with_call_identity, CallIdentity};
    use crate::tools::runtime::LoopToolRegistry;
    use crate::tools::turn_context::TurnContext;
    use crate::tools::ToolService;

    let request_label = match request_id {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "none".to_string(),
    };
    let call_id = format!("rpc:tools.invoke:{request_label}");

    let mut inner = LoopToolRegistry::new();
    let entry = canonical
        .resolve_entry(tool_name)
        .ok_or_else(|| format!("canonical tool not found: {tool_name}"))?;
    inner.register(Box::new(
        crate::tools::adapters::McpRegistryTool::from_registry_entry(
            entry.handler,
            &entry.descriptor,
        ),
    ));
    inner.bind_canonical_registry(canonical, Arc::new(|_| true));
    let ctx = TurnContext {
        session_key: crate::routing::session_key::SessionKey::Ephemeral {
            agent_id: agent_id.unwrap_or("main").to_string(),
            ephemeral_id: call_id.clone(),
        },
        run_id: call_id.clone(),
        channel_id: String::new(),
        conversation_id: String::new(),
        caller_role: crate::gateway::caller_identity::current_caller_role(),
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    };
    let service = crate::tools::ScopedToolService::new(Arc::new(inner), Default::default())
        .with_turn_context(ctx)
        .with_structured_rpc_transport();
    let identity = CallIdentity {
        turn_id: crate::session::events::TurnId::nil(),
        call_id,
    };
    with_call_identity(Some(identity), service.execute(tool_name, arguments))
        .await
        .map(|output| output.value)
        .map_err(|err| err.to_string())
}

/// Mask the `_presentation` side-channel in a tool result this surface is
/// about to return **verbatim**.
///
/// The third face of the presentation. The other two — the live `tool_end`
/// frame (`event_emitter::RedactingEmitter`) and the replayed
/// `tool_call_completed` (`handlers::trace_replay`) — reach a human through
/// `exec::masker::mask_presentation`, and the contract on that function is
/// that every face shares ONE derivation. This handler dispatches straight
/// off the raw `ToolRegistry` (see the module doc), so it never passes
/// `ScopedToolService::apply_layer_two`: the key is not hoisted out and,
/// until this call existed, nothing masked it. What arrives at the caller is
/// up to `MAX_HUNK_LINES` lines of file content — including a deleted file's
/// pre-image — in the clear.
///
/// **Not an escalation** (`file_write`'s caller supplied the content;
/// `file_edit` already returns a post-edit snippet; anyone who can invoke
/// `apply_patch` can invoke `file_read`), which is why it is masked here
/// rather than refused. The point is that a second, unmasked copy of file
/// content must not exist on a door the "one derivation, two faces" story
/// does not cover.
///
/// **The registry is deliberately not changed**: `presentation_census`
/// (`tools::presentation_census`) runs `reg.execute_tool` and asserts the key
/// IS present, so the producer's contract is "attach it"; hoisting/masking is
/// the consumer's job, and this is a consumer.
///
/// A value under the key that does not parse as a `Presentation` (a foreign
/// shape from some future tool) gets `mask_json_strings` instead — every
/// string leaf, no shape knowledge needed. That is weaker than the typed walk
/// (it cannot run the multi-line join pass that degrades to
/// `Unavailable::Redacted`), and it is used ONLY where the typed walk has
/// nothing to walk.
fn mask_presentation_in_place(result: &mut Value) {
    let Some(obj) = result.as_object_mut() else {
        return;
    };
    let Some(raw) = obj.get(aleph_protocol::PRESENTATION_KEY).cloned() else {
        return;
    };
    let masker = crate::exec::masker::SecretMasker::new();
    match serde_json::from_value::<aleph_protocol::Presentation>(raw.clone()) {
        Ok(mut presentation) => {
            crate::exec::masker::mask_presentation(&masker, &mut presentation);
            match serde_json::to_value(presentation) {
                Ok(masked) => {
                    obj.insert(aleph_protocol::PRESENTATION_KEY.to_string(), masked);
                }
                // Unreachable in practice (the type is plain data). If it
                // ever happens, drop the key — shipping the unmasked
                // original because re-serialization failed is the one
                // outcome this function exists to prevent.
                Err(e) => {
                    tracing::warn!(error = %e, "tools.invoke: could not re-serialize a masked presentation; dropping it");
                    obj.remove(aleph_protocol::PRESENTATION_KEY);
                }
            }
        }
        Err(_) => {
            let mut foreign = raw;
            crate::exec::masker::mask_json_strings(&masker, &mut foreign);
            obj.insert(aleph_protocol::PRESENTATION_KEY.to_string(), foreign);
        }
    }
}

/// The agent allowlist check, with the retired `terminal` tool name still
/// honored in user-authored policy for the five observation actions it used to
/// carry.
///
/// Scope: a compatibility rename, not a permission-model change. The ingress
/// rewrites `terminal{action}` to `terminal_sessions_<action>` before this
/// gate, so an `AgentDef` that says `denied_tools: [terminal]` /
/// `allowed_tools: [terminal]` would otherwise stop meaning anything. For
/// those five canonical names only, a request-local clone of the policy maps
/// the literal `terminal` entries to the name being checked, then the one
/// existing `AgentDef::is_tool_allowed` (deny-first, named sets, flat list)
/// decides — no second algorithm, nothing persisted, registry untouched.
/// `terminal_sessions_attach` had no legacy alias and is checked verbatim, as
/// is every other tool. No named tool set contains `terminal`, so sets need no
/// aliasing.
fn is_tool_allowed_with_legacy_terminal_alias(agent_def: &AgentDef, tool_name: &str) -> bool {
    const LEGACY_NAME: &str = "terminal";
    const LEGACY_ACTIONS: [&str; 5] = ["list", "read", "status", "wait", "explain"];
    let aliased = tool_name
        .strip_prefix("terminal_sessions_")
        .is_some_and(|action| LEGACY_ACTIONS.contains(&action));
    if !aliased {
        return agent_def.is_tool_allowed(tool_name);
    }
    let mut local = agent_def.clone();
    for entry in local
        .allowed_tools
        .iter_mut()
        .chain(local.denied_tools.iter_mut())
        .filter(|entry| entry.as_str() == LEGACY_NAME)
    {
        *entry = tool_name.to_owned();
    }
    local.is_tool_allowed(tool_name)
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
    // P1 member hardening (Task 9): the operator-tier tool gate + the member
    // allowlist (review fix round 1).
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
    // `server::connection::dispatch::dispatch_with_caller_context` — P0/Task 3, nothing
    // new to stamp), so `tools.invoke` can be carved open in
    // `method_admin::MEMBER_CARVE_OUTS` without reopening the escalation.
    //
    // Round-1 review found `OPERATOR_TOOLS` alone was not enough: it is a
    // narrow curated self-config/cluster list, not a general destructive-
    // tool gate, so `channel_message` and the desktop-action family
    // (`gui_click`, `type_text`, …) would have executed immediately for any
    // member with zero exec-tier approval / `tool_permissions` / hooks — a
    // NEW member-reachable escalation members never had before this task
    // (they were previously walled off `tools.` entirely). The round-1 fix
    // adds `MEMBER_ALLOWED_TOOLS`, an explicit allowlist checked independent
    // of the `OPERATOR_TOOLS` floor — see the tests below including the
    // deliberate tightening of `memory_search` from allowed to denied.
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

    /// Round-1 tightening: `memory_search` used to be allowed for a member
    /// (round 0, before `MEMBER_ALLOWED_TOOLS` existed — any tool not in
    /// `OPERATOR_TOOLS` passed). It is NOT on the member allowlist, so it is
    /// now denied. This is a deliberate narrowing, not a regression: the
    /// round-0 gate's "everything except OPERATOR_TOOLS" shape was the hole
    /// review found (`channel_message`/desktop-action tools would have
    /// passed the same way `memory_search` did).
    #[tokio::test]
    async fn member_role_is_denied_a_tool_not_on_the_allowlist() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("member".to_string()), async {
                let reg =
                    Arc::new(StubRegistry::new().with_ok("memory_search", json!({"hits": []})));
                let params = json!({"tool_name": "memory_search", "arguments": {"query": "x"}});
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(
                    !resp.is_success(),
                    "memory_search is not on MEMBER_ALLOWED_TOOLS — must be denied"
                );
                assert_eq!(resp.error.unwrap().code, AUTH_REQUIRED);
                assert!(
                    reg.last_call().is_none(),
                    "registry must not be touched when the allowlist floor denies"
                );
            })
            .await;
    }

    /// The stated reason `tools.invoke` was carved open at all: the Panel's
    /// `create_from_template` calls this for a member. Must still work.
    #[tokio::test]
    async fn member_role_may_invoke_the_allowed_tool() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("member".to_string()), async {
                let reg = Arc::new(
                    StubRegistry::new().with_ok("team_from_template", json!({"team_id": "t1"})),
                );
                let params =
                    json!({"tool_name": "team_from_template", "arguments": {"template": "x"}});
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(resp.is_success(), "expected success: {:?}", resp.error);
                assert!(reg.last_call().is_some());
            })
            .await;
    }

    /// `channel_message` is in none of `OPERATOR_TOOLS` / `dangerous_tools`'s
    /// deny list / `requires_confirmation` — the exact class of tool the
    /// round-0 gate would have let a member fire immediately, with no
    /// exec-tier approval, no `tool_permissions`, no hooks. Must be denied
    /// by the member allowlist floor.
    #[tokio::test]
    async fn member_role_is_denied_channel_message() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("member".to_string()), async {
                let reg =
                    Arc::new(StubRegistry::new().with_ok("channel_message", json!({"ok": true})));
                let params = json!({
                    "tool_name": "channel_message",
                    "arguments": {"conversation_id": "c1", "text": "hi"}
                });
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(!resp.is_success(), "channel_message must be denied");
                assert_eq!(resp.error.unwrap().code, AUTH_REQUIRED);
                assert!(reg.last_call().is_none());
            })
            .await;
    }

    /// Same shape as `channel_message`: a desktop-action tool is not in
    /// `OPERATOR_TOOLS` either, so the round-0 gate alone would have let a
    /// member drive the desktop directly through this surface. Denied by
    /// the member allowlist floor.
    #[tokio::test]
    async fn member_role_is_denied_a_desktop_action_tool() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("member".to_string()), async {
                let reg = Arc::new(StubRegistry::new().with_ok("gui_click", json!({"ok": true})));
                let params = json!({"tool_name": "gui_click", "arguments": {"x": 1, "y": 1}});
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(!resp.is_success(), "gui_click must be denied");
                assert_eq!(resp.error.unwrap().code, AUTH_REQUIRED);
                assert!(reg.last_call().is_none());
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

    /// A4 RED: legacy `tools.invoke{tool_name:"terminal", arguments:{action}}`
    /// keeps working for operators by being normalized to the canonical name
    /// (action key stripped) BEFORE the registry lookup — no unknown-tool, and
    /// no call under the retired legacy name.
    #[tokio::test]
    async fn terminal_capability_operator_legacy_terminal_invoke_reaches_canonical_name() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("operator".to_string()), async {
                struct CapturingRead;
                #[async_trait::async_trait]
                impl crate::tools::handlers::ToolHandler for CapturingRead {
                    async fn invoke(&self, input: Value) -> Result<crate::session::events::ToolOutput, crate::tools::ToolError> {
                        assert_eq!(input, json!({"session_id": "x"}));
                        let context = crate::tools::turn_context::current_turn_context()
                            .expect("RPC must install a turn context");
                        assert_eq!(context.caller_role.as_deref(), Some("operator"));
                        assert_eq!(crate::approval::current_tool_call_id().as_deref(), Some("rpc:tools.invoke:1"));
                        Ok(crate::session::events::ToolOutput {
                            value: json!({"text": "hi"}),
                            metadata: Default::default(),
                        })
                    }
                    fn definition(&self) -> crate::tools::service::ToolDefinition {
                        crate::tools::service::ToolDefinition {
                            name: "terminal_sessions_read".into(),
                            description: "Test observation".into(),
                            input_schema: json!({"type":"object", "properties":{"session_id":{"type":"string"}}, "required":["session_id"]}),
                            source: crate::tools::service::ToolSource::Builtin,
                            metadata: crate::tools::service::ToolDefinitionMetadata {
                                idempotent: true,
                                ..Default::default()
                            },
                        }
                    }
                }
                let canonical = Arc::new(ToolHandlerRegistry::new());
                let handler: Arc<dyn crate::tools::handlers::ToolHandler> = Arc::new(CapturingRead);
                let descriptor = crate::tools::ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
                let _handle = canonical.register(descriptor, handler).unwrap();
                let reg = Arc::new(StubRegistry::new());
                let params = json!({
                    "tool_name": "terminal",
                    "arguments": {"action": "read", "session_id": "x"}
                });
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke_with_canonical(req, reg.clone(), None, Some(canonical)).await;
                assert!(resp.is_success(), "expected success: {:?}", resp.error);
                let result = resp.result.unwrap();
                assert_eq!(result["tool_name"], "terminal_sessions_read");
                assert_eq!(result["result"]["text"], "hi", "full result: {result}");
                assert!(reg.last_call().is_none(), "canonical calls must never hit raw executor");
            })
            .await;
    }

    /// Transport-contract regression: a large (>100KB) structured
    /// `TerminalOutput` envelope returned by a canonical terminal read must
    /// reach the RPC caller as the structured object with the screen text
    /// intact — not as a Layer-2-truncated string (default token budget, no
    /// result store).
    #[tokio::test]
    async fn terminal_capability_rpc_preserves_large_structured_envelope() {
        // Varied (non-repetitive) text so size is real and not compressible away.
        let mut screen = String::new();
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut line_no = 0u32;
        while screen.len() < 120 * 1024 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            screen.push_str(&format!(
                "{line_no:05} {state:016x} row-{} col-{}\n",
                state % 9973,
                (state >> 17) % 7919
            ));
            line_no += 1;
        }
        assert!(screen.len() > 100 * 1024);

        struct CannedRead {
            output: Value,
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl crate::tools::handlers::ToolHandler for CannedRead {
            async fn invoke(
                &self,
                _input: Value,
            ) -> Result<crate::session::events::ToolOutput, crate::tools::ToolError> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::session::events::ToolOutput {
                    value: self.output.clone(),
                    metadata: Default::default(),
                })
            }
            fn definition(&self) -> crate::tools::service::ToolDefinition {
                crate::tools::service::ToolDefinition {
                    name: "terminal_sessions_read".into(),
                    description: "Test observation".into(),
                    input_schema: json!({"type":"object", "properties":{"session_id":{"type":"string"}}, "required":["session_id"]}),
                    source: crate::tools::service::ToolSource::Builtin,
                    metadata: crate::tools::service::ToolDefinitionMetadata {
                        idempotent: true,
                        ..Default::default()
                    },
                }
            }
        }

        let envelope = crate::builtin_tools::terminal::TerminalOutput {
            success: true,
            message: "read ok".to_string(),
            data: Some(json!({"screen": screen.clone()})),
            lost_with_restart: false,
        };
        let canned = serde_json::to_value(&envelope).unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("operator".to_string()), async {
                let canonical = Arc::new(ToolHandlerRegistry::new());
                let handler: Arc<dyn crate::tools::handlers::ToolHandler> = Arc::new(CannedRead {
                    output: canned,
                    calls: calls.clone(),
                });
                let descriptor = crate::tools::ToolCapabilityDescriptor::from_definition(
                    &handler.definition(),
                    0,
                );
                let _handle = canonical.register(descriptor, handler).unwrap();
                let reg = Arc::new(StubRegistry::new());
                let params = json!({
                    "tool_name": "terminal",
                    "arguments": {"action": "read", "session_id": "x"}
                });
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp =
                    handle_invoke_with_canonical(req, reg.clone(), None, Some(canonical)).await;
                assert!(resp.is_success(), "expected success: {:?}", resp.error);
                let result = resp.result.unwrap();
                assert_eq!(result["tool_name"], "terminal_sessions_read");
                assert!(
                    result["result"].is_object(),
                    "large structured envelope must stay an object, got {}",
                    if result["result"].is_string() {
                        "truncated String"
                    } else {
                        "non-object"
                    }
                );
                assert_eq!(result["result"]["success"], true);
                assert_eq!(result["result"]["message"], "read ok");
                assert_eq!(
                    result["result"]["data"]["screen"].as_str(),
                    Some(screen.as_str()),
                    "screen text must be retained exactly"
                );
                assert_eq!(
                    calls.load(std::sync::atomic::Ordering::SeqCst),
                    1,
                    "the admitted handler must run exactly once"
                );
                assert!(
                    reg.last_call().is_none(),
                    "canonical calls must never hit raw executor"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn terminal_observation_rpc_never_falls_back_without_canonical_binding() {
        let reg = Arc::new(StubRegistry::new().with_ok("terminal_sessions_list", json!([])));
        let req = JsonRpcRequest::with_id(
            "tools.invoke",
            Some(json!({
                "tool_name": "terminal", "arguments": {"action": "list"}
            })),
            json!(2),
        );
        let resp = handle_invoke(req, reg.clone(), None).await;
        assert!(!resp.is_success());
        assert!(resp
            .error
            .unwrap()
            .message
            .contains("canonical terminal registry is not bound"));
        assert!(reg.last_call().is_none());
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn terminal_observation_rpc_real_handlers_and_member_floor() {
        let canonical = Arc::new(ToolHandlerRegistry::new());
        let mut scope = crate::tools::ToolRegistrationScope::new("rpc-terminal-test");
        crate::builtin_tools::terminal::capabilities::register_observation_capabilities(
            &canonical, &mut scope,
        )
        .unwrap();
        for role in ["operator", "member", "guest"] {
            crate::gateway::caller_identity::CALLER_ROLE
                .scope(Some(role.into()), async {
                    for name in ["terminal", "terminal_sessions_list"] {
                        let reg = Arc::new(StubRegistry::new());
                        let args = if name == "terminal" {
                            json!({"action":"list"})
                        } else {
                            json!({})
                        };
                        let req = JsonRpcRequest::with_id(
                            "tools.invoke",
                            Some(json!({"tool_name":name, "arguments":args})),
                            json!(3),
                        );
                        let resp = handle_invoke_with_canonical(
                            req,
                            reg.clone(),
                            None,
                            Some(canonical.clone()),
                        )
                        .await;
                        if role == "operator" {
                            assert!(resp.is_success(), "{name}: {:?}", resp.error);
                            assert_eq!(resp.result.unwrap()["result"]["success"], true);
                        } else {
                            assert_eq!(resp.error.unwrap().code, AUTH_REQUIRED);
                        }
                        assert!(reg.last_call().is_none());
                    }
                })
                .await;
        }
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("guest".into()), async {
                let result = invoke_canonical(
                    canonical,
                    &Some(json!(4)),
                    None,
                    "terminal_sessions_list",
                    json!({}),
                )
                .await;
                assert!(
                    result.is_err(),
                    "inner scoped gate must preserve the request's guest role"
                );
            })
            .await;
    }

    /// Canonical registry whose six terminal observation names are counting
    /// stubs, so a permission test can tell "admitted and ran" from "refused".
    fn counting_terminal_registry() -> (
        Arc<ToolHandlerRegistry>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        struct Counting {
            name: &'static str,
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl crate::tools::handlers::ToolHandler for Counting {
            async fn invoke(
                &self,
                _input: Value,
            ) -> Result<crate::session::events::ToolOutput, crate::tools::ToolError> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::session::events::ToolOutput {
                    value: json!({"success": true}),
                    metadata: Default::default(),
                })
            }
            fn definition(&self) -> crate::tools::service::ToolDefinition {
                crate::tools::service::ToolDefinition {
                    name: self.name.into(),
                    description: "Test observation".into(),
                    input_schema: json!({"type":"object"}),
                    source: crate::tools::service::ToolSource::Builtin,
                    metadata: crate::tools::service::ToolDefinitionMetadata {
                        idempotent: true,
                        ..Default::default()
                    },
                }
            }
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let canonical = Arc::new(ToolHandlerRegistry::new());
        for name in [
            "terminal_sessions_list",
            "terminal_sessions_read",
            "terminal_sessions_status",
            "terminal_sessions_wait",
            "terminal_sessions_explain",
            "terminal_sessions_attach",
        ] {
            let handler: Arc<dyn crate::tools::handlers::ToolHandler> = Arc::new(Counting {
                name,
                calls: calls.clone(),
            });
            let descriptor =
                crate::tools::ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
            // Handle dropped on purpose: registration must outlive this fn.
            std::mem::forget(canonical.register(descriptor, handler).unwrap());
        }
        (canonical, calls)
    }

    /// Run one `tools.invoke` as an operator against an agent with the given
    /// policy; returns (admitted, handler invocation count).
    async fn invoke_terminal_as_agent(
        agent: AgentDef,
        tool_name: &str,
        arguments: Value,
    ) -> (bool, usize) {
        let (canonical, calls) = counting_terminal_registry();
        let agents = AgentRegistry::new();
        let agent_id = agent.id.clone();
        agents.register(agent);
        let admitted = crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("operator".to_string()), async {
                let req = JsonRpcRequest::with_id(
                    "tools.invoke",
                    Some(json!({
                        "tool_name": tool_name,
                        "agent_id": agent_id,
                        "arguments": arguments,
                    })),
                    json!(9),
                );
                let resp = handle_invoke_with_canonical(
                    req,
                    Arc::new(StubRegistry::new()),
                    Some(Arc::new(agents)),
                    Some(canonical),
                )
                .await;
                resp.is_success()
            })
            .await;
        (admitted, calls.load(std::sync::atomic::Ordering::SeqCst))
    }

    const LEGACY_FIVE: [&str; 5] = ["list", "read", "status", "wait", "explain"];

    /// A4 compat: a user-authored `denied_tools:[terminal]` still denies the
    /// five legacy observation actions, whether called under the legacy name
    /// or the canonical one; the handler never runs.
    #[tokio::test]
    async fn terminal_capability_legacy_terminal_deny_blocks_invocation() {
        for action in LEGACY_FIVE {
            let denied = || {
                AgentDef::new("a", AgentMode::Primary).with_denied_tools(vec!["terminal".into()])
            };
            let (ok, ran) = invoke_terminal_as_agent(
                denied(),
                "terminal",
                json!({"action": action, "session_id": "x"}),
            )
            .await;
            assert!(!ok && ran == 0, "legacy call `{action}` must be denied");
            let (ok, ran) = invoke_terminal_as_agent(
                denied(),
                &format!("terminal_sessions_{action}"),
                json!({"session_id": "x"}),
            )
            .await;
            assert!(
                !ok && ran == 0,
                "canonical `{action}` must honor legacy deny"
            );
        }
    }

    /// `attach` never had a legacy alias: `denied_tools:[terminal]` must not
    /// deny it, and `allowed_tools:[terminal]` must not admit it.
    #[tokio::test]
    async fn terminal_capability_legacy_terminal_entry_does_not_alias_attach() {
        let (ok, ran) = invoke_terminal_as_agent(
            AgentDef::new("a", AgentMode::Primary).with_denied_tools(vec!["terminal".into()]),
            "terminal_sessions_attach",
            json!({"session_id": "x"}),
        )
        .await;
        assert!(
            ok && ran == 1,
            "deny of legacy `terminal` must not reach attach"
        );
        let (ok, ran) = invoke_terminal_as_agent(
            AgentDef::new("a", AgentMode::Primary).with_allowed_tools(vec!["terminal".into()]),
            "terminal_sessions_attach",
            json!({"session_id": "x"}),
        )
        .await;
        assert!(
            !ok && ran == 0,
            "allow of legacy `terminal` must not admit attach"
        );
    }

    /// A4 compat: `allowed_tools:[terminal]` still admits the five legacy
    /// actions (legacy or canonical name), and nothing else.
    #[tokio::test]
    async fn terminal_capability_legacy_terminal_allow_admits_invocation() {
        for action in LEGACY_FIVE {
            let allowed = || {
                AgentDef::new("a", AgentMode::Primary).with_allowed_tools(vec!["terminal".into()])
            };
            let (ok, ran) = invoke_terminal_as_agent(
                allowed(),
                "terminal",
                json!({"action": action, "session_id": "x"}),
            )
            .await;
            assert!(ok && ran == 1, "legacy call `{action}` must be admitted");
            let (ok, ran) = invoke_terminal_as_agent(
                allowed(),
                &format!("terminal_sessions_{action}"),
                json!({"session_id": "x"}),
            )
            .await;
            assert!(
                ok && ran == 1,
                "canonical `{action}` must honor legacy allow"
            );
        }
        let (ok, ran) = invoke_terminal_as_agent(
            AgentDef::new("a", AgentMode::Primary).with_allowed_tools(vec!["terminal".into()]),
            "terminal_sessions_attach",
            json!({"session_id": "x"}),
        )
        .await;
        assert!(!ok && ran == 0);
    }

    /// Deny-first stays: an explicit canonical deny beats a legacy allow, and
    /// a canonical allow does not rescue a legacy deny.
    #[tokio::test]
    async fn terminal_capability_canonical_deny_beats_legacy_allow() {
        for action in LEGACY_FIVE {
            let canonical = format!("terminal_sessions_{action}");
            let (ok, ran) = invoke_terminal_as_agent(
                AgentDef::new("a", AgentMode::Primary)
                    .with_allowed_tools(vec!["terminal".into()])
                    .with_denied_tools(vec![canonical.clone()]),
                "terminal",
                json!({"action": action, "session_id": "x"}),
            )
            .await;
            assert!(
                !ok && ran == 0,
                "canonical deny must beat legacy allow ({action})"
            );
            let (ok, ran) = invoke_terminal_as_agent(
                AgentDef::new("a", AgentMode::Primary)
                    .with_allowed_tools(vec![canonical.clone()])
                    .with_denied_tools(vec!["terminal".into()]),
                &canonical,
                json!({"session_id": "x"}),
            )
            .await;
            assert!(
                !ok && ran == 0,
                "legacy deny must beat canonical allow ({action})"
            );
        }
    }

    /// A4 RED: a legacy `terminal` call whose action does not normalize is
    /// refused at the ingress; the registry is never touched.
    #[tokio::test]
    async fn terminal_capability_legacy_terminal_invoke_with_unknown_action_is_refused() {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("operator".to_string()), async {
                let reg = Arc::new(StubRegistry::new().with_ok("terminal", json!({"ok": true})));
                let params = json!({
                    "tool_name": "terminal",
                    "arguments": {"action": "spawn"}
                });
                let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
                let resp = handle_invoke(req, reg.clone(), None).await;
                assert!(!resp.is_success(), "unknown legacy action must be refused");
                assert!(
                    reg.last_call().is_none(),
                    "registry must not be touched (and never under the legacy name)"
                );
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

    /// The third face. This surface returns the registry's value verbatim —
    /// no `apply_layer_two`, so `_presentation` is still on the object — and
    /// the diff lines inside it carry file content. They go through the same
    /// `mask_presentation` the live frame and the replay leg use.
    ///
    /// The tool is deliberately NOT one of today's three producers: all of
    /// `file_write` / `file_edit` / `apply_patch` are in `DANGEROUS_TOOLS`,
    /// so on this surface they are refused by the first hard floor unless an
    /// operator sets `ALEPH_GATEWAY_TOOLS_ALLOW`. That makes today's door
    /// narrow (the escape hatch, or a future non-dangerous tool that attaches
    /// a presentation) — it does not make it closed, and the masking belongs
    /// to the surface rather than to the current membership of a denylist.
    #[tokio::test]
    async fn a_presentation_returned_verbatim_is_masked_like_the_other_two_faces() {
        const KEY: &str = "sk-abcdefghijklmnopqrstuvwxyz123456789012345678";
        let reg = Arc::new(StubRegistry::new().with_ok(
            "a_presentation_attaching_tool",
            json!({
                "success": true,
                "path": "/tmp/a.rs",
                "_presentation": {
                    "kind": "file_changes",
                    "changes": [{
                        "path": "/tmp/a.rs",
                        "kind": "modified",
                        "hunks": [{
                            "old_start": 1,
                            "new_start": 1,
                            "lines": [{"tag": "add", "text": format!("let k = \"{KEY}\";")}]
                        }],
                        "added": 1,
                        "removed": 0
                    }]
                }
            }),
        ));
        let params = json!({"tool_name": "a_presentation_attaching_tool", "arguments": {}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg, None).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);

        let body = serde_json::to_string(&resp.result.unwrap()).unwrap();
        assert!(
            !body.contains("abcdefghijklmnopqrstuvwxyz"),
            "the secret reached the caller in the clear: {body}"
        );
        assert!(
            body.contains("REDACTED"),
            "expected the masked form: {body}"
        );
    }

    /// A shape nothing can type-walk still gets every string leaf masked,
    /// rather than being returned untouched because it did not parse.
    #[tokio::test]
    async fn a_foreign_presentation_shape_is_still_masked_leaf_by_leaf() {
        const KEY: &str = "sk-abcdefghijklmnopqrstuvwxyz123456789012345678";
        let reg = Arc::new(StubRegistry::new().with_ok(
            "some_future_tool",
            json!({"_presentation": {"kind": "table", "rows": [format!("k={KEY}")]}}),
        ));
        let params = json!({"tool_name": "some_future_tool", "arguments": {}});
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, reg, None).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);

        let body = serde_json::to_string(&resp.result.unwrap()).unwrap();
        assert!(
            !body.contains("abcdefghijklmnopqrstuvwxyz"),
            "an unparseable presentation was returned unmasked: {body}"
        );
    }
}
