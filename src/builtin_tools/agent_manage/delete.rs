//! `AgentDeleteTool` — delete an agent and archive its workspace.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::Result;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::agent_lifecycle::AgentLifecycleEvent;
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Guard helper
// =============================================================================

/// Returns `true` if `id` maps to a built-in agent in the catalog.
///
/// A catalog miss (user-created runtime agent with no `AgentDef`) returns
/// `false` — those agents must remain deletable.
pub(crate) fn is_protected(catalog: &crate::agents::AgentRegistry, id: &str) -> bool {
    catalog
        .get(id)
        .map(|def| def.source == crate::agents::AgentSource::Builtin)
        .unwrap_or(false)
}

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
/// Built-in agents (those whose catalog `AgentDef.source == Builtin`) cannot
/// be deleted. If the deleted agent is currently active, the session is
/// automatically switched to "main".
#[derive(Clone)]
pub struct AgentDeleteTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<AgentEnvStore>,
    event_bus: Option<Arc<GatewayEventBus>>,
    /// Catalog registry — used to detect built-in agents at delete time.
    agent_catalog: Arc<crate::agents::AgentRegistry>,
}

impl AgentDeleteTool {
    #[must_use]
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<AgentEnvStore>,
        event_bus: Option<Arc<GatewayEventBus>>,
        agent_catalog: Arc<crate::agents::AgentRegistry>,
    ) -> Self {
        Self {
            registry,
            workspace_mgr,
            event_bus,
            agent_catalog,
        }
    }
}

#[async_trait]
impl AlephTool for AgentDeleteTool {
    const NAME: &'static str = "agent_delete";
    const DESCRIPTION: &'static str =
        "Delete an agent and archive its workspace. Built-in agents cannot be deleted. \
         If the deleted agent is bound to a channel, the binding is cleared.";

    type Args = AgentDeleteArgs;
    type Output = AgentDeleteOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec!["agent_delete(agent_id='trader')".to_string()])
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "Agent deletion requested");

        // 1. Reject deletion of any built-in agent (main + store + other builtins).
        if is_protected(&self.agent_catalog, &args.agent_id) {
            return Err(crate::error::AlephError::other(format!(
                "Cannot delete the built-in '{}' agent. Built-in agents are protected.",
                args.agent_id
            )));
        }

        // 2. Verify agent exists
        if self.registry.get(&args.agent_id).await.is_none() {
            return Err(crate::error::AlephError::other(format!(
                "Agent '{}' not found",
                args.agent_id
            )));
        }

        // 3. Unbind agent from ALL channels bound to it. The binding model is
        //    many-to-one (N channels → 1 agent), so clearing only the first
        //    bound channel (the prior single-channel reverse-lookup path) left
        //    the other channels pointing at the now-deleted agent — the inbound
        //    router would then resolve them to a ghost agent.
        let _ = self.workspace_mgr.clear_bindings_for_agent(&args.agent_id);

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

        // Emit lifecycle event. Wrap in TopicEvent so the WS forwarder's topic
        // filter delivers it — a bare publish_json is topic-less and dropped by
        // concrete subscriptions (see switch.rs for the same fix).
        if deleted {
            if let Some(ref bus) = self.event_bus {
                let ev = AgentLifecycleEvent::Deleted {
                    agent_id: args.agent_id.clone(),
                    workspace_archived: true,
                };
                let _ = bus.publish_json(&TopicEvent::new(
                    ev.topic(),
                    serde_json::to_value(&ev).unwrap_or_default(),
                ));
            }
        }

        let message = if deleted {
            format!("Agent '{}' deleted and workspace archived.", args.agent_id)
        } else {
            format!(
                "Agent '{}' could not be removed from registry.",
                args.agent_id
            )
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
    use crate::agents::AgentRegistry as CatalogRegistry;
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
        let catalog = Arc::new(CatalogRegistry::with_builtins());
        let tool = AgentDeleteTool::new(registry, workspace_mgr, None, catalog);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_delete");
        assert!(def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[test]
    fn is_protected_rejects_all_builtins() {
        let catalog = CatalogRegistry::with_builtins();
        // Every built-in agent must be protected
        for id in [
            "main",
            "explore",
            "coder",
            "researcher",
            "default",
            "plan",
            "verify",
            "store",
        ] {
            assert!(
                is_protected(&catalog, id),
                "Expected built-in '{}' to be protected",
                id
            );
        }
    }

    #[test]
    fn is_protected_allows_unknown_agent() {
        let catalog = CatalogRegistry::with_builtins();
        // A user-created agent not in the catalog must NOT be protected
        assert!(!is_protected(&catalog, "nonexistent-user-agent"));
        assert!(!is_protected(&catalog, "my-custom-trader"));
    }
}
