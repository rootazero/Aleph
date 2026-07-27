use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::routing::{
    resolve_route, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind, SessionConfig,
};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GatewayRouteArgs {
    pub channel: String,
    #[serde(default)]
    pub peer_id: Option<String>,
    #[serde(default)]
    pub peer_kind: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayRouteOutput {
    pub agent_id: String,
    pub session_key: String,
    pub matched_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<GatewayRouteDetails>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayRouteDetails {
    pub channel: String,
    pub account_id: String,
    pub session_key: String,
    pub main_session_key: String,
}

#[derive(Clone)]
pub struct GatewayRouteTool {
    bindings: Vec<crate::routing::RouteBinding>,
    session_config: SessionConfig,
    default_agent: String,
    /// Per-channel `agent_switch` / Panel bindings — the runtime overlay on top
    /// of the config `[[bindings]]` table. Without it the tool answers from
    /// config alone and reports the wrong agent on any channel that was switched
    /// at runtime (which is *every* zero-config deployment, where the config
    /// table is empty and the switch is the only thing routing anything).
    channel_bindings: Option<Arc<AgentEnvStore>>,
    /// Runtime agent registry, so a binding that outlived its agent is reported
    /// as the fall-through the gateway actually performs rather than as a ghost.
    agent_registry: Option<Arc<AgentRegistry>>,
}

impl GatewayRouteTool {
    #[must_use]
    pub fn new(
        bindings: Vec<crate::routing::RouteBinding>,
        session_config: SessionConfig,
        default_agent: String,
    ) -> Self {
        Self {
            bindings,
            session_config,
            default_agent,
            channel_bindings: None,
            agent_registry: None,
        }
    }

    /// Attach the runtime stores the gateway consults on top of config.
    ///
    /// Both are optional so the tool still answers (from config alone) in test
    /// and non-gateway contexts — but in production both must be wired, or the
    /// tool is back to describing a routing table nobody uses.
    #[must_use]
    pub fn with_runtime_bindings(
        mut self,
        channel_bindings: Option<Arc<AgentEnvStore>>,
        agent_registry: Option<Arc<AgentRegistry>>,
    ) -> Self {
        self.channel_bindings = channel_bindings;
        self.agent_registry = agent_registry;
        self
    }

    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(Vec::new(), SessionConfig::default(), "main".to_string())
    }

    /// The channel's explicit runtime binding, validated to still exist —
    /// the same two-step the inbound router performs before honouring one.
    async fn validated_channel_override(&self, channel: &str) -> Option<String> {
        let agent_id = self
            .channel_bindings
            .as_ref()?
            .get_active_agent(channel)
            .ok()
            .flatten()?;
        if !self.agent_exists(&agent_id).await {
            return None;
        }
        Some(agent_id)
    }

    /// Whether `agent_id` exists in the runtime registry. `true` when no
    /// registry is wired — the same trust-config fallback the router uses.
    async fn agent_exists(&self, agent_id: &str) -> bool {
        match &self.agent_registry {
            Some(reg) => reg.contains(agent_id).await,
            None => true,
        }
    }
}

impl Default for GatewayRouteTool {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[async_trait]
impl AlephTool for GatewayRouteTool {
    const NAME: &'static str = "gateway_route";
    const DESCRIPTION: &'static str =
        "Query Aleph's routing engine to determine which agent and session a message \
        would be routed to. Returns the target agent, session key, and how the match \
        was made: peer/guild/team/account/channel/default from the configured \
        `[routing]` bindings, `channel_override` when an `agent_switch` binding beats \
        them, or `binding_agent_missing` when a binding names a deleted agent and the \
        route falls through. Use this when agents need to self-route or coordinate \
        cross-channel communication. This is a deterministic, config-driven \
        channel→agent lookup — it does NOT classify the message's intent (that is the \
        model's job).";

    type Args = GatewayRouteArgs;
    type Output = GatewayRouteOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(tool = "gateway_route", channel = %args.channel, "querying routing engine");

        let peer = match args.peer_id.as_ref() {
            Some(id) => {
                // Only treat an absent peer_kind as the DM default. An
                // unrecognised value must be rejected, not silently coerced to
                // DM — otherwise a typo ("groupp") yields a DM session key and a
                // different route than the caller intended.
                let kind = match args.peer_kind.as_deref() {
                    None | Some("dm") => RoutePeerKind::Dm,
                    Some("group") => RoutePeerKind::Group,
                    Some("channel") => RoutePeerKind::Channel,
                    Some(other) => {
                        return Err(crate::error::AlephError::tool(format!(
                            "Invalid peer_kind '{other}'. Expected one of: dm, group, channel."
                        )));
                    }
                };
                Some(RoutePeer {
                    kind,
                    id: id.clone(),
                })
            }
            None => None,
        };

        let input = RouteInput {
            channel: args.channel.clone(),
            account_id: args.account_id.clone(),
            peer,
            guild_id: args.guild_id.clone(),
            team_id: args.team_id,
        };

        let resolved: ResolvedRoute = resolve_route(
            &self.bindings,
            &self.session_config,
            &self.default_agent,
            &input,
        );

        // Config answers only half the question. Apply the same runtime overlay
        // the gateway does — the channel's `agent_switch` binding, and whether a
        // specifically-bound agent still exists — through the shared decision.
        let channel_override = self.validated_channel_override(&resolved.channel).await;
        let overlaid = crate::routing::overlay_route(
            &resolved.agent_id,
            resolved.matched_by,
            &crate::routing::RuntimeOverlay {
                channel_override: channel_override.as_deref(),
                bound_agent_exists: self.agent_exists(&resolved.agent_id).await,
            },
        );
        // A dropped ghost binding falls through to the deployment's default
        // agent, exactly as the gateway's Tier 3 does.
        let agent_id = overlaid
            .agent_id
            .unwrap_or_else(|| crate::routing::normalize_agent_id(&self.default_agent));
        // The session key belongs to whoever actually serves the message. When
        // the overlay changed the agent, the key `resolve_route` computed (for
        // the config agent) addresses a different conversation.
        let (session_key, main_session_key) = if agent_id == resolved.agent_id {
            (resolved.session_key, resolved.main_session_key)
        } else {
            crate::routing::resolve::session_keys_for(
                &agent_id,
                &resolved.channel,
                input.peer.as_ref(),
                &self.session_config,
            )
        };

        let output = GatewayRouteOutput {
            session_key: session_key.to_key_string(),
            matched_by: overlaid.source.as_str().to_string(),
            workspace: resolved.workspace,
            details: Some(GatewayRouteDetails {
                channel: resolved.channel,
                account_id: resolved.account_id,
                session_key: session_key.to_key_string(),
                main_session_key: main_session_key.to_key_string(),
            }),
            agent_id,
        };

        info!(
            tool = "gateway_route",
            agent_id = %output.agent_id,
            matched_by = %output.matched_by,
            "routing query complete"
        );

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_route() {
        let tool = GatewayRouteTool::with_defaults();
        let args = GatewayRouteArgs {
            channel: "telegram".to_string(),
            peer_id: None,
            peer_kind: None,
            guild_id: None,
            team_id: None,
            account_id: None,
        };

        let result = tool.call(args).await.unwrap();
        assert_eq!(result.agent_id, "main");
        assert_eq!(result.matched_by, "default");
    }

    #[tokio::test]
    async fn test_configured_bindings_are_honored() {
        // Regression: the tool must reflect the real `[routing]` bindings it is
        // constructed with (the constructor now snapshots them from live config),
        // not silently answer "default" the way `::default()` did. A channel
        // binding to a non-default agent must surface as a `channel` match.
        use crate::routing::{MatchRule, RouteBinding};

        let bindings = vec![RouteBinding {
            agent_id: "telegram-agent".to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
                account_id: Some("*".to_string()),
                ..Default::default()
            },
        }];
        let tool = GatewayRouteTool::new(bindings, SessionConfig::default(), "main".to_string());
        let args = GatewayRouteArgs {
            channel: "telegram".to_string(),
            peer_id: None,
            peer_kind: None,
            guild_id: None,
            team_id: None,
            account_id: None,
        };

        let result = tool.call(args).await.unwrap();
        assert_eq!(result.agent_id, "telegram-agent");
        assert_eq!(result.matched_by, "channel");
    }

    #[tokio::test]
    async fn test_invalid_peer_kind_rejected() {
        let tool = GatewayRouteTool::with_defaults();
        let args = GatewayRouteArgs {
            channel: "telegram".to_string(),
            peer_id: Some("user123".to_string()),
            peer_kind: Some("groupp".to_string()),
            guild_id: None,
            team_id: None,
            account_id: None,
        };

        // A typo'd peer_kind must error, not silently coerce to DM.
        assert!(tool.call(args).await.is_err());
    }
}
