//! SubagentSpawnTool — spawn a new sub-agent run via SubAgentDispatcher.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;

use crate::agents::sub_agents::{SubAgentDispatcher, SubAgentRequest};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for spawning a sub-agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SubagentSpawnArgs {
    /// ID of the sub-agent type to dispatch to (e.g. "mcp_agent", "skill_agent")
    #[serde(default)]
    pub agent_id: Option<String>,
    /// The task/prompt to delegate to the sub-agent
    pub task: String,
    /// Optional model override (reserved for future use)
    #[serde(default)]
    pub model: Option<String>,
    /// Timeout in seconds (default: 120)
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Output from sub-agent spawn.
#[derive(Debug, Clone, Serialize)]
pub struct SubagentSpawnOutput {
    /// The request/run ID assigned to this sub-agent execution
    pub run_id: String,
    /// Whether the sub-agent completed successfully
    pub success: bool,
    /// Summary of the sub-agent result
    pub summary: String,
    /// Number of tools called during execution
    pub tools_called: usize,
    /// Human-readable status message
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that spawns a sub-agent via SubAgentDispatcher.
///
/// Dispatches a task to an appropriate sub-agent and waits for the result
/// synchronously (with a configurable timeout).
#[derive(Clone)]
pub struct SubagentSpawnTool {
    dispatcher: Arc<RwLock<SubAgentDispatcher>>,
}

impl SubagentSpawnTool {
    pub fn new(dispatcher: Arc<RwLock<SubAgentDispatcher>>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl AlephTool for SubagentSpawnTool {
    const NAME: &'static str = "subagent_spawn";
    const DESCRIPTION: &'static str =
        "Spawn a sub-agent to handle a delegated task. The sub-agent executes \
         independently and returns its result. Use this to delegate specialized \
         work (MCP tool calls, skill execution) to a focused sub-agent.";

    type Args = SubagentSpawnArgs;
    type Output = SubagentSpawnOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "subagent_spawn(task='List my open GitHub PRs', agent_id='mcp_agent')".to_string(),
            "subagent_spawn(task='Run the data-analysis skill', timeout_secs=300)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(task = %args.task, agent_id = ?args.agent_id, "Sub-agent spawn requested");

        let timeout_secs = args.timeout_secs.unwrap_or(120);
        let timeout = Duration::from_secs(timeout_secs);

        // Build the sub-agent request
        let mut request = SubAgentRequest::new(&args.task);
        if let Some(ref agent_id) = args.agent_id {
            request = request.with_context(
                "agent_id",
                serde_json::Value::String(agent_id.clone()),
            );
        }

        // Dispatch synchronously and wait for result
        let dispatcher = self.dispatcher.read().await;
        let result = dispatcher.dispatch_sync(request, timeout).await;

        match result {
            Ok(sub_result) => {
                let tools_count = sub_result.tools_called.len();
                let message = if sub_result.success {
                    format!(
                        "Sub-agent completed successfully ({} tool calls).",
                        tools_count
                    )
                } else {
                    format!(
                        "Sub-agent finished with errors: {}",
                        sub_result.error.as_deref().unwrap_or("unknown error")
                    )
                };

                info!(
                    run_id = %sub_result.request_id,
                    success = sub_result.success,
                    tools_called = tools_count,
                    "Sub-agent spawn completed"
                );

                Ok(SubagentSpawnOutput {
                    run_id: sub_result.request_id,
                    success: sub_result.success,
                    summary: sub_result.summary,
                    tools_called: tools_count,
                    message,
                })
            }
            Err(exec_err) => {
                let msg = format!("Sub-agent execution failed: {}", exec_err);
                info!(error = %exec_err, "Sub-agent spawn failed");

                Ok(SubagentSpawnOutput {
                    run_id: String::new(),
                    success: false,
                    summary: String::new(),
                    tools_called: 0,
                    message: msg,
                })
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AlephTool;

    fn make_tool() -> SubagentSpawnTool {
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::new()));
        SubagentSpawnTool::new(dispatcher)
    }

    #[test]
    fn test_spawn_tool_definition() {
        let tool = make_tool();
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "subagent_spawn");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_spawn_no_agents_returns_graceful_error() {
        let tool = make_tool();
        let args = SubagentSpawnArgs {
            agent_id: None,
            task: "Do something".to_string(),
            model: None,
            timeout_secs: Some(5),
        };

        // With no agents registered, dispatch will fail gracefully
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.success);
    }
}
