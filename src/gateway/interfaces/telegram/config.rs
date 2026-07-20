//! Telegram Channel Configuration
//!
//! Configuration types for the Telegram Bot integration.

use serde::{Deserialize, Serialize};

use crate::gateway::coalescer::CoalescingConfig;

use super::config_v2::{
    DmPolicy, GroupPolicy, StreamingOptions, TelegramAccountConfig, TelegramConfigV2,
};

/// Telegram channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot token from @`BotFather`
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

    /// Link preview policy for outbound messages (default: `Enabled`).
    #[serde(default)]
    pub link_preview: super::config_v2::LinkPreviewMode,
}

const fn default_true() -> bool {
    true
}

const fn default_polling_interval() -> u64 {
    1
}

const fn default_max_retries() -> u32 {
    3
}

fn default_coalescing() -> Option<CoalescingConfig> {
    Some(CoalescingConfig::default())
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
            polling_interval_secs: 1,
            send_typing: true,
            max_retries: 3,
            coalescing: default_coalescing(),
            streaming: StreamingOptions::default(),
            link_preview: super::config_v2::LinkPreviewMode::default(),
        }
    }
}

impl TelegramConfig {
    /// Create config from environment variable
    #[must_use]
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
    /// is explicitly listed. Runtime pairing is owned by the router's
    /// `pairing_store`, not tracked here.
    #[must_use]
    pub fn is_user_allowed(&self, user_id: i64) -> bool {
        if self.allowed_users.is_empty() {
            true
        } else {
            self.allowed_users.contains(&user_id)
        }
    }

    /// Check if a group/chat ID is allowed
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn upgrade_to_v2(&self) -> TelegramConfigV2 {
        TelegramConfigV2 {
            accounts: vec![TelegramAccountConfig {
                id: "default".to_string(),
                bot_token: self.bot_token.clone(),
                bot_username: self.bot_username.clone(),
                default_agent: None,
                // Resolve through the effective_* accessors so the legacy
                // `dm_allowed`/`groups_allowed` toggles are honored: a config with
                // `dm_allowed: false` must upgrade to `Disabled`, not silently
                // carry the raw `Pairing` default (which would leave DMs
                // reachable). Also applies the allowlist promotion.
                dm_policy: Some(self.effective_dm_policy()),
                group_policy: Some(self.effective_group_policy()),
                send_typing: Some(self.send_typing),
                // Legacy flat config has no mention gate — inherit the default.
                require_mention: None,
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
                proxy_url: None,
                html_fallback: None,
                link_preview: Some(self.link_preview),
                groups: Vec::new(),
            }],
            coalescing: self.coalescing.clone(),
        }
    }
}

/// Parse a Telegram channel config JSON value, accepting either:
/// - the new V2 format (with an `accounts` array), or
/// - the legacy V1 flat format (with top-level `bot_token`).
///
/// V1 configs are auto-upgraded to V2 so existing user configs keep working
/// without forcing a migration. V2 is preferred and tried first; the V1
/// fallback is only attempted when V2 deserialization fails.
pub fn parse_telegram_channel_config(
    value: serde_json::Value,
) -> Result<TelegramConfigV2, serde_json::Error> {
    let config = match serde_json::from_value::<TelegramConfigV2>(value.clone()) {
        Ok(v2) => v2,
        Err(v2_err) => match serde_json::from_value::<TelegramConfig>(value) {
            Ok(v1) => {
                tracing::debug!(
                    "Telegram channel config: auto-upgraded legacy v1 (flat) format to v2"
                );
                v1.upgrade_to_v2()
            }
            Err(_) => return Err(v2_err),
        },
    };

    // Run validation and log warnings for any issues
    let validation_errors = config.validate();
    if !validation_errors.is_empty() {
        for err in &validation_errors {
            tracing::warn!("Telegram config validation: {}", err);
        }
    }

    Ok(config)
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
    fn parse_v2_config_directly() {
        let raw = serde_json::json!({
            "accounts": [{
                "id": "default",
                "bot_token": "123:ABC",
            }]
        });
        let parsed = parse_telegram_channel_config(raw).expect("v2 parses");
        assert_eq!(parsed.accounts.len(), 1);
        assert_eq!(parsed.accounts[0].bot_token, "123:ABC");
    }

    #[test]
    fn parse_v1_flat_config_upgrades_to_v2() {
        // Legacy flat config — what users with old [channels.AlephzBot] entries have.
        let raw = serde_json::json!({
            "bot_username": "AlephzBot",
            "bot_token": "123:ABC",
        });
        let parsed = parse_telegram_channel_config(raw).expect("v1 fallback parses");
        assert_eq!(parsed.accounts.len(), 1);
        assert_eq!(parsed.accounts[0].bot_token, "123:ABC");
        assert_eq!(
            parsed.accounts[0].bot_username.as_deref(),
            Some("AlephzBot")
        );
    }

    #[test]
    fn parse_invalid_config_surfaces_v2_error() {
        // Neither v2 (no accounts) nor v1 (no bot_token) — should fail.
        let raw = serde_json::json!({ "bot_username": "AlephzBot" });
        let err = parse_telegram_channel_config(raw).unwrap_err();
        assert!(err.to_string().contains("accounts"));
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
