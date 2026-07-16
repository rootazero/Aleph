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
    /// Whether a *bound* agent id actually exists in the runtime registry.
    /// `true` when no registry is wired (nothing to check against — legacy /
    /// test contexts keep their old behaviour). A binding row can outlive its
    /// agent (Panel `agents.delete` removes only the TOML def; a failed
    /// create-persist plus a restart drops the instance) — routing to the
    /// ghost would brick the channel: every message errors `AgentNotFound`
    /// with no reply, forever.
    async fn bound_agent_exists(&self, agent_id: &str) -> bool {
        match &self.agent_registry {
            Some(reg) => reg.get(agent_id).await.is_some(),
            None => true,
        }
    }

    /// A workspace binding points at a vanished agent: warn, best-effort drop
    /// the stale row so the next message doesn't re-trip, and let the caller
    /// fall through to the default agent.
    fn clear_stale_binding(&self, channel: &str, agent_id: &str) {
        tracing::warn!(
            channel = %channel,
            agent_id = %agent_id,
            "channel is bound to an agent that no longer exists — clearing the stale binding and falling back to the default agent"
        );
        if let Some(ref manager) = self.workspace_manager {
            if let Err(e) = manager.clear_active_agent(channel) {
                tracing::warn!(channel = %channel, error = %e, "failed to clear stale agent binding");
            }
        }
    }

    /// Resolve agent ID using multi-tier route bindings with workspace fallback.
    ///
    /// Priority: `resolve_route(bindings)` → `workspace_manager` → `default_agent_id`
    ///
    /// Returns (`agent_id`, Option<ResolvedRoute>). The `ResolvedRoute` carries the
    /// correctly computed `session_key` from the new routing system.
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
            let (guild_id, team_id) = msg.raw.as_ref().map_or((None, None), |raw| {
                let guild = raw
                    .get("guild_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let team = raw
                    .get("team_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (guild, team)
            });

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

            // An explicit per-channel binding (set by `agent_switch` / Panel) is a
            // deliberate runtime override. It must win when NO *specific* route
            // binding governs this conversation — otherwise the namesake
            // `agent_switch` action is a silent no-op whenever any route_bindings
            // exist, because Tier 1 returns before the Tier 2 workspace binding is
            // ever consulted. Specific bindings (Peer/Guild/Team/Account/Channel)
            // still win, preserving carefully-scoped routing config.
            if resolved.matched_by == crate::routing::resolve::MatchedBy::Default {
                let channel = msg.channel_id.as_str();
                if let Some(ref manager) = self.workspace_manager {
                    if let Ok(Some(agent_id)) = manager.get_active_agent(channel) {
                        if agent_id != resolved.agent_id {
                            // Existence gate: a stale binding to a deleted
                            // agent must not override the (existing) default
                            // route — that would brick the channel.
                            if !self.bound_agent_exists(&agent_id).await {
                                self.clear_stale_binding(channel, &agent_id);
                            } else {
                                debug!(
                                    "Channel '{}' override → agent '{}' (explicit switch beats default route)",
                                    channel, agent_id
                                );
                                // Return None for the route so the context builder
                                // rebuilds the session key for the override agent
                                // (the route's key was computed for the default agent).
                                return Some((agent_id, None));
                            }
                        }
                    }
                }
            }

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
                // Existence gate: fall through to the default agent instead of
                // routing every message on this channel into AgentNotFound.
                if self.bound_agent_exists(&agent_id).await {
                    debug!(
                        "Channel '{}' bound to agent '{}' via workspace",
                        channel, agent_id
                    );
                    return Some((agent_id, None));
                }
                self.clear_stale_binding(channel, &agent_id);
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

    /// Build `InboundContext` from message with pre-resolved agent ID
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

    /// Resolve `SessionKey` for a message with pre-resolved agent ID
    pub(super) fn resolve_session_key_with_agent(
        &self,
        msg: &InboundMessage,
        agent_id: &str,
    ) -> SessionKey {
        let channel = msg.channel_id.as_str();

        if msg.is_group {
            // Group message -> isolate by conversation_id. Use the proper
            // `Group` variant (matching `resolve_route`'s bound-route path)
            // so a group chat is not mistyped as a DM and the zero-config
            // fallback key agrees with the configured-binding key for the
            // same conversation.
            SessionKey::group(
                agent_id,
                channel,
                crate::routing::session_key::PeerKind::Group,
                msg.conversation_id.as_str(),
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
