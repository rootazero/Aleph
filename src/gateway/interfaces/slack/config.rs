//! Slack Channel Configuration
//!
//! Configuration types for the Slack Bot integration using Socket Mode + REST API.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_directory_ttl() -> u64 {
    3600
}

/// Slack channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    /// App-level token for Socket Mode (xapp-...)
    pub app_token: String,

    /// Bot token for REST API (xoxb-...)
    pub bot_token: String,

    /// Allowed channel IDs (empty = allow all)
    #[serde(default)]
    pub allowed_channels: Vec<String>,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,

    /// Allow direct messages
    #[serde(default = "default_true")]
    pub dm_allowed: bool,

    /// Enable message reactions (add/remove emoji)
    #[serde(default = "default_true")]
    pub enable_reactions: bool,

    /// Enable message editing
    #[serde(default = "default_true")]
    pub enable_editing: bool,

    /// Enable message deletion (dangerous - defaults to false)
    #[serde(default)]
    pub enable_deletion: bool,

    /// Debounce window (ms) for coalescing rapid messages from same sender
    /// Set to 0 to disable debouncing
    #[serde(default)]
    pub debounce_ms: u64,

    /// Allowed user IDs (empty = allow all users in allowed channels)
    #[serde(default)]
    pub user_allowlist: Vec<String>,

    /// Resolve user IDs to display names via users.info API
    /// Caches results with TTL to avoid rate limiting
    #[serde(default)]
    pub resolve_user_names: bool,

    /// Directory cache TTL in seconds (default: 3600)
    #[serde(default = "default_directory_ttl")]
    pub directory_ttl_secs: u64,
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            app_token: String::new(),
            bot_token: String::new(),
            allowed_channels: Vec::new(),
            send_typing: true,
            dm_allowed: true,
            enable_reactions: true,
            enable_editing: true,
            enable_deletion: false,
            debounce_ms: 700,
            user_allowlist: Vec::new(),
            resolve_user_names: false,
            directory_ttl_secs: 3600,
        }
    }
}

impl SlackConfig {
    /// Create config from environment variables
    pub fn from_env() -> Option<Self> {
        let app_token = std::env::var("SLACK_APP_TOKEN").ok()?;
        let bot_token = std::env::var("SLACK_BOT_TOKEN").ok()?;
        Some(Self {
            app_token,
            bot_token,
            ..Default::default()
        })
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.app_token.is_empty() {
            return Err("app_token is required".to_string());
        }
        if !self.app_token.starts_with("xapp-") {
            return Err(
                "app_token must start with 'xapp-' (Socket Mode app-level token)".to_string(),
            );
        }
        if self.bot_token.is_empty() {
            return Err("bot_token is required".to_string());
        }
        if !self.bot_token.starts_with("xoxb-") {
            return Err("bot_token must start with 'xoxb-' (Bot User OAuth Token)".to_string());
        }
        Ok(())
    }

    /// Check if a channel ID is allowed
    pub fn is_channel_allowed(&self, channel_id: &str) -> bool {
        if self.allowed_channels.is_empty() {
            true
        } else {
            self.allowed_channels.contains(&channel_id.to_string())
        }
    }

    /// Check if a user ID is allowed
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.user_allowlist.is_empty() {
            true
        } else {
            self.user_allowlist.contains(&user_id.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SlackConfig::default();
        assert!(config.app_token.is_empty());
        assert!(config.bot_token.is_empty());
        assert!(config.allowed_channels.is_empty());
        assert!(config.send_typing);
        assert!(config.dm_allowed);
        assert!(config.enable_reactions);
        assert!(config.enable_editing);
        assert!(!config.enable_deletion);
        assert_eq!(config.debounce_ms, 700);
    }

    #[test]
    fn test_validate_empty_tokens() {
        let config = SlackConfig::default();
        assert!(config.validate().is_err());
        assert_eq!(config.validate().unwrap_err(), "app_token is required");
    }

    #[test]
    fn test_validate_invalid_app_token_prefix() {
        let config = SlackConfig {
            app_token: "invalid-token".to_string(),
            bot_token: "xoxb-test-token".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("xapp-"),
            "Error should mention xapp- prefix: {}",
            err
        );
    }

    #[test]
    fn test_validate_empty_bot_token() {
        let config = SlackConfig {
            app_token: "xapp-valid-token".to_string(),
            bot_token: String::new(),
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), "bot_token is required");
    }

    #[test]
    fn test_validate_invalid_bot_token_prefix() {
        let config = SlackConfig {
            app_token: "xapp-valid-token".to_string(),
            bot_token: "invalid-token".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("xoxb-"),
            "Error should mention xoxb- prefix: {}",
            err
        );
    }

    #[test]
    fn test_validate_valid_config() {
        let config = SlackConfig {
            app_token: "xapp-1-ABCDEF123456".to_string(),
            bot_token: "xoxb-1234567890-ABCDEF".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_channel_allowed_empty_list() {
        let config = SlackConfig::default();
        assert!(config.is_channel_allowed("C12345"));
        assert!(config.is_channel_allowed("D67890"));
    }

    #[test]
    fn test_channel_allowed_with_list() {
        let config = SlackConfig {
            allowed_channels: vec!["C12345".to_string(), "C67890".to_string()],
            ..Default::default()
        };
        assert!(config.is_channel_allowed("C12345"));
        assert!(config.is_channel_allowed("C67890"));
        assert!(!config.is_channel_allowed("C99999"));
    }

    #[test]
    fn test_user_allowlist_empty_allows_all() {
        let config = SlackConfig::default();
        assert!(config.is_user_allowed("U123"));
        assert!(config.is_user_allowed("U456"));
    }

    #[test]
    fn test_user_allowlist_restricts() {
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };
        assert!(config.is_user_allowed("U123"));
        assert!(!config.is_user_allowed("U456"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = SlackConfig {
            app_token: "xapp-1-ABCDEF".to_string(),
            bot_token: "xoxb-123-ABC".to_string(),
            allowed_channels: vec!["C123".to_string()],
            send_typing: false,
            dm_allowed: true,
            enable_reactions: false,
            enable_editing: false,
            enable_deletion: true,
            debounce_ms: 500,
            user_allowlist: vec!["U123".to_string(), "U456".to_string()],
            resolve_user_names: true,
            directory_ttl_secs: 7200,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SlackConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.app_token, config.app_token);
        assert_eq!(deserialized.bot_token, config.bot_token);
        assert_eq!(deserialized.allowed_channels, config.allowed_channels);
        assert_eq!(deserialized.send_typing, config.send_typing);
        assert_eq!(deserialized.dm_allowed, config.dm_allowed);
        assert_eq!(deserialized.enable_reactions, config.enable_reactions);
        assert_eq!(deserialized.enable_editing, config.enable_editing);
        assert_eq!(deserialized.enable_deletion, config.enable_deletion);
        assert_eq!(deserialized.debounce_ms, config.debounce_ms);
        assert_eq!(deserialized.user_allowlist, config.user_allowlist);
        assert_eq!(deserialized.resolve_user_names, config.resolve_user_names);
        assert_eq!(deserialized.directory_ttl_secs, config.directory_ttl_secs);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{"app_token": "xapp-test", "bot_token": "xoxb-test"}"#;
        let config: SlackConfig = serde_json::from_str(json).unwrap();

        assert!(config.send_typing);
        assert!(config.dm_allowed);
        assert!(config.enable_reactions);
        assert!(config.enable_editing);
        assert!(!config.enable_deletion);
        assert_eq!(config.debounce_ms, 700);
        assert!(config.allowed_channels.is_empty());
        assert!(config.user_allowlist.is_empty());
        assert!(!config.resolve_user_names);
        assert_eq!(config.directory_ttl_secs, 3600);
    }
}
