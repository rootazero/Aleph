//! `build_request_tool_service` — compose a per-request `ToolService` for the
//! orchestrator dispatch path.
//!
//! Wraps the Phase 4b `ScopedToolService` adapter with the Gateway's
//! concrete pieces:
//!   * `LoopToolRegistry` snapshot filtered by `allowed_tools`
//!   * optional `SubagentTool` (when the agent has it enabled)
//!   * optional `ToolRefreshSource` for dynamic MCP tools
//!
//! Returned as `Arc<dyn ToolService>` for `FlowRequest.tool_service`.

use crate::sync_primitives::Arc;
use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::agents::subagent_tool::SubagentTool;
use crate::executor::CONFIRMATION_REQUIRED_TOOLS;
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
pub fn build_request_tool_service(
    tool_registry: Arc<LoopToolRegistry>,
    allowed_tools: BTreeSet<String>,
    subagent_tool: Option<Arc<SubagentTool>>,
    tool_refresh: Option<Arc<dyn ToolRefreshSource>>,
    turn_context: Option<crate::tools::turn_context::TurnContext>,
) -> Arc<dyn ToolService> {
    let mut svc = ScopedToolService::new(tool_registry, allowed_tools);
    if let Some(st) = subagent_tool {
        svc = svc.with_subagent_tool(st);
    }
    if let Some(refresh) = tool_refresh {
        svc = svc.with_refresh(refresh);
    }
    if let Some(tc) = turn_context {
        svc = svc.with_turn_context(tc);
    }
    // Layer 2 seam: oversized tool outputs are persisted to disk and the
    // LLM gets a marker line instead of the raw text. Inert until boot
    // installs the global store via `set_global_tool_result_store`.
    if let Some(store) = crate::tools::result_store::global_tool_result_store() {
        svc = svc.with_result_store(store);
    }
    // Wire the confirmation seam: confirm-flagged tools route a user prompt
    // before executing. Inert until boot installs the requester.
    if let Some(requester) = CONFIRMATION_REQUESTER.get() {
        let confirm: BTreeSet<String> = CONFIRMATION_REQUIRED_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect();
        svc = svc.with_confirmation(confirm, Arc::clone(requester));
    }
    Arc::new(svc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
    use async_trait::async_trait;
    use serde_json::{json, Value};

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
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Success { output: json!({}) }
        }
    }

    #[tokio::test]
    async fn builder_returns_listable_service() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(StubTool));
        let registry = Arc::new(reg);

        let svc = build_request_tool_service(registry, BTreeSet::new(), None, None, None);
        let defs = svc.list().await;
        assert!(defs.iter().any(|d| d.name == "read_file"));
    }

    #[tokio::test]
    async fn builder_honours_allowed_filter() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(StubTool));
        let registry = Arc::new(reg);

        let allowed: BTreeSet<String> = ["other".to_string()].into_iter().collect();
        let svc = build_request_tool_service(registry, allowed, None, None, None);
        let defs = svc.list().await;
        assert!(
            defs.iter().all(|d| d.name != "read_file"),
            "read_file is not in allowed set so must be filtered"
        );
    }
}
