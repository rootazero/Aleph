//! `AgentDeleteTool` — delete an agent and archive its workspace.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::agent_manager::AgentManager;
use crate::error::Result;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::agent_lifecycle::AgentLifecycleEvent;
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::error::AgentManageError;

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
/// be deleted. Deletion does not rebind any channel: it clears every binding
/// pointing at the deleted agent (`clear_bindings_for_agent`), and those
/// channels then resolve to the router's default agent ("main" unless
/// reconfigured) on the next inbound message.
///
/// When an [`AgentManager`] is wired, the TOML `[[agents.list]]` definition is
/// deleted too (with the manager's own default-agent / only-agent guards) —
/// otherwise the agent silently resurrects at the next daemon boot, because
/// boot re-registers every TOML definition.
#[derive(Clone)]
pub struct AgentDeleteTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<AgentEnvStore>,
    event_bus: Option<Arc<GatewayEventBus>>,
    /// Catalog registry — used to detect built-in agents at delete time.
    agent_catalog: Arc<crate::agents::AgentRegistry>,
    /// Persisted-definition manager (`[[agents.list]]` in config TOML) —
    /// deletes the entry so the agent stays deleted across restarts; without
    /// it boot lazily re-registers the TOML definition and the "deleted"
    /// agent resurrects. `None` (tests / embedded / minimal servers) skips
    /// persistence, mirroring `AgentCreateTool::with_agent_manager`.
    agent_manager: Option<Arc<AgentManager>>,
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
            agent_manager: None,
        }
    }

    /// Wire the persisted-definition manager (builder form keeps `new`
    /// four-arg so every existing caller and test compiles unchanged).
    #[must_use]
    pub fn with_agent_manager(mut self, manager: Arc<AgentManager>) -> Self {
        self.agent_manager = Some(manager);
        self
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

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "Agent deletion requested");

        // 1. Reject deletion of any built-in agent (main + other builtins).
        if is_protected(&self.agent_catalog, &args.agent_id) {
            return Err(AgentManageError::ProtectedAgent(format!(
                "the built-in '{}' agent. Built-in agents are protected.",
                args.agent_id
            ))
            .into());
        }

        // 2. Verify agent exists (existence check only — no need to
        //    instantiate a lazy entry just to delete it).
        if !self.registry.contains(&args.agent_id).await {
            // Build the available list from the runtime registry so the LLM
            // can pick a real existing target without guessing.
            let mut available = self.registry.list().await;
            available.sort();
            return Err(AgentManageError::AgentNotFound {
                agent_id: args.agent_id.clone(),
                available,
            }
            .into());
        }

        // 3. Delete the persisted TOML definition FIRST, while its guards can
        //    still abort the whole operation (cannot delete the default agent
        //    or the only agent). The manager also moves the workspace + state
        //    dirs to trash. Skipped for runtime-only agents that never got a
        //    TOML entry — those fall through to the legacy archival below.
        let mut dirs_archived_by_manager = false;
        if let Some(ref manager) = self.agent_manager {
            if manager.get(&args.agent_id).is_ok() {
                manager.delete(&args.agent_id).map_err(|e| {
                    AgentManageError::Store(format!(
                        "Cannot delete agent '{}': {}",
                        args.agent_id, e
                    ))
                })?;
                dirs_archived_by_manager = true;
            }
        }

        // 4. Unbind agent from ALL channels bound to it. The binding model is
        //    many-to-one (N channels → 1 agent), so clearing only the first
        //    bound channel (the prior single-channel reverse-lookup path) left
        //    the other channels pointing at the now-deleted agent — the inbound
        //    router would then resolve them to a ghost agent.
        if let Err(e) = self.workspace_mgr.clear_bindings_for_agent(&args.agent_id) {
            warn!(agent_id = %args.agent_id, error = %e, "Failed to clear channel bindings for deleted agent");
        }

        // 5. Remove from registry. Both variants (Instance / Lazy) mean the
        //    agent is gone; the previous API collapsed Lazy into None, which
        //    misreported "could not be removed" and skipped archival + the
        //    lifecycle event for never-instantiated agents.
        let removed = self.registry.remove(&args.agent_id).await;
        let deleted = removed.is_some();

        // 6. Archive workspace + state dir (rename to .archived) — legacy path
        //    for agents without a TOML entry; the manager already trashed the
        //    dirs otherwise.
        if !dirs_archived_by_manager {
            if let Some(ref removed) = removed {
                let workspace = removed.workspace();
                let archived = workspace.with_extension("archived");
                if workspace.exists() {
                    if let Err(e) = tokio::fs::rename(workspace, &archived).await {
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

                // Also archive agent state directory (~/.aleph/agents/{id}/).
                // Resolved via `discovery::aleph_home_dir()` for the same reason
                // `create` does — `ALEPH_HOME` override parity.
                let Some(home) = crate::discovery::aleph_home_dir().ok() else {
                    warn!(
                        agent_id = %args.agent_id,
                        "Cannot determine home directory, skipping agent state archive"
                    );
                    return Ok(AgentDeleteOutput {
                        deleted,
                        message: format!(
                            "Agent '{}' deleted but agent state could not be archived (no home directory).",
                            args.agent_id
                        ),
                    });
                };
                let agent_state_dir = home.join("agents").join(&args.agent_id);
                if agent_state_dir.exists() {
                    let archived_state = agent_state_dir.with_extension("archived");
                    if let Err(e) = tokio::fs::rename(&agent_state_dir, &archived_state).await {
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
        }

        // Emit lifecycle event (TopicEvent-wrapped inside `publish`).
        if deleted {
            if let Some(ref bus) = self.event_bus {
                AgentLifecycleEvent::Deleted {
                    agent_id: args.agent_id.clone(),
                    workspace_archived: true,
                }
                .publish(bus);
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
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::tools::AlephTool;

    #[test]
    fn test_delete_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let catalog = Arc::new(CatalogRegistry::with_builtins());
        let tool = AgentDeleteTool::new(registry, wm, None, catalog);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_delete");
        assert!(def.requires_confirmation);
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

    #[tokio::test]
    async fn delete_lazy_agent_reports_deleted_and_clears_bindings() {
        use crate::gateway::agent_instance::AgentInstanceConfig;

        // A lazily-registered (never instantiated) agent must still report
        // deleted=true and drop its channel bindings — the old
        // `Option<Arc<AgentInstance>>` remove() collapsed this case to None.
        let registry = Arc::new(AgentRegistry::new());
        let (sm, _sm_temp) = test_utils::session_store();
        let root = tempfile::tempdir().unwrap().keep();
        registry
            .register_config(
                AgentInstanceConfig {
                    agent_id: "trader".to_string(),
                    workspace: root.join("workspace"),
                    agent_dir: root.join("state"),
                    ..Default::default()
                },
                sm,
            )
            .await;

        let (wm, _wm_temp) = test_utils::workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();
        wm.set_active_agent("discord", "trader").unwrap();

        let catalog = Arc::new(CatalogRegistry::with_builtins());
        let tool = AgentDeleteTool::new(Arc::clone(&registry), Arc::clone(&wm), None, catalog);

        let out = tool
            .call(AgentDeleteArgs {
                agent_id: "trader".to_string(),
            })
            .await
            .expect("delete should succeed");

        assert!(out.deleted, "lazy agent deletion must report deleted=true");
        assert!(!registry.contains("trader").await);
        assert!(wm.get_active_agent("telegram").unwrap().is_none());
        assert!(wm.get_active_agent("discord").unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_unknown_agent_errors_with_available_list() {
        let registry = Arc::new(AgentRegistry::new());
        // Pre-register a known agent so the not-found error can list it.
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let catalog = Arc::new(CatalogRegistry::with_builtins());
        let tool = AgentDeleteTool::new(registry, wm, None, catalog);

        let err = tool
            .call(AgentDeleteArgs {
                agent_id: "ghost".to_string(),
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"), "got: {msg}");
        assert!(msg.contains("trader"), "available list missing: {msg}");
    }

    #[tokio::test]
    async fn delete_builtin_main_is_protected() {
        let registry = Arc::new(AgentRegistry::new());
        // Register "main" so the existence check passes — the catalog guard
        // is what should reject the delete.
        let (instance, _sm, _t) = test_utils::instance("main");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let catalog = Arc::new(CatalogRegistry::with_builtins());
        let tool = AgentDeleteTool::new(registry, wm, None, catalog);

        let err = tool
            .call(AgentDeleteArgs {
                agent_id: "main".to_string(),
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("built-in"), "got: {msg}");
    }
}