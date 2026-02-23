//! Link configuration types for the Social Connectivity plugin system.
//!
//! A Link is a configured instance of a Bridge — it binds a bridge plugin
//! to a specific account/bot and defines routing policies.
//! Link configurations are parsed from `link.yaml` files.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::gateway::bridge::BridgeId;

// ---------------------------------------------------------------------------
// LinkId
// ---------------------------------------------------------------------------

/// Unique identifier for a link instance (e.g. "my-telegram-bot").
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinkId(pub String);

impl LinkId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LinkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// LinkConfig
// ---------------------------------------------------------------------------

/// A link configuration parsed from `link.yaml`.
///
/// Links bind a bridge to a specific account and define message routing
/// policies (which agent handles DMs vs group messages, allowlists, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkConfig {
    /// Manifest schema version (e.g. "1")
    pub spec_version: String,

    /// Unique link identifier
    pub id: LinkId,

    /// Which bridge plugin this link uses
    pub bridge: BridgeId,

    /// Human-readable name for this link
    pub name: String,

    /// Whether this link is active
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Bridge-specific settings (validated against bridge's settings_schema)
    #[serde(default)]
    pub settings: serde_json::Value,

    /// Message routing configuration
    #[serde(default)]
    pub routing: LinkRoutingConfig,
}

fn default_enabled() -> bool {
    true
}

// ---------------------------------------------------------------------------
// LinkRoutingConfig
// ---------------------------------------------------------------------------

/// Routing policy for messages arriving on a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRoutingConfig {
    /// Which agent handles messages from this link (default: "main")
    #[serde(default = "default_agent")]
    pub agent: String,

    /// Policy for direct messages
    #[serde(default)]
    pub dm_policy: DmPolicyConfig,

    /// Policy for group messages
    #[serde(default)]
    pub group_policy: GroupPolicyConfig,
}

impl Default for LinkRoutingConfig {
    fn default() -> Self {
        Self {
            agent: default_agent(),
            dm_policy: DmPolicyConfig::default(),
            group_policy: GroupPolicyConfig::default(),
        }
    }
}

fn default_agent() -> String {
    "main".to_string()
}

// ---------------------------------------------------------------------------
// DmPolicyConfig
// ---------------------------------------------------------------------------

/// Policy controlling who can send direct messages through a link.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicyConfig {
    /// Accept DMs from anyone
    Open,
    /// Require a pairing handshake before accepting DMs (secure default)
    #[default]
    Pairing,
    /// Only accept DMs from users in an explicit allowlist
    Allowlist,
    /// Reject all DMs
    Disabled,
}

// ---------------------------------------------------------------------------
// GroupPolicyConfig
// ---------------------------------------------------------------------------

/// Policy controlling which groups the link participates in.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GroupPolicyConfig {
    /// Accept messages from any group the bot is added to
    Open,
    /// Only accept messages from groups in an explicit allowlist
    Allowlist,
    /// Ignore all group messages (secure default)
    #[default]
    Disabled,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_id_basics() {
        let id = LinkId::new("my-telegram");
        assert_eq!(id.as_str(), "my-telegram");
        assert_eq!(format!("{}", id), "my-telegram");
        assert_eq!(id, LinkId::new("my-telegram"));
    }

    #[test]
    fn test_deserialize_full_link() {
        let yaml = r#"
spec_version: "1"
id: my-telegram-bot
bridge: telegram
name: My Telegram Bot
enabled: true
settings:
  bot_token: "${TELEGRAM_BOT_TOKEN}"
  webhook_url: "https://example.com/webhook"
routing:
  agent: assistant
  dm_policy: pairing
  group_policy: allowlist
"#;
        let cfg: LinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.spec_version, "1");
        assert_eq!(cfg.id.as_str(), "my-telegram-bot");
        assert_eq!(cfg.bridge.as_str(), "telegram");
        assert_eq!(cfg.name, "My Telegram Bot");
        assert!(cfg.enabled);
        assert_eq!(cfg.routing.agent, "assistant");
        assert_eq!(cfg.routing.dm_policy, DmPolicyConfig::Pairing);
        assert_eq!(cfg.routing.group_policy, GroupPolicyConfig::Allowlist);
    }

    #[test]
    fn test_deserialize_minimal_link() {
        let yaml = r#"
spec_version: "1"
id: bare
bridge: signal
name: Bare Link
"#;
        let cfg: LinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.id.as_str(), "bare");
        assert_eq!(cfg.bridge.as_str(), "signal");
        assert!(cfg.enabled); // default true
        assert_eq!(cfg.routing.agent, "main"); // default "main"
        assert_eq!(cfg.routing.dm_policy, DmPolicyConfig::Pairing); // secure default
        assert_eq!(cfg.routing.group_policy, GroupPolicyConfig::Disabled); // secure default
        assert_eq!(cfg.settings, serde_json::Value::Null); // default
    }

    #[test]
    fn test_env_var_syntax_preserved() {
        let yaml = r#"
spec_version: "1"
id: env-test
bridge: telegram
name: Env Test
settings:
  bot_token: "${TELEGRAM_BOT_TOKEN}"
  api_key: "${MY_API_KEY}"
"#;
        let cfg: LinkConfig = serde_yaml::from_str(yaml).unwrap();
        // Environment variable syntax is preserved as literal strings
        assert_eq!(
            cfg.settings["bot_token"].as_str().unwrap(),
            "${TELEGRAM_BOT_TOKEN}"
        );
        assert_eq!(
            cfg.settings["api_key"].as_str().unwrap(),
            "${MY_API_KEY}"
        );
    }

    #[test]
    fn test_dm_policy_variants() {
        for (yaml_val, expected) in [
            ("open", DmPolicyConfig::Open),
            ("pairing", DmPolicyConfig::Pairing),
            ("allowlist", DmPolicyConfig::Allowlist),
            ("disabled", DmPolicyConfig::Disabled),
        ] {
            let yaml = format!(
                r#"
spec_version: "1"
id: test
bridge: test
name: Test
routing:
  dm_policy: {yaml_val}
"#
            );
            let cfg: LinkConfig = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(cfg.routing.dm_policy, expected, "dm_policy: {yaml_val}");
        }
    }

    #[test]
    fn test_group_policy_variants() {
        for (yaml_val, expected) in [
            ("open", GroupPolicyConfig::Open),
            ("allowlist", GroupPolicyConfig::Allowlist),
            ("disabled", GroupPolicyConfig::Disabled),
        ] {
            let yaml = format!(
                r#"
spec_version: "1"
id: test
bridge: test
name: Test
routing:
  group_policy: {yaml_val}
"#
            );
            let cfg: LinkConfig = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(cfg.routing.group_policy, expected, "group_policy: {yaml_val}");
        }
    }
}
