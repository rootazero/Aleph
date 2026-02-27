//! Slack Channel Configuration
//!
//! Configuration types for the Slack Bot integration using Socket Mode + REST API.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
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
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            app_token: String::new(),
            bot_token: String::new(),
            allowed_channels: Vec::new(),
            send_typing: true,
            dm_allowed: true,
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
            return Err("app_token must start with 'xapp-' (Socket Mode app-level token)".to_string());
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
        assert!(err.contains("xapp-"), "Error should mention xapp- prefix: {}", err);
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
        assert!(err.contains("xoxb-"), "Error should mention xoxb- prefix: {}", err);
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
    fn test_serde_roundtrip() {
        let config = SlackConfig {
            app_token: "xapp-1-ABCDEF".to_string(),
            bot_token: "xoxb-123-ABC".to_string(),
            allowed_channels: vec!["C123".to_string()],
            send_typing: false,
            dm_allowed: true,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SlackConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.app_token, config.app_token);
        assert_eq!(deserialized.bot_token, config.bot_token);
        assert_eq!(deserialized.allowed_channels, config.allowed_channels);
        assert_eq!(deserialized.send_typing, config.send_typing);
        assert_eq!(deserialized.dm_allowed, config.dm_allowed);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{"app_token": "xapp-test", "bot_token": "xoxb-test"}"#;
        let config: SlackConfig = serde_json::from_str(json).unwrap();

        assert!(config.send_typing);
        assert!(config.dm_allowed);
        assert!(config.allowed_channels.is_empty());
    }
}
