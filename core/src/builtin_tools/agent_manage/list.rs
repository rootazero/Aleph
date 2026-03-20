//! AgentListTool — list all registered agents and show which is active.

use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::agent_env::AgentEnvStore;
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
}

/// Information about a single agent.
#[derive(Debug, Clone, Serialize)]
pub struct AgentListInfo {
    /// Unique agent ID
    pub id: String,
    /// Human-readable display name
    pub name: String,
    /// Path to the agent's workspace
    pub workspace_path: String,
    /// LLM model used by this agent
    pub model: String,
    /// Channel this agent is bound to (if any)
    pub bound_channel: Option<String>,
}

/// Output from listing agents.
#[derive(Debug, Clone, Serialize)]
pub struct AgentListOutput {
    /// Human-readable text representation (used by slash command fast path)
    #[serde(rename = "_display")]
    pub display_text: String,
    /// All registered agents
    pub agents: Vec<AgentListInfo>,
    /// Total number of agents
    pub total: usize,
}

impl fmt::Display for AgentListOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Agents ({} total):", self.total)?;
        writeln!(f)?;
        for agent in &self.agents {
            writeln!(f, "  {} ({})", agent.name, agent.id)?;
            writeln!(f, "    model: {}", agent.model)?;
            if let Some(ref ch) = agent.bound_channel {
                writeln!(f, "    bound to: {}", ch)?;
            }
        }
        Ok(())
    }
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that lists all registered agents and shows which is active.
#[derive(Clone)]
pub struct AgentListTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<AgentEnvStore>,
}

impl AgentListTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<AgentEnvStore>,
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

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        info!("Agent list requested");

        // 1. Get all agent→channel bindings
        let bindings = self.workspace_mgr
            .get_all_agent_bindings()
            .unwrap_or_default();

        // 2. List all agents from registry
        let agent_ids = self.registry.list().await;
        let mut agents = Vec::with_capacity(agent_ids.len());

        for id in &agent_ids {
            if let Some(instance) = self.registry.get(id).await {
                agents.push(AgentListInfo {
                    id: id.clone(),
                    name: instance.display_name().to_string(),
                    workspace_path: instance.workspace().to_string_lossy().to_string(),
                    model: instance.config().model.clone(),
                    bound_channel: bindings.get(id).cloned(),
                });
            }
        }

        // Sort by id for deterministic output
        agents.sort_by(|a, b| a.id.cmp(&b.id));

        let total = agents.len();

        info!(total, "Agent list complete");

        let mut output = AgentListOutput {
            display_text: String::new(),
            agents,
            total,
        };
        output.display_text = output.to_string();

        Ok(output)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_env::AgentEnvStoreConfig;
    use crate::tools::AlephTool;
    use tempfile::tempdir;

    fn test_workspace_mgr() -> Arc<AgentEnvStore> {
        let temp = tempdir().unwrap();
        let config = AgentEnvStoreConfig {
            db_path: temp.into_path().join("test.db"),
            default_profile: "default".to_string(),
            archive_after_days: 0,
        };
        Arc::new(AgentEnvStore::new(config).unwrap())
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
