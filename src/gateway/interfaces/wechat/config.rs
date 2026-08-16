//! `WeChat` Channel Configuration
//!
//! Configuration types for the iLink Bot API integration.

use serde::{Deserialize, Serialize};

use super::types::{ILINK_BASE_URL, WEIXIN_CDN_BASE_URL};

/// DM access policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Accept messages from any user (default).
    #[default]
    Open,
    /// Only accept messages from users in the allowlist.
    Allowlist,
    /// Reject all DMs.
    Disabled,
}

/// Group access policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Reject all group messages (default).
    #[default]
    Disabled,
    /// Accept messages from any group.
    Open,
    /// Only accept messages from groups in the allowlist.
    Allowlist,
}

/// `WeChat` channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeChatConfig {
    /// Account identifier for this `WeChat` instance.
    pub account_id: String,

    /// iLink bot token.
    pub token: String,

    /// iLink API base URL.
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// `WeChat` CDN base URL for media downloads.
    #[serde(default = "default_cdn_url")]
    pub cdn_base_url: String,

    /// DM access policy.
    #[serde(default)]
    pub dm_policy: DmPolicy,

    /// Group access policy.
    #[serde(default)]
    pub group_policy: GroupPolicy,

    /// Allowed user IDs for DMs (used when `dm_policy` is Allowlist).
    #[serde(default)]
    pub allow_from: Vec<String>,

    /// Allowed group IDs (used when `group_policy` is Allowlist).
    #[serde(default)]
    pub group_allow_from: Vec<String>,

    /// Split long messages into multiple chunks.
    #[serde(default)]
    pub split_multiline_messages: bool,

    /// Delay between sending message chunks (seconds).
    #[serde(default = "default_chunk_delay")]
    pub send_chunk_delay_seconds: f64,

    /// Number of retries for failed chunk sends.
    #[serde(default = "default_chunk_retries")]
    pub send_chunk_retries: u32,

    /// Data directory for persistent storage.
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}

fn default_base_url() -> String {
    ILINK_BASE_URL.to_string()
}

fn default_cdn_url() -> String {
    WEIXIN_CDN_BASE_URL.to_string()
}

const fn default_chunk_delay() -> f64 {
    0.35
}

const fn default_chunk_retries() -> u32 {
    2
}

fn default_data_dir() -> String {
    std::env::var("ALEPH_DATA_DIR").unwrap_or_else(|_| {
        crate::utils::paths::get_config_dir()
            .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
    })
}

impl Default for WeChatConfig {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            token: String::new(),
            base_url: default_base_url(),
            cdn_base_url: default_cdn_url(),
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::default(),
            allow_from: Vec::new(),
            group_allow_from: Vec::new(),
            split_multiline_messages: false,
            send_chunk_delay_seconds: default_chunk_delay(),
            send_chunk_retries: default_chunk_retries(),
            data_dir: default_data_dir(),
        }
    }
}

impl WeChatConfig {
    /// Validate configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.account_id.is_empty() {
            return Err("account_id is required".to_string());
        }
        if self.token.is_empty() {
            return Err("token is required".to_string());
        }
        Ok(())
    }

    /// Check if a user ID is allowed for DM.
    #[must_use]
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        match self.dm_policy {
            DmPolicy::Open => true,
            DmPolicy::Allowlist => self.allow_from.contains(&user_id.to_string()),
            DmPolicy::Disabled => false,
        }
    }

    /// Check if a group ID is allowed.
    #[must_use]
    pub fn is_group_allowed(&self, group_id: &str) -> bool {
        match self.group_policy {
            GroupPolicy::Open => true,
            GroupPolicy::Allowlist => self.group_allow_from.contains(&group_id.to_string()),
            GroupPolicy::Disabled => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WeChatConfig::default();
        assert!(config.account_id.is_empty());
        assert!(config.token.is_empty());
        assert_eq!(config.base_url, ILINK_BASE_URL);
        assert_eq!(config.cdn_base_url, WEIXIN_CDN_BASE_URL);
        assert_eq!(config.dm_policy, DmPolicy::Open);
        assert_eq!(config.group_policy, GroupPolicy::Disabled);
    }

    #[test]
    fn test_validate_requires_account_id() {
        let config = WeChatConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_requires_token() {
        let config = WeChatConfig {
            account_id: "test".to_string(),
            token: "".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_ok() {
        let config = WeChatConfig {
            account_id: "test".to_string(),
            token: "token123".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_is_user_allowed_open_policy() {
        let config = WeChatConfig::default();
        assert!(config.is_user_allowed("anyone"));
    }

    #[test]
    fn test_is_user_allowed_allowlist_policy() {
        let config = WeChatConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["wxid_abc".to_string()],
            ..Default::default()
        };
        assert!(config.is_user_allowed("wxid_abc"));
        assert!(!config.is_user_allowed("wxid_xyz"));
    }

    #[test]
    fn test_is_group_allowed_disabled() {
        let config = WeChatConfig::default();
        assert!(!config.is_group_allowed("any_group"));
    }

    #[test]
    fn test_is_group_allowed_open() {
        let config = WeChatConfig {
            group_policy: GroupPolicy::Open,
            ..Default::default()
        };
        assert!(config.is_group_allowed("any_group"));
    }
}
