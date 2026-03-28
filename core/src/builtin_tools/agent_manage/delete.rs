//! AgentDeleteTool — delete an agent and archive its workspace.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::agent_lifecycle::AgentLifecycleEvent;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::agent_env::AgentEnvStore;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for deleting an agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentDeleteArgs {
    /// ID of the agent to delete
    pub agent_id: String,
    /// Injected by registry — session channel (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __channel: String,
}

/// Output from agent deletion.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDeleteOutput {
    /// Whether the agent was successfully deleted
    pub deleted: bool,
    /// Human-readable status message
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that deletes an agent and archives its workspace.
///
/// The "main" agent cannot be deleted. If the deleted agent is currently
/// active, the session is automatically switched to "main".
#[derive(Clone)]
pub struct AgentDeleteTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<AgentEnvStore>,
    event_bus: Option<Arc<GatewayEventBus>>,
}

impl AgentDeleteTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<AgentEnvStore>,
        event_bus: Option<Arc<GatewayEventBus>>,
    ) -> Self {
        Self {
            registry,
            workspace_mgr,
            event_bus,
        }
    }
}

#[async_trait]
impl AlephTool for AgentDeleteTool {
    const NAME: &'static str = "agent_delete";
    const DESCRIPTION: &'static str =
        "Delete an agent and archive its workspace. The 'main' agent cannot be deleted. \
         If the deleted agent is bound to a channel, the binding is cleared.";

    type Args = AgentDeleteArgs;
    type Output = AgentDeleteOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "agent_delete(agent_id='trader')".to_string(),
        ])
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "Agent deletion requested");

        // 1. Reject "main" deletion
        if args.agent_id == "main" {
            return Err(crate::error::AlephError::other(
                "Cannot delete the 'main' agent. It is the default agent and must always exist.",
            ));
        }

        // 2. Verify agent exists
        if self.registry.get(&args.agent_id).await.is_none() {
            return Err(crate::error::AlephError::other(format!(
                "Agent '{}' not found",
                args.agent_id
            )));
        }

        // 3. Unbind agent from its channel if bound
        if let Ok(Some(bound_channel)) = self.workspace_mgr.get_channel_for_agent(&args.agent_id) {
            let _ = self.workspace_mgr.clear_active_agent(&bound_channel);
        }

        // 4. Remove from registry
        let removed = self.registry.remove(&args.agent_id).await;

        // 5. Archive workspace (rename to .archived)
        if let Some(ref instance) = removed {
            let workspace = instance.workspace();
            let archived = workspace.with_extension("archived");
            if workspace.exists() {
                if let Err(e) = std::fs::rename(workspace, &archived) {
                    warn!(
                        agent_id = %args.agent_id,
                        error = %e,
                        "Failed to archive workspace, it will remain on disk"
                    );
                } else {
                    info!(
                        agent_id = %args.agent_id,
                        archived_path = %archived.display(),
                        "Workspace archived"
                    );
                }
            }

            // Also archive agent state directory (~/.aleph/agents/{id}/)
            let agent_state_dir = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join(".aleph")
                .join("agents")
                .join(&args.agent_id);
            if agent_state_dir.exists() {
                let archived_state = agent_state_dir.with_extension("archived");
                if let Err(e) = std::fs::rename(&agent_state_dir, &archived_state) {
                    warn!(
                        agent_id = %args.agent_id,
                        error = %e,
                        "Failed to archive agent state directory"
                    );
                } else {
                    info!(
                        agent_id = %args.agent_id,
                        archived_path = %archived_state.display(),
                        "Agent state directory archived"
                    );
                }
            }
        }

        let deleted = removed.is_some();

        // Emit lifecycle event
        if deleted {
            if let Some(ref bus) = self.event_bus {
                let _ = bus.publish_json(&AgentLifecycleEvent::Deleted {
                    agent_id: args.agent_id.clone(),
                    workspace_archived: true,
                });
            }
        }

        let message = if deleted {
            format!("Agent '{}' deleted and workspace archived.", args.agent_id)
        } else {
            format!("Agent '{}' could not be removed from registry.", args.agent_id)
        };

        info!(agent_id = %args.agent_id, deleted, "Agent deletion complete");

        Ok(AgentDeleteOutput { deleted, message })
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
            db_path: temp.keep().join("test.db"),
            default_profile: "default".to_string(),
            archive_after_days: 0,
        };
        Arc::new(AgentEnvStore::new(config).unwrap())
    }

    #[test]
    fn test_delete_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let workspace_mgr = test_workspace_mgr();
        let tool = AgentDeleteTool::new(registry, workspace_mgr, None);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_delete");
        assert!(def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }
}
