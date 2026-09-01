//! Agent ID resolution and context building

use tracing::{debug, warn};

use crate::gateway::channel::InboundMessage;
use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
use crate::gateway::router::SessionKey;
use crate::gateway::routing_config::DmScope;
use crate::routing::identity_links::resolve_linked_peer_id;

use super::InboundMessageRouter;

#[cfg(target_os = "macos")]
use crate::gateway::interfaces::imessage::normalize_phone;

#[cfg(not(target_os = "macos"))]
use super::normalize_phone;

impl InboundMessageRouter {
    /// Read the channel's explicit agent binding, validated against the
    /// runtime registry.
    ///
    /// A stale binding — the bound agent's TOML definition removed while the
    /// binding row survived a restart, or a crash between delete steps —
    /// would otherwise brick the channel: every inbound message resolves to
    /// a ghost and fails `AgentNotFound`, and the user can no longer reach
    /// the LLM to run `agent_switch` and fix it. Fail-soft on the hot path
    /// (control-plane surfaces stay fail-loud): warn and fall back to
    /// default routing. The binding row is deliberately NOT cleared — if the
    /// agent is re-registered (config restored, next boot) the user's
    /// explicit switch comes back to life instead of being destroyed.
    async fn validated_channel_override(&self, channel: &str) -> Option<String> {
        let manager = self.workspace_manager.as_ref()?;
        let agent_id = manager.get_active_agent(channel).ok().flatten()?;
        if let Some(ref registry) = self.agent_registry {
            if !registry.contains(&agent_id).await {
                warn!(
                    channel = %channel,
                    agent_id = %agent_id,
                    "Channel is bound to an agent missing from the runtime registry; \
                     falling back to default routing (binding kept for recovery)"
                );
                return None;
            }
        }
        Some(agent_id)
    }

    /// Whether a *bound* agent id actually exists in the runtime registry.
    /// `true` when no registry is wired (nothing to check against — legacy /
    /// test contexts keep their old behaviour). A binding row can outlive its
    /// agent (Panel `agents.delete` removes only the TOML def; a failed
    /// create-persist plus a restart drops the instance) — routing to the
    /// ghost would brick the channel: every message errors `AgentNotFound`
    /// with no reply, forever.
    async fn bound_agent_exists(&self, agent_id: &str) -> bool {
        match &self.agent_registry {
            Some(reg) => reg.contains(agent_id).await,
            None => true,
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

            // The two runtime facts that can override a config answer — an
            // explicit `agent_switch` binding, and whether a specifically-bound
            // agent still exists — are composed by `routing::overlay_route`, the
            // same function the `gateway_route` tool calls. Keeping the
            // precedence in one place is what stops the tool from confidently
            // reporting an agent the gateway would never dispatch to.
            let channel = msg.channel_id.as_str();
            let channel_override = self.validated_channel_override(channel).await;
            let overlaid = crate::routing::overlay_route(
                &resolved.agent_id,
                resolved.matched_by,
                &crate::routing::RuntimeOverlay {
                    channel_override: channel_override.as_deref(),
                    bound_agent_exists: self.bound_agent_exists(&resolved.agent_id).await,
                },
            );
            match overlaid.source {
                crate::routing::OverlaySource::Binding(_) => {
                    debug!(
                        "Route resolved: channel='{}' → agent='{}' (matched_by={:?})",
                        channel, resolved.agent_id, resolved.matched_by,
                    );
                    return Some((resolved.agent_id.clone(), Some(resolved)));
                }
                crate::routing::OverlaySource::ChannelOverride => {
                    // `None` for the route so the context builder rebuilds the
                    // session key for the override agent (the route's key was
                    // computed for the config-resolved agent).
                    if let Some(agent_id) = overlaid.agent_id {
                        debug!(
                            "Channel '{channel}' override → agent '{agent_id}' \
                             (explicit switch beats default route)"
                        );
                        return Some((agent_id, None));
                    }
                }
                crate::routing::OverlaySource::BindingAgentMissing => {
                    // `[[bindings]]` is config-TOML snapshotted at boot and
                    // `agents.delete` cannot touch it, so a binding can outlive
                    // its agent. Routing to the ghost would brick every
                    // conversation it governs — every message `AgentNotFound`,
                    // forever, with no restart cure. Fall through to Tier 2/3
                    // instead; recreating the agent restores the route on the
                    // next message.
                    tracing::warn!(
                        channel = %channel,
                        agent_id = %resolved.agent_id,
                        matched_by = ?resolved.matched_by,
                        "route binding targets an agent that no longer exists — falling back to workspace binding / default agent; fix [[bindings]] in config"
                    );
                }
            }
        }

        // Tier 2: Fallback to workspace_manager (backward compat for zero-config)
        let channel = msg.channel_id.as_str();
        if let Some(agent_id) = self.validated_channel_override(channel).await {
            debug!(
                "Channel '{}' bound to agent '{}' via workspace",
                channel, agent_id
            );
            return Some((agent_id, None));
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

        // A binding's `MatchRule.workspace` pins the run's workspace (most
        // specific routing rule) — but only when it is an absolute, existing
        // directory, mirroring `ChannelConfig::resolved_default_workspace`.
        // A relative path or missing dir falls back to the channel default /
        // agent default rather than handing the engine a dir it cannot chdir
        // into.
        let binding_workspace = resolved_route
            .and_then(|r| r.workspace.as_deref())
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir())
            .or_else(|| {
                if let Some(w) = resolved_route.and_then(|r| r.workspace.as_deref()) {
                    tracing::warn!(
                        workspace = w,
                        "route binding workspace is not an existing absolute directory; \
                         falling back to channel/agent default workspace"
                    );
                }
                None
            });

        InboundContext::new(msg.clone(), reply_route, session_key)
            .with_sender_normalized(sender_normalized)
            .with_workspace(binding_workspace)
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
            // DM -> use the same constructor as the bound-route path
            // (`SessionKey::dm`), so the zero-config fallback key agrees with
            // `resolve_route`'s key for the same conversation. The previous
            // `SessionKey::peer(agent, "dm:{sender}")` idiom sanitized the
            // `dm:` prefix into the peer id, yielding `peer:dm-{sender}` and
            // splitting history between the two routing paths.
            //
            // Apply identity_links resolution on this zero-config fallback so
            // a deployment configured with `[session] identity_links` and no
            // `[[bindings]]` still collapses the same person across channels
            // into one session, mirroring what `resolve_route` does on the
            // bindings path (`src/routing/resolve.rs::build_session_key`).
            let peer_id = resolve_linked_peer_id(
                &self.route_session_config.identity_links,
                channel,
                msg.sender_id.as_str(),
            )
            .unwrap_or_else(|| msg.sender_id.as_str().to_string());
            SessionKey::dm(
                agent_id,
                channel,
                peer_id.as_str(),
                match self.config.dm_scope {
                    DmScope::Main => crate::routing::session_key::DmScope::Main,
                    DmScope::PerPeer => crate::routing::session_key::DmScope::PerPeer,
                    DmScope::PerChannelPeer => crate::routing::session_key::DmScope::PerChannelPeer,
                },
            )
        }
    }
}
