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
    /// Resolve agent ID using multi-tier route bindings with workspace fallback.
    ///
    /// Priority: resolve_route(bindings) → workspace_manager → default_agent_id
    ///
    /// Returns (agent_id, Option<ResolvedRoute>). The ResolvedRoute carries the
    /// correctly computed session_key from the new routing system.
    pub(super) async fn resolve_agent_id_async(
        &self,
        msg: &InboundMessage,
    ) -> Option<(String, Option<crate::routing::resolve::ResolvedRoute>)> {
        // Tier 1: Try hierarchical route bindings (if configured)
        if !self.route_bindings.is_empty() {
            use crate::routing::{resolve_route, RouteInput, RoutePeer, RoutePeerKind};

            let peer = if msg.is_group {
                Some(RoutePeer {
                    kind: RoutePeerKind::Group,
                    id: msg.conversation_id.as_str().to_string(),
                })
            } else {
                Some(RoutePeer {
                    kind: RoutePeerKind::Dm,
                    id: msg.sender_id.as_str().to_string(),
                })
            };

            // Extract guild_id and team_id from raw metadata (set by channel implementations)
            let (guild_id, team_id) = msg
                .raw
                .as_ref()
                .map(|raw| {
                    let guild = raw
                        .get("guild_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let team = raw
                        .get("team_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (guild, team)
                })
                .unwrap_or((None, None));

            let input = RouteInput {
                channel: msg.channel_id.as_str().to_string(),
                account_id: None, // TODO: multi-account support
                peer,
                guild_id,
                team_id,
            };

            let resolved = resolve_route(
                &self.route_bindings,
                &self.route_session_config,
                &self.default_agent_id,
                &input,
            );

            debug!(
                "Route resolved: channel='{}' → agent='{}' (matched_by={:?})",
                msg.channel_id.as_str(),
                resolved.agent_id,
                resolved.matched_by,
            );
            return Some((resolved.agent_id.clone(), Some(resolved)));
        }

        // Tier 2: Fallback to workspace_manager (backward compat for zero-config)
        let channel = msg.channel_id.as_str();
        if let Some(ref manager) = self.workspace_manager {
            if let Ok(Some(agent_id)) = manager.get_active_agent(channel) {
                debug!(
                    "Channel '{}' bound to agent '{}' via workspace",
                    channel, agent_id
                );
                return Some((agent_id, None));
            }
        }

        // Tier 3: Default agent
        debug!(
            "Channel '{}' using default agent '{}'",
            msg.channel_id.as_str(),
            self.default_agent_id
        );
        Some((self.default_agent_id.clone(), None))
    }

    /// Build InboundContext from message with pre-resolved agent ID
    pub(super) async fn build_context_with_agent(
        &self,
        msg: &InboundMessage,
        agent_id: &str,
        resolved_route: Option<&crate::routing::resolve::ResolvedRoute>,
    ) -> InboundContext {
        let reply_route = ReplyRoute::new(msg.channel_id.clone(), msg.conversation_id.clone())
            .with_inbound_message_id(msg.id.clone());

        let base_key = if let Some(route) = resolved_route {
            route.session_key.clone()
        } else {
            // Fallback: use old-style session key construction
            self.resolve_session_key_with_agent(msg, agent_id)
        };

        // Resolve current epoch from session manager so messages route to
        // the latest session created by /new
        let session_key = if let Some(ref sm) = self.session_store {
            let base_pattern = base_key.base_key_pattern();
            match sm.get_current_epoch(&base_pattern).await {
                Ok(epoch) if epoch > 0 => base_key.with_epoch(epoch),
                _ => base_key,
            }
        } else {
            base_key
        };

        let sender_normalized = if msg.channel_id.as_str() == "imessage" {
            normalize_phone(msg.sender_id.as_str())
        } else {
            msg.sender_id.as_str().to_string()
        };

        InboundContext::new(msg.clone(), reply_route, session_key)
            .with_sender_normalized(sender_normalized)
    }

    /// Resolve SessionKey for a message with pre-resolved agent ID
    pub(super) fn resolve_session_key_with_agent(
        &self,
        msg: &InboundMessage,
        agent_id: &str,
    ) -> SessionKey {
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
                DmScope::PerPeer => {
                    SessionKey::peer(agent_id, format!("dm:{}", msg.sender_id.as_str()))
                }
                DmScope::PerChannelPeer => SessionKey::peer(
                    agent_id,
                    format!("{}:dm:{}", channel, msg.sender_id.as_str()),
                ),
            }
        }
    }
}
