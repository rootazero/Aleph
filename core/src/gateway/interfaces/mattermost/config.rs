//! Mattermost Channel Configuration
//!
//! Configuration types for the Mattermost Bot integration using WebSocket + REST API v4.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Mattermost channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MattermostConfig {
    /// Mattermost server URL (e.g., "https://mattermost.example.com")
    pub server_url: String,

    /// Personal access token or bot token
    pub bot_token: String,

    /// Allowed channel IDs (empty = allow all)
    #[serde(default)]
    pub allowed_channels: Vec<String>,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,
}

impl Default for MattermostConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            bot_token: String::new(),
            allowed_channels: Vec::new(),
            send_typing: true,
        }
    }
}

impl MattermostConfig {
    /// Create config from environment variables
    pub fn from_env() -> Option<Self> {
        let server_url = std::env::var("MATTERMOST_SERVER_URL").ok()?;
        let bot_token = std::env::var("MATTERMOST_BOT_TOKEN").ok()?;
        Some(Self {
            server_url,
            bot_token,
            ..Default::default()
        })
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.server_url.is_empty() {
            return Err("server_url is required".to_string());
        }
        if !self.server_url.starts_with("http://") && !self.server_url.starts_with("https://") {
            return Err(
                "server_url must start with 'http://' or 'https://'".to_string(),
            );
        }
        if self.bot_token.is_empty() {
            return Err("bot_token is required".to_string());
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

    /// Get server URL with trailing slash removed
    pub fn server_url_trimmed(&self) -> &str {
        self.server_url.trim_end_matches('/')
    }

    /// Build the WebSocket URL from the server URL.
    ///
    /// Replaces `https://` with `wss://` and `http://` with `ws://`,
    /// then appends `/api/v4/websocket`.
    pub fn ws_url(&self) -> String {
        let base = self.server_url_trimmed();
        let ws_base = if base.starts_with("https://") {
            base.replacen("https://", "wss://", 1)
        } else if base.starts_with("http://") {
            base.replacen("http://", "ws://", 1)
        } else {
            format!("wss://{base}")
        };
        format!("{ws_base}/api/v4/websocket")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MattermostConfig::default();
        assert!(config.server_url.is_empty());
        assert!(config.bot_token.is_empty());
        assert!(config.allowed_channels.is_empty());
        assert!(config.send_typing);
    }

    #[test]
    fn test_validate_empty_server_url() {
        let config = MattermostConfig::default();
        assert!(config.validate().is_err());
        assert_eq!(config.validate().unwrap_err(), "server_url is required");
    }

    #[test]
    fn test_validate_invalid_server_url_prefix() {
        let config = MattermostConfig {
            server_url: "ftp://mattermost.example.com".to_string(),
            bot_token: "test-token".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("http://") || err.contains("https://"),
            "Error should mention http/https prefix: {}",
            err
        );
    }

    #[test]
    fn test_validate_empty_bot_token() {
        let config = MattermostConfig {
            server_url: "https://mattermost.example.com".to_string(),
            bot_token: String::new(),
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), "bot_token is required");
    }

    #[test]
    fn test_validate_valid_config_https() {
        let config = MattermostConfig {
            server_url: "https://mattermost.example.com".to_string(),
            bot_token: "abcdef123456".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_config_http() {
        let config = MattermostConfig {
            server_url: "http://localhost:8065".to_string(),
            bot_token: "test-token".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_channel_allowed_empty_list() {
        let config = MattermostConfig::default();
        assert!(config.is_channel_allowed("ch-12345"));
        assert!(config.is_channel_allowed("ch-67890"));
    }

    #[test]
    fn test_channel_allowed_with_list() {
        let config = MattermostConfig {
            allowed_channels: vec!["ch-12345".to_string(), "ch-67890".to_string()],
            ..Default::default()
        };
        assert!(config.is_channel_allowed("ch-12345"));
        assert!(config.is_channel_allowed("ch-67890"));
        assert!(!config.is_channel_allowed("ch-99999"));
    }

    #[test]
    fn test_ws_url_https() {
        let config = MattermostConfig {
            server_url: "https://mm.example.com".to_string(),
            ..Default::default()
        };
        assert_eq!(config.ws_url(), "wss://mm.example.com/api/v4/websocket");
    }

    #[test]
    fn test_ws_url_http() {
        let config = MattermostConfig {
            server_url: "http://localhost:8065".to_string(),
            ..Default::default()
        };
        assert_eq!(config.ws_url(), "ws://localhost:8065/api/v4/websocket");
    }

    #[test]
    fn test_ws_url_trailing_slash() {
        let config = MattermostConfig {
            server_url: "https://mm.example.com/".to_string(),
            ..Default::default()
        };
        assert_eq!(config.ws_url(), "wss://mm.example.com/api/v4/websocket");
    }

    #[test]
    fn test_server_url_trimmed() {
        let config = MattermostConfig {
            server_url: "https://mm.example.com/".to_string(),
            ..Default::default()
        };
        assert_eq!(config.server_url_trimmed(), "https://mm.example.com");
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = MattermostConfig {
            server_url: "https://mm.example.com".to_string(),
            bot_token: "test-token-abc".to_string(),
            allowed_channels: vec!["ch-123".to_string()],
            send_typing: false,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MattermostConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.server_url, config.server_url);
        assert_eq!(deserialized.bot_token, config.bot_token);
        assert_eq!(deserialized.allowed_channels, config.allowed_channels);
        assert_eq!(deserialized.send_typing, config.send_typing);
    }

    #[test]
    fn test_serde_defaults() {
        let json =
            r#"{"server_url": "https://mm.example.com", "bot_token": "test-token"}"#;
        let config: MattermostConfig = serde_json::from_str(json).unwrap();

        assert!(config.send_typing);
        assert!(config.allowed_channels.is_empty());
    }
}
