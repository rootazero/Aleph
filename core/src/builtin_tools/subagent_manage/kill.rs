//! SubagentKillTool — cancel/kill a running sub-agent run.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::agents::sub_agents::{RunStatus, SubAgentDispatcher, SubAgentRegistry};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for killing a sub-agent run.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SubagentKillArgs {
    /// Run ID of the sub-agent to kill
    pub run_id: String,
}

/// Output from sub-agent kill.
#[derive(Debug, Clone, Serialize)]
pub struct SubagentKillOutput {
    /// Whether the run was successfully cancelled
    pub killed: bool,
    /// Human-readable status message
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that cancels/kills a running sub-agent run.
///
/// Transitions the run to Cancelled status in the SubAgentRegistry.
/// If the run is already in a terminal state, it reports that no action
/// was needed.
#[derive(Clone)]
pub struct SubagentKillTool {
    #[allow(dead_code)]
    dispatcher: Arc<RwLock<SubAgentDispatcher>>,
    registry: Arc<SubAgentRegistry>,
}

impl SubagentKillTool {
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
impl AlephTool for SubagentKillTool {
    const NAME: &'static str = "subagent_kill";
    const DESCRIPTION: &'static str =
        "Cancel/kill a running sub-agent. Use this to stop a sub-agent that \
         is no longer needed or is taking too long. The sub-agent's run status \
         will be set to 'cancelled'.";

    type Args = SubagentKillArgs;
    type Output = SubagentKillOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "subagent_kill(run_id='abc-123')".to_string(),
        ])
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(run_id = %args.run_id, "Sub-agent kill requested");

        // Look up the run
        let run = self.registry.get(&args.run_id).await?;

        match run {
            Some(run) => {
                if run.status.is_terminal() {
                    let msg = format!(
                        "Run '{}' is already in terminal state ({:?}). No action needed.",
                        args.run_id, run.status
                    );
                    info!(run_id = %args.run_id, status = ?run.status, "Kill skipped: already terminal");
                    Ok(SubagentKillOutput {
                        killed: false,
                        message: msg,
                    })
                } else {
                    // Transition to Cancelled
                    match self.registry.transition(&args.run_id, RunStatus::Cancelled).await {
                        Ok(()) => {
                            let msg = format!(
                                "Run '{}' cancelled successfully (was {:?}).",
                                args.run_id, run.status
                            );
                            info!(run_id = %args.run_id, old_status = ?run.status, "Sub-agent killed");
                            Ok(SubagentKillOutput {
                                killed: true,
                                message: msg,
                            })
                        }
                        Err(e) => {
                            let msg = format!(
                                "Failed to cancel run '{}': {}",
                                args.run_id, e
                            );
                            info!(run_id = %args.run_id, error = %e, "Kill failed");
                            Ok(SubagentKillOutput {
                                killed: false,
                                message: msg,
                            })
                        }
                    }
                }
            }
            None => {
                let msg = format!("Run '{}' not found in registry.", args.run_id);
                info!(run_id = %args.run_id, "Kill rejected: run not found");
                Ok(SubagentKillOutput {
                    killed: false,
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
    use crate::agents::sub_agents::SubAgentRun;
    use crate::routing::SessionKey;
    use crate::tools::AlephTool;

    fn make_tool() -> (SubagentKillTool, Arc<SubAgentRegistry>) {
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::new()));
        let registry = Arc::new(SubAgentRegistry::new_in_memory());
        let tool = SubagentKillTool::new(dispatcher, registry.clone());
        (tool, registry)
    }

    #[test]
    fn test_kill_tool_definition() {
        let (tool, _) = make_tool();
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "subagent_kill");
        assert!(def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_kill_nonexistent_run() {
        let (tool, _) = make_tool();
        let args = SubagentKillArgs {
            run_id: "nonexistent-run".to_string(),
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.killed);
        assert!(result.message.contains("not found"));
    }

    #[tokio::test]
    async fn test_kill_pending_run() {
        let (tool, registry) = make_tool();

        let parent_key = SessionKey::main("parent");
        let session_key = SessionKey::Subagent {
            parent_key: Box::new(parent_key.clone()),
            subagent_id: "test-sub".to_string(),
        };
        let run = SubAgentRun::new(session_key, parent_key, "Test task", "mcp");
        let run_id = run.run_id.clone();
        registry.register(run).await.unwrap();

        let args = SubagentKillArgs { run_id: run_id.clone() };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.killed);
        assert!(result.message.contains("cancelled"));

        // Verify status changed
        let updated = registry.get(&run_id).await.unwrap().unwrap();
        assert_eq!(updated.status, RunStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_kill_already_completed_run() {
        let (tool, registry) = make_tool();

        let parent_key = SessionKey::main("parent");
        let session_key = SessionKey::Subagent {
            parent_key: Box::new(parent_key.clone()),
            subagent_id: "test-sub-2".to_string(),
        };
        let run = SubAgentRun::new(session_key, parent_key, "Test task", "skill");
        let run_id = run.run_id.clone();
        registry.register(run).await.unwrap();

        // Transition through Running -> Completed
        registry.transition(&run_id, RunStatus::Running).await.unwrap();
        registry.transition(&run_id, RunStatus::Completed).await.unwrap();

        let args = SubagentKillArgs { run_id };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.killed);
        assert!(result.message.contains("terminal"));
    }
}
