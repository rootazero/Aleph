//! AgentDeleteTool — delete an agent and archive its workspace.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::agent_lifecycle::AgentLifecycleEvent;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::workspace::WorkspaceManager;
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
    /// Injected by registry — session peer_id (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __peer_id: String,
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
    workspace_mgr: Arc<WorkspaceManager>,
    event_bus: Option<Arc<GatewayEventBus>>,
}

impl AgentDeleteTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<WorkspaceManager>,
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
         If the deleted agent is currently active, the session switches to 'main'.";

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

        // 3. If active, switch to main first (channel/peer_id injected by registry snapshot)
        let channel = args.__channel.clone();
        let peer_id = args.__peer_id.clone();
        if !channel.is_empty() && !peer_id.is_empty() {
            let current_active = self
                .workspace_mgr
                .get_active_agent(&channel, &peer_id)
                .ok()
                .flatten();

            if current_active.as_deref() == Some(args.agent_id.as_str()) {
                info!(
                    agent_id = %args.agent_id,
                    "Deleted agent is active, switching to main"
                );
                if let Err(e) = self
                    .workspace_mgr
                    .set_active_agent(&channel, &peer_id, "main")
                {
                    warn!("Failed to switch to main after deletion: {}", e);
                }
            }
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
