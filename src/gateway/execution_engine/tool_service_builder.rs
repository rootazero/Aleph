//! `build_request_tool_service` — compose a per-request `ToolService` for the
//! orchestrator dispatch path.
//!
//! Wraps the Phase 4b `ScopedToolService` adapter with the Gateway's
//! concrete pieces:
//!   * `LoopToolRegistry` snapshot filtered by `allowed_tools` (builtins +
//!     plugins + the MCP registry snapshot `run_loop` joins per request)
//!   * optional `SubagentTool` (when the agent has it enabled)
//!   * optional `ToolRefreshSource` for plugin / markdown-skill hot-reload
//!
//! Returned as `Arc<dyn ToolService>` for `FlowRequest.tool_service`.

use crate::sync_primitives::Arc;
use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::agents::subagent_tool::SubagentTool;
use crate::extension::hooks::HookExecutor;
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::tools::refresh::ToolRefreshSource;
use crate::tools::runtime::LoopToolRegistry;
use crate::tools::scoped::ScopedToolService;
use crate::tools::service::ToolService;

/// Process-wide confirmation requester, installed by boot once the channel
/// registry exists (see `start/mod.rs`). `build_request_tool_service` consults
/// it so confirm-flagged tools route a user confirmation before executing.
static CONFIRMATION_REQUESTER: OnceLock<Arc<dyn ApprovalRequester>> = OnceLock::new();

/// Install the process-wide tool-confirmation requester. Called once at boot.
pub fn set_confirmation_requester(requester: Arc<dyn ApprovalRequester>) {
    let _ = CONFIRMATION_REQUESTER.set(requester);
}

/// Process-wide config-tier approval requester (Phase 2b sudo), installed by
/// boot once the gateway event bus + exec approval manager exist.
static CONFIG_APPROVAL_REQUESTER: OnceLock<Arc<dyn ApprovalRequester>> = OnceLock::new();

/// Install the process-wide config-tier approval requester. Called once at boot.
pub fn set_config_approval_requester(requester: Arc<dyn ApprovalRequester>) {
    let _ = CONFIG_APPROVAL_REQUESTER.set(requester);
}

/// Process-wide MCP tool registry, installed by boot right after the MCP
/// tool bridge's target `ToolHandlerRegistry` is created (see
/// `start/mod.rs`). `run_loop` snapshots it per request so every connected
/// MCP server's tools join the agent's `LoopToolRegistry` — this is the
/// consumer side of the bridge; without it the registry is write-only and
/// external MCP tools never reach the LLM. Same install-once pattern as
/// `CONFIRMATION_REQUESTER` above.
static MCP_TOOL_REGISTRY: OnceLock<Arc<crate::tools::ToolHandlerRegistry>> = OnceLock::new();

/// Install the process-wide MCP tool registry. Called once at boot.
pub fn set_mcp_tool_registry(registry: Arc<crate::tools::ToolHandlerRegistry>) {
    let _ = MCP_TOOL_REGISTRY.set(registry);
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
/// * `tool_refresh` — optional refresh source (plugin/MCP hot-reload).
/// * `turn_context` — optional routing context of the agent turn; lets HITL
///   tools (sandbox escalation, `requires_confirmation`, `ask_user`) reach the
///   originating channel.
/// * `hook_executor` — optional extension `HookExecutor` snapshot. Wires
///   `BeforeToolCall` / `AfterToolCall` / `AfterToolCallFailure` extension
///   hooks around every tool dispatch. `None` (or an empty executor) means
///   no extension hooks are active for this request.
/// * `session_id` — session identifier surfaced into `HookContext` for
///   extension command hooks.
/// * `tool_permissions` — merged tool permission policy for this turn
///   (global `[policies.tool_permissions]` → agent override → channel
///   override, most restrictive wins). `None` = all-default policy; pass
///   `None` rather than a default config so the hot path stays a no-op.
/// * `unattended` — true for an autonomous continuation run; makes the service
///   fail closed on confirm-gated tools (no human to approve).
/// * `core_tools` — tool names kept at full schema (progressive tool
///   disclosure). Empty or `["*"]` disables collapsing (escape hatch).
/// * `truncate_tool_descriptions` — also truncate non-core tool descriptions
///   when collapsing their schema.
/// * `deferred_tool_names` — tool names hidden from the model's initial list
///   (populated by the deferred MCP tier when `defer_mcp_tools` is on).
///   Empty set (flag off) is a no-op — byte-identical surface.
pub fn build_request_tool_service(
    tool_registry: Arc<LoopToolRegistry>,
    allowed_tools: BTreeSet<String>,
    subagent_tool: Option<Arc<SubagentTool>>,
    tool_refresh: Option<Arc<dyn ToolRefreshSource>>,
    turn_context: Option<crate::tools::turn_context::TurnContext>,
    hook_executor: Option<Arc<HookExecutor>>,
    session_id: impl Into<String>,
    tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
    unattended: bool,
    core_tools: &[String],
    truncate_tool_descriptions: bool,
    deferred_tool_names: BTreeSet<String>,
) -> Arc<dyn ToolService> {
    let mut svc = ScopedToolService::new(tool_registry, allowed_tools);
    if let Some(st) = subagent_tool {
        svc = svc.with_subagent_tool(st);
    }
    if let Some(perms) = tool_permissions {
        svc = svc.with_tool_permissions(perms);
    }
    svc = svc.with_unattended(unattended);
    if let Some(refresh) = tool_refresh {
        svc = svc.with_refresh(refresh);
    }
    if let Some(tc) = turn_context {
        svc = svc.with_turn_context(tc);
    }
    if let Some(executor) = hook_executor {
        svc = svc.with_hook_executor(executor, session_id);
    }
    // Layer 2 seam: oversized tool outputs are persisted to disk and the
    // LLM gets a marker line instead of the raw text. Inert until boot
    // installs the global store via `set_global_tool_result_store`.
    if let Some(store) = crate::tools::result_store::global_tool_result_store() {
        svc = svc.with_result_store(store);
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
    svc = svc.with_deferred(deferred_tool_names);
    Arc::new(svc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

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
            None,
            "",
            None,
            false,
            &[],
            false,
            BTreeSet::new(),
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
        let deferred: BTreeSet<String> = ["web_fetch".to_string()].into_iter().collect();
        let svc = build_request_tool_service(
            Arc::new(reg),
            BTreeSet::new(),
            None,
            None,
            None,
            None,
            "",
            None,
            false,
            &[],
            false,
            deferred,
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
            None,
            "",
            None,
            false,
            &[],
            false,
            BTreeSet::new(),
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
            None,
            "",
            None,
            false,
            &[],
            false,
            BTreeSet::new(),
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
            None,
            "",
            None,
            false,
            &[],
            false,
            ["web_fetch".to_string()].into_iter().collect(),
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
            None,
            "",
            None,
            false,
            &["bash".to_string()],
            false, // core, truncate
            BTreeSet::new(),
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
            None,
            "",
            None,
            false,
            &["*".to_string()],
            false,
            BTreeSet::new(),
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
