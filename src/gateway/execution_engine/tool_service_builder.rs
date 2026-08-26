//! `build_request_tool_service` — compose a per-request `ToolService` for the
//! orchestrator dispatch path.
//!
//! Wraps the Phase 4b `ScopedToolService` adapter with the Gateway's
//! concrete pieces:
//!   * `LoopToolRegistry` snapshot filtered by `allowed_tools` (builtins +
//!     plugins + the MCP registry snapshot `run_loop` joins per request)
//!   * optional `SubagentTool` (when the agent has it enabled)
//!
//! Returned as `Arc<dyn ToolService>` for `FlowRequest.tool_service`.

use crate::sync_primitives::Arc;
use std::collections::BTreeSet;

use crate::agents::subagent_tool::SubagentTool;
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::extension::hooks::HookExecutor;
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::tools::runtime::LoopToolRegistry;
use crate::tools::scoped::ScopedToolService;
use crate::tools::service::ToolService;

/// Process-wide confirmation requester, installed by boot once the channel
/// registry exists (see `start/mod.rs`). `build_request_tool_service` consults
/// it so confirm-flagged tools route a user confirmation before executing.
///
/// `FailsClosed`, and this is the one disposition in the batch worth stating
/// out loud because the reflex reading is the opposite one. "No approval
/// channel" sounds like a gate that stops gating; `scoped/dispatch.rs`
/// does the reverse — its `None` arm records a gate refusal and returns
/// `ToolError::Execution { .. "No approval channel is available, so it cannot
/// be authorized here. Do not retry." }`. So a boot that never reached this
/// installer leaves every confirm-flagged tool permanently auto-denied, at
/// every tier, with no card anywhere that could authorize it: a door with no
/// handle, which is a wall.
static CONFIRMATION_REQUESTER: CapabilitySlot<Arc<dyn ApprovalRequester>> = CapabilitySlot::new(
    "gateway/confirmation-requester",
    MissingSemantics::FailsClosed,
);

/// Install the process-wide tool-confirmation requester. Called once at boot.
pub fn set_confirmation_requester(requester: Arc<dyn ApprovalRequester>) {
    let _ = CONFIRMATION_REQUESTER.install(requester);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn confirmation_requester_slot() -> &'static dyn SlotStatus {
    &CONFIRMATION_REQUESTER
}

/// Process-wide config-tier approval requester (Phase 2b sudo), installed by
/// boot once the gateway event bus + exec approval manager exist.
///
/// `FailsClosed`: `check_operator_gate`'s `None` arm hard-denies every
/// operator-gated tool a chat-tier connection reaches for, with
/// "no approval channel is available … Do not retry". The installed behaviour
/// is the *softer* one — suspend and ask an operator — so losing this handle
/// removes the escalation path rather than the gate. Safe, and invisible: the
/// refusal text is identical whether boot never installed a requester or the
/// deployment genuinely has no operator surface.
static CONFIG_APPROVAL_REQUESTER: CapabilitySlot<Arc<dyn ApprovalRequester>> = CapabilitySlot::new(
    "gateway/config-approval-requester",
    MissingSemantics::FailsClosed,
);

/// Install the process-wide config-tier approval requester. Called once at boot.
pub fn set_config_approval_requester(requester: Arc<dyn ApprovalRequester>) {
    let _ = CONFIG_APPROVAL_REQUESTER.install(requester);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn config_approval_requester_slot() -> &'static dyn SlotStatus {
    &CONFIG_APPROVAL_REQUESTER
}

/// Process-wide MCP tool registry, installed by boot right after the MCP
/// tool bridge's target `ToolHandlerRegistry` is created (see
/// `start/mod.rs`). `run_loop` snapshots it per request so every connected
/// MCP server's tools join the agent's `LoopToolRegistry` — this is the
/// consumer side of the bridge; without it the registry is write-only and
/// external MCP tools never reach the LLM. Same install-once pattern as
/// `CONFIRMATION_REQUESTER` above.
///
/// `FailsClosed`: the single consumer (`run_loop/inner.rs`) skips the whole
/// join block, so no external MCP tool becomes LLM-visible. Nothing is granted
/// that should not be — and nothing is said. This is the §5.20 shape in full:
/// `mcp.list` can report every configured server healthy while zero of their
/// tools ever reach a request, because the bridge's write side and this,
/// its read side, are installed by two different boot statements.
static MCP_TOOL_REGISTRY: CapabilitySlot<Arc<crate::tools::ToolHandlerRegistry>> =
    CapabilitySlot::new("gateway/mcp-tool-registry", MissingSemantics::FailsClosed);

/// Install the process-wide MCP tool registry. Called once at boot.
pub fn set_mcp_tool_registry(registry: Arc<crate::tools::ToolHandlerRegistry>) {
    let _ = MCP_TOOL_REGISTRY.install(registry);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn mcp_tool_registry_slot() -> &'static dyn SlotStatus {
    &MCP_TOOL_REGISTRY
}

/// The MCP tool registry, if boot installed one (absent in unit tests and
/// simulated mode — callers treat `None` as "no external MCP tools").
pub(super) fn mcp_tool_registry() -> Option<&'static Arc<crate::tools::ToolHandlerRegistry>> {
    MCP_TOOL_REGISTRY.get()
}

/// Build the per-request `ToolService` for a chat turn.
///
/// * `tool_registry` — shared `LoopToolRegistry` (agent builtins + MCP).
/// * `allowed_tools` — tool names visible to this agent. Empty = allow-all.
/// * `subagent_tool` — optional subagent tool handle (adds the `subagent`
///   verb to the agent's toolbelt).
/// * `turn_context` — optional routing context of the agent turn; lets HITL
///   tools (sandbox escalation, `requires_confirmation`, `ask_user`) reach the
///   originating channel.
/// * `hook_executor` — optional extension `HookExecutor` snapshot. Wires
///   `BeforeToolCall` / `AfterToolCall` / `AfterToolCallFailure` extension
///   hooks around every tool dispatch. `None` (or an empty executor) means
///   no extension hooks are active for this request.
/// * `session_id` — session identifier surfaced into `HookContext` for
///   extension command hooks, and the scope key stamped onto this request's
///   `ToolResultStore` handle. Both call sites pass
///   `request.session_key.to_key_string()`, which is exactly what
///   `turn_context::current_session_key()` returns inside a dispatched tool —
///   `ctx_search` relies on that equality to find what this service offloaded.
/// * `tool_permissions` — merged EXPLICIT tool permission policy for this turn
///   (global `[policies.tool_permissions]` → agent override → channel
///   override, most restrictive wins). `None` = all-default policy; pass
///   `None` rather than a default config so the hot path stays a no-op.
/// * `exec_tier` — effective execution tier (global → session → channel clamp).
///   Decides every tool the explicit policy above does not name, from the
///   tool's declared metadata.
/// * `unattended` — true for an autonomous continuation run; makes the service
///   fail closed on confirm-gated tools (no human to approve).
/// * `core_tools` — tool names kept at full schema (progressive tool
///   disclosure). Empty or `["*"]` disables collapsing (escape hatch).
/// * `truncate_tool_descriptions` — also truncate non-core tool descriptions
///   when collapsing their schema.
/// * `deferred` — the shared deferred tier: tools hidden from the model's
///   initial list (populated from the MCP tool set when `defer_mcp_tools` is
///   on). The SAME `Arc` is held by `ToolSearchTool`, which promotes tools out
///   of it on discovery so they become callable — passing a fresh set here
///   would silently restore the old "discoverable but uncallable" trap.
///   Empty set (flag off) is a no-op — byte-identical surface.
/// * `tool_health` — the runtime probe cache shared with the `ToolCatalog`.
///   When present, a tool whose probe reports Unhealthy is stripped from the
///   model's list. `None` is fail-open: every tool stays visible.
pub fn build_request_tool_service(
    tool_registry: Arc<LoopToolRegistry>,
    allowed_tools: BTreeSet<String>,
    subagent_tool: Option<Arc<SubagentTool>>,
    turn_context: Option<crate::tools::turn_context::TurnContext>,
    hook_executor: Option<Arc<HookExecutor>>,
    session_id: impl Into<String>,
    tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
    exec_tier: crate::config::types::policies::ExecTier,
    unattended: bool,
    core_tools: &[String],
    truncate_tool_descriptions: bool,
    deferred: Arc<crate::tools::scoped::DeferredTools>,
    tool_health: Option<Arc<crate::tool_metadata::ToolHealthCache>>,
) -> Arc<dyn ToolService> {
    let session_id: String = session_id.into();
    let mut svc = ScopedToolService::new(tool_registry, allowed_tools);
    if let Some(st) = subagent_tool {
        svc = svc.with_subagent_tool(st);
    }
    if let Some(perms) = tool_permissions {
        svc = svc.with_tool_permissions(perms);
    }
    if let Some(health) = tool_health {
        svc = svc.with_health(health);
    }
    svc = svc.with_exec_tier(exec_tier);
    svc = svc.with_unattended(unattended);
    if let Some(tc) = turn_context {
        svc = svc.with_turn_context(tc);
    }
    if let Some(executor) = hook_executor {
        svc = svc.with_hook_executor(executor, session_id.clone());
    }
    // Layer 2 seam: oversized tool outputs are persisted to disk and the
    // LLM gets a marker line instead of the raw text. Inert until boot
    // installs the global store via `set_global_tool_result_store`.
    //
    // The installed store is process-wide; this request's handle is narrowed to
    // its own session, so the blobs land in a per-session directory and the
    // index rows carry a scope. Without it, `ctx_search` in one Panel tab could
    // retrieve another tab's tool output, and the denial circuit-breaker's
    // `purge_all` (3 refusals in ONE session) wiped every concurrent session's
    // artifacts. Scoping the handle keeps `persist_if_large`'s signature — and
    // therefore `harness/agent/act.rs` — untouched.
    if let Some(store) = crate::tools::result_store::global_tool_result_store() {
        svc = svc.with_result_store(crate::tools::result_store::ToolResultStore::for_session(
            &store, session_id,
        ));
    }
    // Wire the confirmation seam: confirm-gated tools route a user prompt
    // before executing. Inert until boot installs the requester. Gating is
    // declaration-driven — `LoopTool::requires_confirmation()` (builtins
    // through `RegistryToolAdapter`'s `CONFIRMATION_REQUIRED_TOOLS` list, MCP /
    // skill tools through their own adapters) — or policy-driven, via an `Ask`
    // permission in `tool_permissions`. No gateway-side name list exists.
    if let Some(requester) = CONFIRMATION_REQUESTER.get() {
        svc = svc.with_confirmation(Arc::clone(requester));
    }
    // Phase 2b: operator-targeted approval for config-tier tools invoked by a
    // chat-tier connection. Inert until boot installs the requester (then the
    // config gate suspends for operator approval instead of hard-rejecting).
    if let Some(requester) = CONFIG_APPROVAL_REQUESTER.get() {
        svc = svc.with_config_approval(Arc::clone(requester));
    }
    // Progressive tool disclosure: collapse non-core tool schemas. `None`
    // (core empty / ["*"]) leaves the service byte-identical to old behavior.
    if let Some(rewriter) = crate::tools::scoped::ProgressiveDisclosureRewriter::from_config(
        core_tools,
        truncate_tool_descriptions,
    ) {
        svc = svc.with_definition_rewriter(rewriter);
    }
    // Deferred exposure tier: drop these names from the LLM-visible lists.
    // Empty set (defer_mcp_tools off) is a no-op — byte-identical surface.
    svc = svc.with_deferred(deferred);
    Arc::new(svc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    ///
    /// All three handles in this file fail CLOSED, which is worth pinning
    /// rather than assuming: two of them are approval requesters, and the
    /// reflex reading of "the approval channel is missing" is that the gate
    /// stops gating. It does the opposite -- `scoped/dispatch.rs` refuses
    /// without asking anyone in every one of its three `None` arms.
    #[test]
    fn the_confirmation_requester_accessor_exposes_this_handle_to_the_roster() {
        let slot = confirmation_requester_slot();
        assert_eq!(slot.id(), "gateway/confirmation-requester");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_config_approval_accessor_exposes_this_handle_to_the_roster() {
        let slot = config_approval_requester_slot();
        assert_eq!(slot.id(), "gateway/config-approval-requester");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_mcp_registry_accessor_exposes_this_handle_to_the_roster() {
        let slot = mcp_tool_registry_slot();
        assert_eq!(slot.id(), "gateway/mcp-tool-registry");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    struct StubTool;

    #[async_trait]
    impl LoopTool for StubTool {
        fn name(&self) -> &str {
            "read_file"
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
            ToolResult::Success { output: json!({}) }
        }
    }

    #[tokio::test]
    async fn builder_returns_listable_service() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(StubTool));
        let registry = Arc::new(reg);

        let svc = build_request_tool_service(
            registry,
            BTreeSet::new(),
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );
        let defs = svc.list().await;
        assert!(defs.iter().any(|d| d.name == "read_file"));
    }

    struct OtherStub;
    #[async_trait]
    impl LoopTool for OtherStub {
        fn name(&self) -> &str {
            "web_fetch"
        }
        fn description(&self) -> &str {
            "stub2"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _i: Value, _c: CancellationToken) -> ToolResult {
            ToolResult::Success { output: json!({}) }
        }
    }

    #[tokio::test]
    async fn builder_defers_named_tools() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(StubTool)); // read_file
        reg.register(Box::new(OtherStub)); // web_fetch
        let deferred = crate::tools::scoped::DeferredTools::new(["web_fetch".to_string()].into());
        let svc = build_request_tool_service(
            Arc::new(reg),
            BTreeSet::new(),
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &[],
            false,
            deferred,
            None,
        );
        let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(
            !names.contains(&"web_fetch".to_string()),
            "deferred tool must be dropped"
        );
    }

    #[tokio::test]
    async fn builder_honours_allowed_filter() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(StubTool));
        let registry = Arc::new(reg);

        let allowed: BTreeSet<String> = ["other".to_string()].into_iter().collect();
        let svc = build_request_tool_service(
            registry,
            allowed,
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );
        let defs = svc.list().await;
        assert!(
            defs.iter().all(|d| d.name != "read_file"),
            "read_file is not in allowed set so must be filtered"
        );
    }

    #[tokio::test]
    async fn off_vs_on_deferral_surface() {
        let make = || {
            let mut reg = LoopToolRegistry::new();
            reg.register(Box::new(StubTool)); // read_file
            reg.register(Box::new(OtherStub)); // web_fetch (stands in for an MCP tool)
            Arc::new(reg)
        };
        // OFF: empty deferred set → both listed.
        let off = build_request_tool_service(
            make(),
            BTreeSet::new(),
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );
        let off_names: Vec<String> = off.list().await.into_iter().map(|d| d.name).collect();
        assert!(off_names.contains(&"web_fetch".to_string()));

        // ON: web_fetch deferred → absent from list, still describable/executable.
        let on = build_request_tool_service(
            make(),
            BTreeSet::new(),
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::new(["web_fetch".to_string()].into()),
            None,
        );
        let on_names: Vec<String> = on.list().await.into_iter().map(|d| d.name).collect();
        assert!(!on_names.contains(&"web_fetch".to_string()));
        assert!(on.describe("web_fetch").await.is_some());
        assert!(on.execute("web_fetch", json!({})).await.is_ok());
    }
}

#[cfg(test)]
mod progressive_tests {
    use super::*;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::BTreeSet;
    use tokio_util::sync::CancellationToken;

    struct Fat(&'static str);
    #[async_trait]
    impl LoopTool for Fat {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "fat tool"
        }
        fn schema(&self) -> Value {
            json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a","b"]})
        }
        async fn execute(&self, _i: Value, _c: CancellationToken) -> ToolResult {
            ToolResult::Success { output: json!({}) }
        }
    }

    #[tokio::test]
    async fn non_core_schema_collapsed_core_kept() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(Fat("bash")));
        reg.register(Box::new(Fat("browser_navigate")));
        let svc = build_request_tool_service(
            std::sync::Arc::new(reg),
            BTreeSet::new(),
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &["bash".to_string()],
            false, // core, truncate
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );
        let schema = svc.metadata_schema();
        let bash = schema.iter().find(|d| d.name == "bash").unwrap();
        let nav = schema
            .iter()
            .find(|d| d.name == "browser_navigate")
            .unwrap();
        assert!(bash.parameters.get("properties").is_some()); // core kept
        assert!(nav.parameters.get("properties").is_none()); // non-core collapsed
        assert_eq!(nav.parameters["additionalProperties"], json!(true));
    }

    #[tokio::test]
    async fn wildcard_core_keeps_all_full() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(Fat("browser_navigate")));
        let svc = build_request_tool_service(
            std::sync::Arc::new(reg),
            BTreeSet::new(),
            None,
            None,
            None,
            "",
            None,
            crate::config::types::policies::ExecTier::Auto,
            false,
            &["*".to_string()],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );
        let nav = svc
            .metadata_schema()
            .iter()
            .find(|d| d.name == "browser_navigate")
            .cloned()
            .unwrap();
        assert!(nav.parameters.get("properties").is_some()); // escape hatch: full
    }
}
