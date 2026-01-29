//! Routing Configuration
//!
//! Configuration for message routing, session resolution, and permission policies.

use serde::{Deserialize, Serialize};

/// DM (Direct Message) scope - how to isolate DM sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DmScope {
    /// All DMs share the main session
    Main,
    /// Each peer gets their own session (cross-channel)
    #[default]
    PerPeer,
    /// Each peer per channel gets their own session
    PerChannelPeer,
}

/// Configuration for inbound message routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Default agent ID for routing
    #[serde(default = "default_agent_id")]
    pub default_agent: String,

    /// How to scope DM sessions
    #[serde(default)]
    pub dm_scope: DmScope,

    /// Whether to auto-start channels on gateway startup
    #[serde(default = "default_true")]
    pub auto_start_channels: bool,

    /// Pairing code expiry in seconds (0 = never)
    #[serde(default = "default_pairing_expiry")]
    pub pairing_code_expiry_secs: u64,
}

fn default_agent_id() -> String {
    "main".to_string()
}

fn default_true() -> bool {
    true
}

fn default_pairing_expiry() -> u64 {
    86400 // 24 hours
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            default_agent: default_agent_id(),
            dm_scope: DmScope::default(),
            auto_start_channels: true,
            pairing_code_expiry_secs: default_pairing_expiry(),
        }
    }
}

impl RoutingConfig {
    /// Create a new routing config with default agent
    pub fn new(default_agent: impl Into<String>) -> Self {
        Self {
            default_agent: default_agent.into(),
            ..Default::default()
        }
    }

    /// Set DM scope
    pub fn with_dm_scope(mut self, scope: DmScope) -> Self {
        self.dm_scope = scope;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RoutingConfig::default();
        assert_eq!(config.default_agent, "main");
        assert_eq!(config.dm_scope, DmScope::PerPeer);
        assert!(config.auto_start_channels);
    }

    #[test]
    fn test_dm_scope_serialization() {
        let json = serde_json::to_string(&DmScope::PerChannelPeer).unwrap();
        assert_eq!(json, "\"per-channel-peer\"");

        let parsed: DmScope = serde_json::from_str("\"main\"").unwrap();
        assert_eq!(parsed, DmScope::Main);
    }

    #[test]
    fn test_config_builder() {
        let config = RoutingConfig::new("custom-agent")
            .with_dm_scope(DmScope::Main);

        assert_eq!(config.default_agent, "custom-agent");
        assert_eq!(config.dm_scope, DmScope::Main);
    }
}
