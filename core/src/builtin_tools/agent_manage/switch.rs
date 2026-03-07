//! AgentSwitchTool — switch the active agent for the current conversation.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

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

/// Arguments for switching the active agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentSwitchArgs {
    /// ID of the agent to switch to
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
    event_bus: Option<Arc<GatewayEventBus>>,
}

impl AgentSwitchTool {
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

    async fn call(&self, mut args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "Agent switch requested");

        // 1. Verify target agent exists in registry (try exact ID, then name → ID)
        if self.registry.get(&args.agent_id).await.is_none() {
            // Try converting display name to agent ID (e.g. "交易助手" → "agent-73f36847")
            let generated_id = super::create::generate_agent_id_from_name(&args.agent_id);
            if self.registry.get(&generated_id).await.is_some() {
                info!(
                    original = %args.agent_id,
                    resolved = %generated_id,
                    "Resolved agent name to ID"
                );
                args.agent_id = generated_id;
            } else {
                let available = self.registry.list().await.join(", ");
                return Err(crate::error::AlephError::other(format!(
                    "Agent '{}' not found. Available: {}",
                    args.agent_id, available
                )));
            }
        }

        // 2. Get current active agent (channel/peer_id injected by registry snapshot)
        let channel = args.__channel.clone();
        let peer_id = args.__peer_id.clone();
        let previous = if !channel.is_empty() && !peer_id.is_empty() {
            self.workspace_mgr
                .get_active_agent(&channel, &peer_id)
                .ok()
                .flatten()
        } else {
            None
        };

        // 3. Set or clear active agent override
        if !channel.is_empty() && !peer_id.is_empty() {
            if args.agent_id == "main" {
                // Clear the override so routing falls through to config bindings / default
                self.workspace_mgr
                    .clear_active_agent(&channel, &peer_id)
                    .map_err(|e| crate::error::AlephError::other(format!(
                        "Failed to clear agent override: {}",
                        e
                    )))?;
            } else {
                self.workspace_mgr
                    .set_active_agent(&channel, &peer_id, &args.agent_id)
                    .map_err(|e| crate::error::AlephError::other(format!(
                        "Failed to switch agent: {}",
                        e
                    )))?;
            }
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

        // Emit lifecycle event
        if let Some(ref bus) = self.event_bus {
            let _ = bus.publish_json(&AgentLifecycleEvent::Switched {
                agent_id: args.agent_id.clone(),
                channel: channel.clone(),
                peer_id: peer_id.clone(),
                previous_agent_id: previous.clone().unwrap_or_default(),
            });
        }

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
        let tool = AgentSwitchTool::new(registry, workspace_mgr, None);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_switch");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }
}
