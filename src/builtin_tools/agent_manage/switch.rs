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
use crate::gateway::agent_binding::{bind_channel_agent, BindError, BindOutcome};
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::error::AgentManageError;

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
    /// `true` when the channel was already bound to the requested agent and
    /// nothing was written (matches [`crate::gateway::agent_binding::BindOutcome::no_op`].
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_op: bool,
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
        "Switch the active agent for the current channel. Use when the user wants a \
         different persona (e.g., switch to a trading or coding assistant). The target \
         must already exist — create it first with agent_create.";

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
        .map_err(map_bind_error)?;

        let message = render_message(&channel, &args.agent_id, &outcome);
        Ok(AgentSwitchOutput {
            agent_id: args.agent_id,
            channel,
            previous_agent: outcome.previous_agent,
            no_op: outcome.no_op,
            message,
        })
    }
}

/// Translate `BindError` into the typed `AgentManageError` used by this tool.
///
/// The `UnknownAgent` arm enriches the message with the full `available` list
/// from the runtime registry — that list is what the LLM (and the next agent
/// it picks) actually consults, so the error must reflect it.
fn map_bind_error(err: BindError) -> crate::error::AlephError {
    match err {
        BindError::EmptyChannel => AgentManageError::NoActiveChannel.into(),
        BindError::UnknownAgent { available, .. } => AgentManageError::AgentNotFound {
            // Note: `agent_id` is not carried by `BindError::UnknownAgent` in
            // the seam — the seam puts it in `agent_id: String` but the
            // variant doesn't store it (a deliberate seam-level simplification).
            // To still produce a useful `Available agents:` line, we use the
            // available list directly. The Display impl in `BindError`
            // already covers the full text path.
            agent_id: String::new(),
            available,
        }
        .into(),
        BindError::Store(msg) => AgentManageError::Store(msg).into(),
    }
}

fn render_message(channel: &str, agent_id: &str, outcome: &BindOutcome) -> String {
    if outcome.no_op {
        format!("Channel '{channel}' is already using agent '{agent_id}'.")
    } else {
        match outcome.previous_agent.as_deref() {
            Some(prev) => format!(
                "Switched channel '{channel}' from agent '{prev}' to '{agent_id}'."
            ),
            None => format!("Bound channel '{channel}' to agent '{agent_id}'."),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::tools::AlephTool;

    #[test]
    fn test_switch_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentSwitchTool::new(registry, wm, None);
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_switch");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_switch_binds_existing_agent() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
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
        assert!(!out.no_op);
        assert_eq!(
            wm.get_active_agent("telegram").unwrap().as_deref(),
            Some("trader")
        );
    }

    #[tokio::test]
    async fn test_switch_reports_previous_agent() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance1, _sm, _t) = test_utils::instance("trader");
        registry.register(instance1).await;
        let (instance2, _sm2, _t2) = test_utils::instance("coder");
        registry.register(instance2).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
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
        assert!(!out.no_op);
    }

    #[tokio::test]
    async fn test_switch_rejects_unknown_agent_with_available_list() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentSwitchTool::new(registry, wm, None);

        let err = tool
            .call(AgentSwitchArgs {
                agent_id: "ghost".to_string(),
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"), "got: {msg}");
        assert!(msg.contains("trader"), "available list missing: {msg}");
    }

    #[tokio::test]
    async fn test_switch_rejects_empty_channel() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentSwitchTool::new(registry, wm, None);

        let err = tool
            .call(AgentSwitchArgs {
                agent_id: "trader".to_string(),
                __channel: String::new(),
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no active channel") || msg.contains("No active channel"), "got: {msg}");
    }

    #[tokio::test]
    async fn test_switch_is_idempotent() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();
        let tool = AgentSwitchTool::new(registry, Arc::clone(&wm), None);

        let out = tool
            .call(AgentSwitchArgs {
                agent_id: "trader".to_string(),
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap();
        assert!(out.no_op);
        assert!(out.message.contains("already using"));
    }
}

