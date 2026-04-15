//! Telegram Channel Configuration
//!
//! Configuration types for the Telegram Bot integration.

use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::gateway::coalescer::CoalescingConfig;

use super::config_v2::{TelegramAccountConfig, TelegramConfigV2};

/// DM access policy.
///
/// Controls how the bot handles direct messages from users.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Reject all DMs.
    Disabled,
    /// Require a pairing code before accepting messages (safe default).
    #[default]
    Pairing,
    /// Only accept messages from users in the allowlist.
    Allowlist,
    /// Accept messages from any user.
    Open,
}

/// Group access policy.
///
/// Controls how the bot handles group/supergroup messages.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Reject all group messages.
    Disabled,
    /// Only accept messages from allowed groups (default).
    #[default]
    Allowlist,
    /// Accept messages from any group.
    Open,
}

/// A pending pairing entry: code + creation time + TTL.
#[derive(Debug, Clone)]
pub struct PairingEntry {
    pub code: String,
    pub created_at: Instant,
    pub ttl_secs: u64,
}

impl PairingEntry {
    pub fn new(code: String) -> Self {
        Self {
            code,
            created_at: Instant::now(),
            ttl_secs: 3600,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed().as_secs() > self.ttl_secs
    }
}

/// Streaming delivery options for Telegram's edit-based streaming.
///
/// When enabled, LLM responses are delivered progressively via
/// `editMessageText` instead of buffering until completion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamingOptions {
    /// Whether edit-based streaming is enabled (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Minimum interval between edits in milliseconds (default: 800).
    /// Telegram's edit API is more strictly rate-limited than send.
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,

    /// Minimum characters before sending the initial message (default: 30).
    #[serde(default = "default_min_initial_chars")]
    pub min_initial_chars: usize,
}

impl Default for StreamingOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 800,
            min_initial_chars: 30,
        }
    }
}

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

    /// DM access policy (default: Pairing)
    #[serde(default)]
    pub dm_policy: DmPolicy,

    /// Group access policy (default: Allowlist)
    #[serde(default)]
    pub group_policy: GroupPolicy,

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

    /// Message coalescing configuration (merges rapid-fire messages).
    /// Enabled by default for Telegram channels.
    #[serde(default = "default_coalescing")]
    pub coalescing: Option<CoalescingConfig>,

    /// Streaming delivery options (edit-based progressive output).
    #[serde(default)]
    pub streaming: StreamingOptions,
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

fn default_coalescing() -> Option<CoalescingConfig> {
    Some(CoalescingConfig::default())
}

fn default_debounce_ms() -> u64 {
    800
}

fn default_min_initial_chars() -> usize {
    30
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
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::default(),
            webhook: None,
            polling_interval_secs: 1,
            send_typing: true,
            max_retries: 3,
            coalescing: default_coalescing(),
            streaming: StreamingOptions::default(),
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

    /// Check if a user ID is in the static config allowlist.
    ///
    /// Returns `true` when the allowlist is empty (open to all) or when the user
    /// is explicitly listed. Runtime-paired users are tracked separately in
    /// `AccessController::runtime_users`.
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

    /// Resolve effective DM policy.
    ///
    /// Backward compatibility: non-empty `allowed_users` with the default
    /// `Pairing` policy promotes to `Allowlist`. An explicit `dm_policy`
    /// always wins. `dm_allowed: false` forces `Disabled`.
    pub fn effective_dm_policy(&self) -> DmPolicy {
        if !self.dm_allowed {
            return DmPolicy::Disabled;
        }
        match self.dm_policy {
            DmPolicy::Pairing if !self.allowed_users.is_empty() => DmPolicy::Allowlist,
            ref p => p.clone(),
        }
    }

    /// Resolve effective group policy.
    ///
    /// Backward compatibility: `groups_allowed: false` forces `Disabled`
    /// regardless of the explicit `group_policy` value.
    pub fn effective_group_policy(&self) -> GroupPolicy {
        if !self.groups_allowed {
            return GroupPolicy::Disabled;
        }
        self.group_policy.clone()
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

    /// 将旧版扁平配置升级为新版层级配置
    pub fn upgrade_to_v2(&self) -> TelegramConfigV2 {
        TelegramConfigV2 {
            accounts: vec![TelegramAccountConfig {
                id: "default".to_string(),
                bot_token: self.bot_token.clone(),
                bot_username: self.bot_username.clone(),
                default_agent: None,
                dm_policy: Some(self.dm_policy.clone()),
                group_policy: Some(self.group_policy.clone()),
                send_typing: Some(self.send_typing),
                allowed_users: if self.allowed_users.is_empty() {
                    None
                } else {
                    Some(self.allowed_users.clone())
                },
                allowed_groups: if self.allowed_groups.is_empty() {
                    None
                } else {
                    Some(self.allowed_groups.clone())
                },
                streaming: Some(self.streaming.clone()),
                error_policy: None,
                groups: Vec::new(),
            }],
            coalescing: self.coalescing.clone(),
        }
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

    #[test]
    fn test_pairing_entry_not_expired() {
        let entry = PairingEntry::new("ABC123".to_string());
        assert!(!entry.is_expired());
        assert_eq!(entry.code, "ABC123");
        assert_eq!(entry.ttl_secs, 3600);
    }

    #[test]
    fn test_pairing_entry_expired() {
        let mut entry = PairingEntry::new("XYZ789".to_string());
        // Backdate the creation time so it appears expired
        entry.created_at = Instant::now() - std::time::Duration::from_secs(3601);
        assert!(entry.is_expired());
    }

    // --- DmPolicy / GroupPolicy backward-compatibility tests ---

    #[test]
    fn test_effective_dm_policy_default() {
        let config = TelegramConfig::default();
        assert_eq!(config.effective_dm_policy(), DmPolicy::Pairing);
    }

    #[test]
    fn test_effective_dm_policy_with_allowlist() {
        let config = TelegramConfig {
            allowed_users: vec![123],
            ..Default::default()
        };
        // Non-empty allowed_users with default Pairing → promoted to Allowlist
        assert_eq!(config.effective_dm_policy(), DmPolicy::Allowlist);
    }

    #[test]
    fn test_effective_dm_policy_explicit_open() {
        let config = TelegramConfig {
            dm_policy: DmPolicy::Open,
            allowed_users: vec![123], // Should be ignored
            ..Default::default()
        };
        assert_eq!(config.effective_dm_policy(), DmPolicy::Open);
    }

    #[test]
    fn test_effective_dm_policy_dm_disabled() {
        let config = TelegramConfig {
            dm_allowed: false,
            dm_policy: DmPolicy::Open, // Overridden by dm_allowed=false
            ..Default::default()
        };
        assert_eq!(config.effective_dm_policy(), DmPolicy::Disabled);
    }

    #[test]
    fn test_effective_group_policy_disabled() {
        let config = TelegramConfig {
            groups_allowed: false,
            group_policy: GroupPolicy::Open, // Overridden
            ..Default::default()
        };
        assert_eq!(config.effective_group_policy(), GroupPolicy::Disabled);
    }

    #[test]
    fn test_effective_group_policy_default() {
        let config = TelegramConfig::default();
        assert_eq!(config.effective_group_policy(), GroupPolicy::Allowlist);
    }

    #[test]
    fn test_effective_group_policy_open() {
        let config = TelegramConfig {
            group_policy: GroupPolicy::Open,
            ..Default::default()
        };
        assert_eq!(config.effective_group_policy(), GroupPolicy::Open);
    }
}
