//! SubagentSteerTool — send additional instructions to a running sub-agent.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::agents::sub_agents::{SubAgentDispatcher, SubAgentRegistry};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for steering a running sub-agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SubagentSteerArgs {
    /// Run ID of the sub-agent to steer
    pub run_id: String,
    /// Additional instructions or message to send to the sub-agent
    pub message: String,
}

/// Output from sub-agent steer.
#[derive(Debug, Clone, Serialize)]
pub struct SubagentSteerOutput {
    /// Whether the steer message was accepted
    pub accepted: bool,
    /// Human-readable status message
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that sends additional instructions to a running sub-agent.
///
/// Currently a stub — the SubAgentDispatcher does not yet support injecting
/// messages into an in-flight run. This tool validates the run exists and
/// returns an informational message.
#[derive(Clone)]
pub struct SubagentSteerTool {
    #[allow(dead_code)]
    dispatcher: Arc<RwLock<SubAgentDispatcher>>,
    registry: Arc<SubAgentRegistry>,
}

impl SubagentSteerTool {
    pub fn new(
        dispatcher: Arc<RwLock<SubAgentDispatcher>>,
        registry: Arc<SubAgentRegistry>,
    ) -> Self {
        Self {
            dispatcher,
            registry,
        }
    }
}

#[async_trait]
impl AlephTool for SubagentSteerTool {
    const NAME: &'static str = "subagent_steer";
    const DESCRIPTION: &'static str =
        "Send additional instructions to a running sub-agent. This allows you \
         to redirect or refine a sub-agent's task while it is still executing. \
         Note: steering is not yet fully supported — the message is recorded \
         but may not be injected into the running execution.";

    type Args = SubagentSteerArgs;
    type Output = SubagentSteerOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "subagent_steer(run_id='abc-123', message='Focus only on open PRs from this week')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(run_id = %args.run_id, "Sub-agent steer requested");

        // Validate the run exists
        let run = self.registry.get(&args.run_id).await?;

        match run {
            Some(run) => {
                if run.status.is_terminal() {
                    let msg = format!(
                        "Run '{}' has already ended (status: {:?}). Cannot steer a completed run.",
                        args.run_id, run.status
                    );
                    info!(run_id = %args.run_id, status = ?run.status, "Steer rejected: terminal");
                    Ok(SubagentSteerOutput {
                        accepted: false,
                        message: msg,
                    })
                } else {
                    // The SubAgentDispatcher does not currently support injecting
                    // messages into a running execution. Log the intent and return
                    // a stub response.
                    let msg = format!(
                        "Steer message recorded for run '{}', but live injection is not yet \
                         implemented. The sub-agent will continue with its original task. \
                         Message: {}",
                        args.run_id,
                        args.message.chars().take(200).collect::<String>()
                    );
                    info!(run_id = %args.run_id, "Steer message recorded (stub)");
                    Ok(SubagentSteerOutput {
                        accepted: true,
                        message: msg,
                    })
                }
            }
            None => {
                let msg = format!("Run '{}' not found in registry.", args.run_id);
                info!(run_id = %args.run_id, "Steer rejected: run not found");
                Ok(SubagentSteerOutput {
                    accepted: false,
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

    fn make_tool() -> SubagentSteerTool {
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::new()));
        let registry = Arc::new(SubAgentRegistry::new_in_memory());
        SubagentSteerTool::new(dispatcher, registry)
    }

    #[test]
    fn test_steer_tool_definition() {
        let tool = make_tool();
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "subagent_steer");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_steer_nonexistent_run() {
        let tool = make_tool();
        let args = SubagentSteerArgs {
            run_id: "nonexistent-run".to_string(),
            message: "Focus on X".to_string(),
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.accepted);
        assert!(result.message.contains("not found"));
    }
}
