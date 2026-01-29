//! Configuration structures for routing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::session_key::DmScope;

/// Session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// DM isolation strategy
    #[serde(default)]
    pub dm_scope: DmScope,

    /// Cross-channel identity links: canonical_name -> [channel:id, ...]
    #[serde(default)]
    pub identity_links: HashMap<String, Vec<String>>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: DmScope::PerPeer,
            identity_links: HashMap::new(),
        }
    }
}

/// Route binding configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteBinding {
    pub agent_id: String,
    #[serde(rename = "match")]
    pub match_rule: MatchRule,
}

/// Match rule for route binding
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatchRule {
    /// Channel to match (telegram, discord, slack, ...)
    pub channel: Option<String>,
    /// API account ID (supports "*" wildcard)
    pub account_id: Option<String>,
    /// Peer match (specific user/group)
    pub peer: Option<PeerMatchConfig>,
    /// Discord guild ID
    pub guild_id: Option<String>,
    /// Slack team ID
    pub team_id: Option<String>,
}

/// Peer match configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMatchConfig {
    pub kind: String,
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_config_default() {
        let cfg = SessionConfig::default();
        assert_eq!(cfg.dm_scope, DmScope::PerPeer);
        assert!(cfg.identity_links.is_empty());
    }

    #[test]
    fn test_session_config_deserialize() {
        let toml_str = r#"
            dm_scope = "per-channel-peer"

            [identity_links]
            john = ["telegram:123", "discord:456"]
        "#;
        let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.dm_scope, DmScope::PerChannelPeer);
        assert_eq!(cfg.identity_links["john"].len(), 2);
    }

    #[test]
    fn test_route_binding_deserialize() {
        let toml_str = r#"
            agent_id = "work"
            [match]
            channel = "slack"
            team_id = "T12345"
        "#;
        let binding: RouteBinding = toml::from_str(toml_str).unwrap();
        assert_eq!(binding.agent_id, "work");
        assert_eq!(binding.match_rule.channel.as_deref(), Some("slack"));
        assert_eq!(binding.match_rule.team_id.as_deref(), Some("T12345"));
    }
}
