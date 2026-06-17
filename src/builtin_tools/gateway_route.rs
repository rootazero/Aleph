use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::routing::{
    resolve_route, MatchedBy, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind, SessionConfig,
};
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

#[derive(Debug, Clone)]
pub struct GatewayRouteTool {
    bindings: Vec<crate::routing::RouteBinding>,
    session_config: SessionConfig,
    default_agent: String,
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
        }
    }

    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(Vec::new(), SessionConfig::default(), "main".to_string())
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
        was made (peer/guild/team/account/channel/default). Use this when agents need \
        to self-route or coordinate cross-channel communication. This is a deterministic, \
        config-driven channel→agent lookup — it does NOT classify the message's intent \
        (that is the model's job).";

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
            team_id: args.team_id.clone(),
        };

        let resolved: ResolvedRoute = resolve_route(
            &self.bindings,
            &self.session_config,
            &self.default_agent,
            &input,
        );

        let matched_by_str = match resolved.matched_by {
            MatchedBy::Peer => "peer",
            MatchedBy::Guild => "guild",
            MatchedBy::Team => "team",
            MatchedBy::Account => "account",
            MatchedBy::Channel => "channel",
            MatchedBy::Default => "default",
        };

        let output = GatewayRouteOutput {
            agent_id: resolved.agent_id.clone(),
            session_key: resolved.session_key.to_key_string(),
            matched_by: matched_by_str.to_string(),
            workspace: resolved.workspace,
            details: Some(GatewayRouteDetails {
                channel: resolved.channel,
                account_id: resolved.account_id,
                session_key: resolved.session_key.to_key_string(),
                main_session_key: resolved.main_session_key.to_key_string(),
            }),
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
