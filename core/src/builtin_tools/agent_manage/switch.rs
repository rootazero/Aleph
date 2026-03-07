//! AgentSwitchTool — switch the active agent for the current conversation.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::workspace::WorkspaceManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::SessionContextHandle;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for switching the active agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentSwitchArgs {
    /// ID of the agent to switch to
    pub agent_id: String,
}

/// Output from agent switching.
#[derive(Debug, Clone, Serialize)]
pub struct AgentSwitchOutput {
    /// The agent ID that is now active
    pub agent_id: String,
    /// The previously active agent ID (if any)
    pub previous_agent: Option<String>,
    /// Human-readable status message
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that switches the active agent for the current conversation.
#[derive(Clone)]
pub struct AgentSwitchTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<WorkspaceManager>,
    session_ctx: SessionContextHandle,
}

impl AgentSwitchTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<WorkspaceManager>,
        session_ctx: SessionContextHandle,
    ) -> Self {
        Self {
            registry,
            workspace_mgr,
            session_ctx,
        }
    }
}

#[async_trait]
impl AlephTool for AgentSwitchTool {
    const NAME: &'static str = "agent_switch";
    const DESCRIPTION: &'static str =
        "Switch to an existing agent for the current conversation. Future messages \
         will be handled by the specified agent with its own workspace and memory.";

    type Args = AgentSwitchArgs;
    type Output = AgentSwitchOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "agent_switch(agent_id='trader')".to_string(),
            "agent_switch(agent_id='main')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "Agent switch requested");

        // 1. Verify target agent exists in registry
        if self.registry.get(&args.agent_id).await.is_none() {
            return Err(crate::error::AlephError::other(format!(
                "Agent '{}' not found. Use agent_list to see available agents.",
                args.agent_id
            )));
        }

        // 2. Get current active agent
        let session = self.session_ctx.read().await;
        let previous = if !session.channel.is_empty() && !session.peer_id.is_empty() {
            self.workspace_mgr
                .get_active_agent(&session.channel, &session.peer_id)
                .ok()
                .flatten()
        } else {
            None
        };

        // 3. Set new active agent
        if !session.channel.is_empty() && !session.peer_id.is_empty() {
            self.workspace_mgr
                .set_active_agent(&session.channel, &session.peer_id, &args.agent_id)
                .map_err(|e| crate::error::AlephError::other(format!(
                    "Failed to switch agent: {}",
                    e
                )))?;
        }

        let msg = match &previous {
            Some(prev) => format!(
                "Switched from '{}' to '{}'. Future messages will be handled by '{}'.",
                prev, args.agent_id, args.agent_id
            ),
            None => format!(
                "Switched to '{}'. Future messages will be handled by '{}'.",
                args.agent_id, args.agent_id
            ),
        };

        info!(
            agent_id = %args.agent_id,
            previous = ?previous,
            "Agent switched successfully"
        );

        Ok(AgentSwitchOutput {
            agent_id: args.agent_id,
            previous_agent: previous,
            message: msg,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::workspace::WorkspaceManagerConfig;
    use crate::tools::AlephTool;
    use tempfile::tempdir;

    fn test_workspace_mgr() -> Arc<WorkspaceManager> {
        let temp = tempdir().unwrap();
        let config = WorkspaceManagerConfig {
            db_path: temp.into_path().join("test.db"),
            default_profile: "default".to_string(),
            archive_after_days: 0,
        };
        Arc::new(WorkspaceManager::new(config).unwrap())
    }

    #[test]
    fn test_switch_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let workspace_mgr = test_workspace_mgr();
        let session_ctx = super::super::new_session_context_handle();
        let tool = AgentSwitchTool::new(registry, workspace_mgr, session_ctx);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_switch");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }
}
