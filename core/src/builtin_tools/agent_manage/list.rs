//! AgentListTool — list all registered agents and show which is active.

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

/// Arguments for listing agents (no parameters needed).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentListArgs {}

/// Information about a single agent.
#[derive(Debug, Clone, Serialize)]
pub struct AgentListInfo {
    /// Unique agent ID
    pub id: String,
    /// Path to the agent's workspace
    pub workspace_path: String,
    /// LLM model used by this agent
    pub model: String,
    /// Whether this agent is currently active for the caller's session
    pub is_active: bool,
}

/// Output from listing agents.
#[derive(Debug, Clone, Serialize)]
pub struct AgentListOutput {
    /// All registered agents
    pub agents: Vec<AgentListInfo>,
    /// Currently active agent for the caller's session (if any)
    pub active_agent: Option<String>,
    /// Total number of agents
    pub total: usize,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that lists all registered agents and shows which is active.
#[derive(Clone)]
pub struct AgentListTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<WorkspaceManager>,
    session_ctx: SessionContextHandle,
}

impl AgentListTool {
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
impl AlephTool for AgentListTool {
    const NAME: &'static str = "agent_list";
    const DESCRIPTION: &'static str =
        "List all available agents and show which one is currently active \
         for this conversation.";

    type Args = AgentListArgs;
    type Output = AgentListOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "agent_list()".to_string(),
        ])
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        info!("Agent list requested");

        // 1. Get active agent for current session
        let session = self.session_ctx.read().await;
        let active_agent = if !session.channel.is_empty() && !session.peer_id.is_empty() {
            self.workspace_mgr
                .get_active_agent(&session.channel, &session.peer_id)
                .ok()
                .flatten()
        } else {
            None
        };

        // 2. List all agents from registry
        let agent_ids = self.registry.list().await;
        let mut agents = Vec::with_capacity(agent_ids.len());

        for id in &agent_ids {
            if let Some(instance) = self.registry.get(id).await {
                agents.push(AgentListInfo {
                    id: id.clone(),
                    workspace_path: instance.workspace().to_string_lossy().to_string(),
                    model: instance.config().model.clone(),
                    is_active: active_agent.as_deref() == Some(id.as_str()),
                });
            }
        }

        // Sort by id for deterministic output
        agents.sort_by(|a, b| a.id.cmp(&b.id));

        let total = agents.len();

        info!(total, active = ?active_agent, "Agent list complete");

        Ok(AgentListOutput {
            agents,
            active_agent,
            total,
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
    fn test_list_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let workspace_mgr = test_workspace_mgr();
        let session_ctx = super::super::new_session_context_handle();
        let tool = AgentListTool::new(registry, workspace_mgr, session_ctx);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_list");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }
}
