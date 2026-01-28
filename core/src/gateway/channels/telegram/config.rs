//! Telegram Channel Configuration
//!
//! Configuration types for the Telegram Bot integration.

use serde::{Deserialize, Serialize};

/// Telegram channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot token from @BotFather
    pub bot_token: String,

    /// Bot username (without @)
    #[serde(default)]
    pub bot_username: Option<String>,

    /// Allowed user IDs (empty = allow all)
    #[serde(default)]
    pub allowed_users: Vec<i64>,

    /// Allowed group/chat IDs (empty = allow all groups)
    #[serde(default)]
    pub allowed_groups: Vec<i64>,

    /// Allow direct messages
    #[serde(default = "default_true")]
    pub dm_allowed: bool,

    /// Allow group messages
    #[serde(default = "default_true")]
    pub groups_allowed: bool,

    /// Webhook configuration (optional, defaults to long-polling)
    #[serde(default)]
    pub webhook: Option<WebhookConfig>,

    /// Polling interval in seconds (for long-polling mode)
    #[serde(default = "default_polling_interval")]
    pub polling_interval_secs: u64,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,

    /// Maximum retries for failed messages
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

/// Webhook configuration for receiving updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Public URL for the webhook
    pub url: String,

    /// Port to listen on (default: 8443)
    #[serde(default = "default_webhook_port")]
    pub port: u16,

    /// Path for the webhook endpoint
    #[serde(default = "default_webhook_path")]
    pub path: String,

    /// SSL certificate path (optional, for self-signed)
    pub certificate: Option<String>,

    /// Secret token for webhook verification
    #[serde(default)]
    pub secret_token: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_polling_interval() -> u64 {
    1
}

fn default_max_retries() -> u32 {
    3
}

fn default_webhook_port() -> u16 {
    8443
}

fn default_webhook_path() -> String {
    "/telegram/webhook".to_string()
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token: String::new(),
            bot_username: None,
            allowed_users: Vec::new(),
            allowed_groups: Vec::new(),
            dm_allowed: true,
            groups_allowed: true,
            webhook: None,
            polling_interval_secs: 1,
            send_typing: true,
            max_retries: 3,
        }
    }
}

impl TelegramConfig {
    /// Create config from environment variable
    pub fn from_env() -> Option<Self> {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").ok()?;
        Some(Self {
            bot_token,
            ..Default::default()
        })
    }

    /// Check if a user ID is allowed
    pub fn is_user_allowed(&self, user_id: i64) -> bool {
        if self.allowed_users.is_empty() {
            true
        } else {
            self.allowed_users.contains(&user_id)
        }
    }

    /// Check if a group/chat ID is allowed
    pub fn is_group_allowed(&self, chat_id: i64) -> bool {
        if !self.groups_allowed {
            return false;
        }
        if self.allowed_groups.is_empty() {
            true
        } else {
            self.allowed_groups.contains(&chat_id)
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.bot_token.is_empty() {
            return Err("bot_token is required".to_string());
        }
        if !self.bot_token.contains(':') {
            return Err("bot_token format invalid (expected: <bot_id>:<token>)".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TelegramConfig::default();
        assert!(config.bot_token.is_empty());
        assert!(config.dm_allowed);
        assert!(config.groups_allowed);
        assert_eq!(config.polling_interval_secs, 1);
    }

    #[test]
    fn test_user_allowed() {
        let mut config = TelegramConfig::default();

        // Empty allowed_users = allow all
        assert!(config.is_user_allowed(12345));

        // With allowed_users list
        config.allowed_users = vec![12345, 67890];
        assert!(config.is_user_allowed(12345));
        assert!(!config.is_user_allowed(99999));
    }

    #[test]
    fn test_group_allowed() {
        let mut config = TelegramConfig::default();

        // Groups allowed by default
        assert!(config.is_group_allowed(-100123456));

        // Disable groups
        config.groups_allowed = false;
        assert!(!config.is_group_allowed(-100123456));

        // Re-enable with specific list
        config.groups_allowed = true;
        config.allowed_groups = vec![-100123456];
        assert!(config.is_group_allowed(-100123456));
        assert!(!config.is_group_allowed(-100999999));
    }

    #[test]
    fn test_validate() {
        let mut config = TelegramConfig::default();

        // Empty token
        assert!(config.validate().is_err());

        // Invalid format
        config.bot_token = "invalid".to_string();
        assert!(config.validate().is_err());

        // Valid format
        config.bot_token = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11".to_string();
        assert!(config.validate().is_ok());
    }
}
