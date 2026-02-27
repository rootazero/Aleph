//! Generic Webhook Channel Configuration
//!
//! Configuration types for the bidirectional HTTP webhook integration.
//! Any system that can POST JSON and receive POST JSON can integrate
//! with Aleph through this channel.

use serde::{Deserialize, Serialize};

fn default_path() -> String {
    "/webhook/generic".to_string()
}

/// Generic webhook channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookChannelConfig {
    /// HMAC-SHA256 secret for signature verification (inbound + outbound)
    pub secret: String,

    /// URL to POST outbound messages to
    pub callback_url: String,

    /// URL path to receive inbound webhooks on (default: "/webhook/generic")
    #[serde(default = "default_path")]
    pub path: String,

    /// List of allowed sender_ids (empty = all allowed)
    #[serde(default)]
    pub allowed_senders: Vec<String>,
}

impl Default for WebhookChannelConfig {
    fn default() -> Self {
        Self {
            secret: String::new(),
            callback_url: String::new(),
            path: default_path(),
            allowed_senders: Vec::new(),
        }
    }
}

impl WebhookChannelConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.secret.is_empty() {
            return Err("secret is required".to_string());
        }
        if self.callback_url.is_empty() {
            return Err("callback_url is required".to_string());
        }
        if !self.path.starts_with('/') {
            return Err("path must start with '/'".to_string());
        }
        Ok(())
    }

    /// Check if a sender_id is allowed
    pub fn is_sender_allowed(&self, sender_id: &str) -> bool {
        if self.allowed_senders.is_empty() {
            true
        } else {
            self.allowed_senders.iter().any(|s| s == sender_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WebhookChannelConfig::default();
        assert!(config.secret.is_empty());
        assert!(config.callback_url.is_empty());
        assert_eq!(config.path, "/webhook/generic");
        assert!(config.allowed_senders.is_empty());
    }

    #[test]
    fn test_validate_empty_secret() {
        let config = WebhookChannelConfig::default();
        let err = config.validate().unwrap_err();
        assert_eq!(err, "secret is required");
    }

    #[test]
    fn test_validate_empty_callback_url() {
        let config = WebhookChannelConfig {
            secret: "my-secret".to_string(),
            callback_url: String::new(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err, "callback_url is required");
    }

    #[test]
    fn test_validate_bad_path() {
        let config = WebhookChannelConfig {
            secret: "my-secret".to_string(),
            callback_url: "https://example.com/callback".to_string(),
            path: "no-leading-slash".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("/"), "Error should mention '/' requirement: {}", err);
    }

    #[test]
    fn test_validate_valid_config() {
        let config = WebhookChannelConfig {
            secret: "my-secret".to_string(),
            callback_url: "https://example.com/callback".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_custom_path() {
        let config = WebhookChannelConfig {
            secret: "my-secret".to_string(),
            callback_url: "https://example.com/callback".to_string(),
            path: "/my/custom/path".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sender_allowed_empty_list() {
        let config = WebhookChannelConfig::default();
        assert!(config.is_sender_allowed("anyone"));
        assert!(config.is_sender_allowed("user-123"));
    }

    #[test]
    fn test_sender_allowed_with_list() {
        let config = WebhookChannelConfig {
            allowed_senders: vec![
                "user-123".to_string(),
                "user-456".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.is_sender_allowed("user-123"));
        assert!(config.is_sender_allowed("user-456"));
        assert!(!config.is_sender_allowed("user-789"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = WebhookChannelConfig {
            secret: "test-secret".to_string(),
            callback_url: "https://example.com/cb".to_string(),
            path: "/webhook/custom".to_string(),
            allowed_senders: vec!["user-1".to_string()],
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WebhookChannelConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.secret, config.secret);
        assert_eq!(deserialized.callback_url, config.callback_url);
        assert_eq!(deserialized.path, config.path);
        assert_eq!(deserialized.allowed_senders, config.allowed_senders);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{"secret": "s", "callback_url": "https://x.com/cb"}"#;
        let config: WebhookChannelConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.path, "/webhook/generic");
        assert!(config.allowed_senders.is_empty());
    }

    #[test]
    fn test_serde_with_all_fields() {
        let json = r#"{
            "secret": "my-secret",
            "callback_url": "https://example.com/callback",
            "path": "/webhook/myapp",
            "allowed_senders": ["alice", "bob"]
        }"#;
        let config: WebhookChannelConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.secret, "my-secret");
        assert_eq!(config.callback_url, "https://example.com/callback");
        assert_eq!(config.path, "/webhook/myapp");
        assert_eq!(config.allowed_senders, vec!["alice", "bob"]);
    }
}
