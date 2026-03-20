//! Agent ID resolution and context building

use tracing::debug;

use crate::gateway::channel::InboundMessage;
use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
use crate::gateway::router::SessionKey;
use crate::gateway::routing_config::DmScope;

use super::InboundMessageRouter;

#[cfg(target_os = "macos")]
use crate::gateway::interfaces::imessage::normalize_phone;

#[cfg(not(target_os = "macos"))]
use super::normalize_phone;

impl InboundMessageRouter {
    /// Resolve agent ID from channel binding (single-tier).
    ///
    /// Looks up the 1:1 channel-agent binding. Returns None if unbound.
    pub(super) async fn resolve_agent_id_async(&self, channel: &str) -> Option<String> {
        if let Some(ref manager) = self.workspace_manager {
            if let Ok(Some(agent_id)) = manager.get_active_agent(channel) {
                debug!("Channel '{}' bound to agent '{}'", channel, agent_id);
                return Some(agent_id);
            }
        }
        debug!("Channel '{}' has no agent binding", channel);
        None
    }

    /// Build InboundContext from message with pre-resolved agent ID
    pub(super) fn build_context_with_agent(&self, msg: &InboundMessage, agent_id: &str) -> InboundContext {
        let reply_route = ReplyRoute::new(
            msg.channel_id.clone(),
            msg.conversation_id.clone(),
        );

        let session_key = self.resolve_session_key_with_agent(msg, agent_id);

        let sender_normalized = if msg.channel_id.as_str() == "imessage" {
            normalize_phone(msg.sender_id.as_str())
        } else {
            msg.sender_id.as_str().to_string()
        };

        InboundContext::new(msg.clone(), reply_route, session_key)
            .with_sender_normalized(sender_normalized)
    }

    /// Resolve SessionKey for a message with pre-resolved agent ID
    pub(super) fn resolve_session_key_with_agent(&self, msg: &InboundMessage, agent_id: &str) -> SessionKey {
        let channel = msg.channel_id.as_str();

        if msg.is_group {
            // Group message -> isolate by conversation_id
            SessionKey::peer(
                agent_id,
                format!("{}:group:{}", channel, msg.conversation_id.as_str()),
            )
        } else {
            // DM -> based on dm_scope
            match self.config.dm_scope {
                DmScope::Main => SessionKey::main(agent_id),
                DmScope::PerPeer => SessionKey::peer(
                    agent_id,
                    format!("dm:{}", msg.sender_id.as_str()),
                ),
                DmScope::PerChannelPeer => SessionKey::peer(
                    agent_id,
                    format!("{}:dm:{}", channel, msg.sender_id.as_str()),
                ),
            }
        }
    }
}
