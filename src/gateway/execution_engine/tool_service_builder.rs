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

#![cfg(feature = "phase7_traffic_flip")]

use std::collections::BTreeSet;
use crate::sync_primitives::Arc;

use crate::agents::subagent_tool::SubagentTool;
use crate::tools::refresh::ToolRefreshSource;
use crate::tools::runtime::LoopToolRegistry;
use crate::tools::scoped::ScopedToolService;
use crate::tools::service::ToolService;

/// Build the per-request `ToolService` for a chat turn.
///
/// * `tool_registry` — shared `LoopToolRegistry` (agent builtins + MCP).
/// * `allowed_tools` — tool names visible to this agent. Empty = allow-all.
/// * `subagent_tool` — optional subagent tool handle (adds the `subagent`
///   verb to the agent's toolbelt).
/// * `tool_refresh` — optional refresh source (plugin/MCP hot-reload).
pub fn build_request_tool_service(
    tool_registry: Arc<LoopToolRegistry>,
    allowed_tools: BTreeSet<String>,
    subagent_tool: Option<Arc<SubagentTool>>,
    tool_refresh: Option<Arc<dyn ToolRefreshSource>>,
) -> Arc<dyn ToolService> {
    let mut svc = ScopedToolService::new(tool_registry, allowed_tools);
    if let Some(st) = subagent_tool {
        svc = svc.with_subagent_tool(st);
    }
    if let Some(refresh) = tool_refresh {
        svc = svc.with_refresh(refresh);
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

        let svc = build_request_tool_service(registry, BTreeSet::new(), None, None);
        let defs = svc.list().await;
        assert!(defs.iter().any(|d| d.name == "read_file"));
    }

    #[tokio::test]
    async fn builder_honours_allowed_filter() {
        let mut reg = LoopToolRegistry::new();
        reg.register(Box::new(StubTool));
        let registry = Arc::new(reg);

        let allowed: BTreeSet<String> = ["other".to_string()].into_iter().collect();
        let svc = build_request_tool_service(registry, allowed, None, None);
        let defs = svc.list().await;
        assert!(
            defs.iter().all(|d| d.name != "read_file"),
            "read_file is not in allowed set so must be filtered"
        );
    }
}
