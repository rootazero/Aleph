//! `AgentSwitchTool` — switch the active agent bound to the current channel.
//!
//! This is the namesake action of the "Agent Switching" feature: it lets the
//! LLM re-bind the conversation's channel to a different agent persona in
//! response to natural language (R8 — everything is a tool). It is pure I/O
//! wiring over existing infrastructure: validate the target against the runtime
//! [`AgentRegistry`], then persist the binding via [`AgentEnvStore`]. The
//! inbound router (`agent_resolver`) reads that binding on the next message, so
//! the switch takes effect immediately for subsequent turns.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_binding::bind_channel_agent;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for switching the active agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentSwitchArgs {
    /// ID of the agent to make active for the current channel.
    pub agent_id: String,
    /// Injected by registry — session channel (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __channel: String,
}

/// Output from switching the active agent.
#[derive(Debug, Clone, Serialize)]
pub struct AgentSwitchOutput {
    /// The agent now bound to the channel.
    pub agent_id: String,
    /// The channel whose active agent changed.
    pub channel: String,
    /// The agent previously bound to the channel, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_agent: Option<String>,
    /// Human-readable status message.
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that switches which agent is active for the current channel.
///
/// The target agent must already exist in the runtime registry (create it
/// first with `agent_create`). The binding is per-channel, so switching only
/// affects the conversation it is invoked from.
#[derive(Clone)]
pub struct AgentSwitchTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<AgentEnvStore>,
    event_bus: Option<Arc<GatewayEventBus>>,
}

impl AgentSwitchTool {
    #[must_use]
    pub const fn new(
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
impl AlephTool for AgentSwitchTool {
    const NAME: &'static str = "agent_switch";
    const DESCRIPTION: &'static str =
        "Switch the active agent for the current conversation's channel. Use this \
         when the user wants a different agent persona to handle the conversation \
         (e.g., switch to a trading or coding assistant). The target agent must \
         already exist — create it first with agent_create, then list with agent_list.";

    type Args = AgentSwitchArgs;
    type Output = AgentSwitchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, channel = %args.__channel, "Agent switch requested");

        // Validation, no-op detection, persistence, and the Bound lifecycle
        // event all live in the shared binding seam (`gateway::agent_binding`)
        // so this tool and the Panel `channels.set_agent` RPC cannot drift.
        let channel = args.__channel.trim().to_string();
        let outcome = bind_channel_agent(
            Some(&self.registry),
            &self.workspace_mgr,
            self.event_bus.as_deref(),
            &channel,
            &args.agent_id,
        )
        .await
        .map_err(|e| crate::error::AlephError::other(e.to_string()))?;

        let message = if outcome.no_op {
            format!(
                "Channel '{channel}' is already using agent '{}'.",
                args.agent_id
            )
        } else {
            match outcome.previous_agent.as_deref() {
                Some(prev) => format!(
                    "Switched channel '{channel}' from agent '{prev}' to '{}'.",
                    args.agent_id
                ),
                None => format!("Bound channel '{channel}' to agent '{}'.", args.agent_id),
            }
        };

        Ok(AgentSwitchOutput {
            agent_id: args.agent_id,
            channel,
            previous_agent: outcome.previous_agent,
            message,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_env::AgentEnvStoreConfig;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
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

    fn test_session_store() -> Arc<dyn crate::gateway::session_store::SessionStore> {
        let temp = tempdir().unwrap();
        let cfg = SessionManagerConfig {
            db_path: temp.keep().join("sessions.db"),
            ..Default::default()
        };
        Arc::new(SessionManager::new(cfg).expect("session manager"))
    }

    fn test_instance(agent_id: &str) -> AgentInstance {
        let root = tempdir().unwrap().keep();
        let config = AgentInstanceConfig {
            agent_id: agent_id.to_string(),
            workspace: root.join("workspace"),
            agent_dir: root.join("state"),
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };
        AgentInstance::new(config, test_session_store()).expect("instance")
    }

    async fn registry_with(agent_id: &str) -> Arc<AgentRegistry> {
        let registry = Arc::new(AgentRegistry::new());
        registry.register(test_instance(agent_id)).await;
        registry
    }

    #[test]
    fn test_switch_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let tool = AgentSwitchTool::new(registry, test_workspace_mgr(), None);
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_switch");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_switch_binds_existing_agent() {
        let registry = registry_with("trader").await;
        let wm = test_workspace_mgr();
        let tool = AgentSwitchTool::new(registry, Arc::clone(&wm), None);

        let out = tool
            .call(AgentSwitchArgs {
                agent_id: "trader".to_string(),
                __channel: "telegram".to_string(),
            })
            .await
            .expect("switch should succeed");

        assert_eq!(out.agent_id, "trader");
        assert_eq!(out.channel, "telegram");
        assert!(out.previous_agent.is_none());
        assert_eq!(
            wm.get_active_agent("telegram").unwrap().as_deref(),
            Some("trader")
        );
    }

    #[tokio::test]
    async fn test_switch_reports_previous_agent() {
        let registry = Arc::new(AgentRegistry::new());
        for id in ["trader", "coder"] {
            registry.register(test_instance(id)).await;
        }
        let wm = test_workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();
        let tool = AgentSwitchTool::new(registry, Arc::clone(&wm), None);

        let out = tool
            .call(AgentSwitchArgs {
                agent_id: "coder".to_string(),
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(out.previous_agent.as_deref(), Some("trader"));
        assert_eq!(out.agent_id, "coder");
    }

    #[tokio::test]
    async fn test_switch_rejects_unknown_agent() {
        let registry = registry_with("trader").await;
        let tool = AgentSwitchTool::new(registry, test_workspace_mgr(), None);
        let err = tool
            .call(AgentSwitchArgs {
                agent_id: "ghost".to_string(),
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_switch_rejects_empty_channel() {
        let registry = registry_with("trader").await;
        let tool = AgentSwitchTool::new(registry, test_workspace_mgr(), None);
        let err = tool
            .call(AgentSwitchArgs {
                agent_id: "trader".to_string(),
                __channel: String::new(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no active channel"));
    }

    #[tokio::test]
    async fn test_switch_is_idempotent() {
        let registry = registry_with("trader").await;
        let wm = test_workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();
        let tool = AgentSwitchTool::new(registry, Arc::clone(&wm), None);
        let out = tool
            .call(AgentSwitchArgs {
                agent_id: "trader".to_string(),
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap();
        assert!(out.message.contains("already using"));
    }
}
