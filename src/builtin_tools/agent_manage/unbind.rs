//! `AgentUnbindTool` — clear the explicit channel→agent binding for the
//! calling conversation.
//!
//! Companion to [`super::switch`]: `agent_switch` writes the binding; this
//! removes it. The inbound router then resolves the channel via the
//! registry default agent (or the `[routing]` table when configured) on
//! the next inbound message, exactly as if no explicit switch had ever
//! happened.
//!
//! Both tools route through the same [`crate::gateway::agent_binding`]
//! seam so they share ghost validation, no-op detection, and the
//! `Bound`/`Unbound` lifecycle event delivery — same contract as
//! `channels.set_agent` RPC + future "unbind" RPC.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_binding::unbind_channel_agent;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::agent_binding::BindError;
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::error::AgentManageError;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for clearing the current channel's active-agent binding.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentUnbindArgs {
    /// Injected by registry — session channel (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __channel: String,
}

/// Output from clearing the binding.
#[derive(Debug, Clone, Serialize)]
pub struct AgentUnbindOutput {
    /// The channel whose binding was cleared (or "no-op cleared").
    pub channel: String,
    /// The agent that was bound, if any. `None` means the channel was
    /// already unbound — a quiet no-op (the seam suppresses the
    /// `Unbound` event in that case too).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_agent: Option<String>,
    /// `true` when nothing was actually cleared (channel was already unbound).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_op: bool,
    /// Human-readable status message.
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that clears the per-channel active-agent binding.
///
/// Idempotent: unbinding an already-unbound channel is a quiet no-op, the
/// `Unbound` lifecycle event is suppressed, and `previous_agent` returns
/// `None`. Built-in agent guards live in [`super::delete`] — this tool only
/// touches the binding row, never the agent definition or workspace.
#[derive(Clone)]
pub struct AgentUnbindTool {
    workspace_mgr: Arc<AgentEnvStore>,
    event_bus: Option<Arc<GatewayEventBus>>,
    // Held for parity with `agent_switch` (so the constructor signature stays
    // symmetric across the toolset) and to make future "is this channel
    // actually using a built-in?" guards trivial to add.
    #[allow(dead_code)]
    registry: Arc<AgentRegistry>,
}

impl AgentUnbindTool {
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
impl AlephTool for AgentUnbindTool {
    const NAME: &'static str = "agent_unbind";
    const DESCRIPTION: &'static str =
        "Clear the active-agent binding for the current channel. The channel \
         then resolves via the default agent or the route table. Idempotent: \
         unbinding an unbound channel is a no-op.";

    type Args = AgentUnbindArgs;
    type Output = AgentUnbindOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(channel = %args.__channel, "Agent unbind requested");

        let channel = args.__channel.trim().to_string();
        let previous_agent = unbind_channel_agent(
            &self.workspace_mgr,
            self.event_bus.as_deref(),
            &channel,
        )
        .map_err(map_unbind_error)?;

        let no_op = previous_agent.is_none();
        let message = match &previous_agent {
            Some(prev) => format!("Cleared active-agent binding for channel '{channel}' (was '{prev}')."),
            None => format!("Channel '{channel}' was already unbound."),
        };

        Ok(AgentUnbindOutput {
            channel,
            previous_agent,
            no_op,
            message,
        })
    }
}

/// Translate `BindError` from `unbind_channel_agent` into the typed
/// `AgentManageError` family. Mirrors `switch.rs::map_bind_error` so both
/// surfaces report the same reason for the same root cause.
fn map_unbind_error(err: BindError) -> crate::error::AlephError {
    match err {
        BindError::EmptyChannel => AgentManageError::NoActiveChannel.into(),
        // `unbind_channel_agent` never produces `UnknownAgent` — there is no
        // target id to validate — but the match stays exhaustive so a future
        // seam addition can never accidentally widen the error surface
        // silently.
        BindError::UnknownAgent { available, .. } => AgentManageError::AgentNotFound {
            agent_id: String::new(),
            available,
        }
        .into(),
        BindError::Store(msg) => AgentManageError::Store(msg).into(),
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
    fn test_unbind_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentUnbindTool::new(registry, wm, None);
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_unbind");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn unbind_clears_existing_binding() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();

        let tool = AgentUnbindTool::new(registry, Arc::clone(&wm), None);
        let out = tool
            .call(AgentUnbindArgs {
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(out.previous_agent.as_deref(), Some("trader"));
        assert!(!out.no_op);
        assert!(wm.get_active_agent("telegram").unwrap().is_none());
        assert!(out.message.contains("Cleared"));
    }

    #[tokio::test]
    async fn unbind_unbound_channel_is_quiet_noop() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();

        let tool = AgentUnbindTool::new(registry, Arc::clone(&wm), None);
        let out = tool
            .call(AgentUnbindArgs {
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap();

        assert!(out.no_op);
        assert!(out.previous_agent.is_none());
        assert!(out.message.contains("already unbound"));
    }

    #[tokio::test]
    async fn unbind_rejects_empty_channel() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentUnbindTool::new(registry, wm, None);

        let err = tool
            .call(AgentUnbindArgs {
                __channel: String::new(),
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no active channel") || msg.contains("No active channel"), "got: {msg}");
    }
}