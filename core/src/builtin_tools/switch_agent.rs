//! Switch Agent Tool — LLM-driven agent switching
//!
//! Allows the AI to switch the active agent for the current channel/peer
//! based on natural language intent detection.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::WorkspaceManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the switch_agent tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SwitchAgentArgs {
    /// The agent ID to switch to (e.g., "health", "trading", "coding")
    pub agent_id: String,
    /// The channel identifier where the switch should take effect
    pub channel_id: String,
    /// The peer/sender identifier within the channel
    pub peer_id: String,
    /// Reason for switching (for auditability)
    pub reason: String,
}

/// Output from the switch_agent tool
#[derive(Debug, Clone, Serialize)]
pub struct SwitchAgentOutput {
    /// Whether the switch succeeded
    pub success: bool,
    /// Human-readable result message
    pub message: String,
    /// The agent that is now active
    pub active_agent: String,
    /// Available agents (for reference)
    pub available_agents: Vec<String>,
}

/// Tool that allows the LLM to switch the active agent for a channel/peer
#[derive(Clone)]
pub struct SwitchAgentTool {
    workspace_manager: Arc<WorkspaceManager>,
    agent_registry: Arc<AgentRegistry>,
}

impl SwitchAgentTool {
    pub fn new(workspace_manager: Arc<WorkspaceManager>, agent_registry: Arc<AgentRegistry>) -> Self {
        Self {
            workspace_manager,
            agent_registry,
        }
    }
}

#[async_trait]
impl AlephTool for SwitchAgentTool {
    const NAME: &'static str = "switch_agent";
    const DESCRIPTION: &'static str =
        "Switch the active AI agent for the current conversation channel. \
         Use when the user wants to talk to a different agent persona \
         (e.g., switching from the main assistant to a health advisor, \
         trading assistant, or coding expert). The switch persists for \
         future messages on the same channel until changed again.";

    type Args = SwitchAgentArgs;
    type Output = SwitchAgentOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "switch_agent(agent_id='health', channel_id='telegram', peer_id='123456', reason='User asked about health topics')"
                .to_string(),
            "switch_agent(agent_id='trading', channel_id='discord', peer_id='user#1234', reason='User wants to discuss Bitcoin trading')"
                .to_string(),
            "switch_agent(agent_id='main', channel_id='telegram', peer_id='123456', reason='User wants to go back to the main assistant')"
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let available = self.agent_registry.list().await;

        // Check if agent exists
        if self.agent_registry.get(&args.agent_id).await.is_none() {
            return Ok(SwitchAgentOutput {
                success: false,
                message: format!(
                    "Agent '{}' not found. Available agents: {}",
                    args.agent_id,
                    available.join(", ")
                ),
                active_agent: String::new(),
                available_agents: available,
            });
        }

        // Perform the switch
        match self.workspace_manager.set_active_agent(&args.channel_id, &args.peer_id, &args.agent_id) {
            Ok(()) => {
                info!(
                    agent_id = %args.agent_id,
                    channel = %args.channel_id,
                    peer = %args.peer_id,
                    reason = %args.reason,
                    "Agent switched via tool"
                );

                Ok(SwitchAgentOutput {
                    success: true,
                    message: format!(
                        "Switched to agent '{}'. Future messages on this channel will be handled by this agent.",
                        args.agent_id
                    ),
                    active_agent: args.agent_id,
                    available_agents: available,
                })
            }
            Err(e) => Ok(SwitchAgentOutput {
                success: false,
                message: format!("Failed to switch agent: {}", e),
                active_agent: String::new(),
                available_agents: available,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name_and_description() {
        assert_eq!(SwitchAgentTool::NAME, "switch_agent");
        assert!(SwitchAgentTool::DESCRIPTION.contains("Switch"));
    }
}
