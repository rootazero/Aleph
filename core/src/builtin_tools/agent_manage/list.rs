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

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for listing agents (no parameters needed).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentListArgs {
    /// Injected by registry — session channel (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __channel: String,
    /// Injected by registry — session peer_id (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __peer_id: String,
}

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
}

impl AgentListTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<WorkspaceManager>,
    ) -> Self {
        Self {
            registry,
            workspace_mgr,
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

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!("Agent list requested");

        // 1. Get active agent for current session (channel/peer_id injected by registry snapshot)
        let channel = args.__channel.clone();
        let peer_id = args.__peer_id.clone();
        let active_agent = if !channel.is_empty() && !peer_id.is_empty() {
            self.workspace_mgr
                .get_active_agent(&channel, &peer_id)
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
        let tool = AgentListTool::new(registry, workspace_mgr);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_list");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }
}
