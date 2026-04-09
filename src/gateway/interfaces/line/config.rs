//! LINE Channel Configuration
//!
//! Configuration types for the LINE Messaging API integration.

use serde::{Deserialize, Serialize};

use crate::gateway::coalescer::CoalescingConfig;

/// DM access policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LineDmPolicy {
    /// Require a pairing code before accepting messages (safe default).
    #[default]
    Pairing,
    /// Only accept messages from users in the allowlist.
    Allowlist,
    /// Accept messages from any user.
    Open,
    /// Reject all DMs.
    Disabled,
}

/// Group access policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LineGroupPolicy {
    /// Only accept messages from allowed groups (default).
    #[default]
    Allowlist,
    /// Accept messages from any group.
    Open,
    /// Reject all group messages.
    Disabled,
}

/// LINE channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineConfig {
    /// Long-lived channel access token from LINE Console.
    pub channel_access_token: String,

    /// Channel secret from LINE Console.
    pub channel_secret: String,

    /// Webhook path (default: "line/webhook").
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,

    /// Webhook server host (default: "0.0.0.0").
    #[serde(default = "default_webhook_host")]
    pub webhook_host: String,

    /// Webhook server port (default: 8080).
    #[serde(default = "default_webhook_port")]
    pub webhook_port: u16,

    /// DM access policy (default: Pairing).
    #[serde(default)]
    pub dm_policy: LineDmPolicy,

    /// Group access policy (default: Allowlist).
    #[serde(default)]
    pub group_policy: LineGroupPolicy,

    /// Allowed user IDs for DMs (empty = allow all).
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Allowed group IDs (empty = allow all groups).
    #[serde(default)]
    pub allowed_groups: Vec<String>,

    /// Send typing indicator while processing.
    #[serde(default = "default_true")]
    pub send_typing: bool,

    /// Message coalescing configuration.
    #[serde(default)]
    pub coalescing: Option<CoalescingConfig>,
}

fn default_webhook_path() -> String {
    "line/webhook".to_string()
}

fn default_webhook_host() -> String {
    "0.0.0.0".to_string()
}

fn default_webhook_port() -> u16 {
    8080
}

fn default_true() -> bool {
    true
}

impl Default for LineConfig {
    fn default() -> Self {
        Self {
            channel_access_token: String::new(),
            channel_secret: String::new(),
            webhook_path: default_webhook_path(),
            webhook_host: default_webhook_host(),
            webhook_port: default_webhook_port(),
            dm_policy: LineDmPolicy::default(),
            group_policy: LineGroupPolicy::default(),
            allowed_users: Vec::new(),
            allowed_groups: Vec::new(),
            send_typing: true,
            coalescing: Some(CoalescingConfig::default()),
        }
    }
}

impl LineConfig {
    /// Validate configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.channel_access_token.is_empty() {
            return Err("channel_access_token is required".to_string());
        }
        if self.channel_secret.is_empty() {
            return Err("channel_secret is required".to_string());
        }
        Ok(())
    }

    /// Check if a user ID is allowed for DM.
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.allowed_users.is_empty() {
            true
        } else {
            self.allowed_users.contains(&user_id.to_string())
        }
    }

    /// Check if a group ID is allowed.
    pub fn is_group_allowed(&self, group_id: &str) -> bool {
        if self.allowed_groups.is_empty() {
            true
        } else {
            self.allowed_groups.contains(&group_id.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LineConfig::default();
        assert!(config.channel_access_token.is_empty());
        assert!(config.channel_secret.is_empty());
        assert_eq!(config.webhook_path, "line/webhook");
        assert_eq!(config.webhook_host, "0.0.0.0");
        assert_eq!(config.webhook_port, 8080);
        assert!(config.send_typing);
    }

    #[test]
    fn test_validate_empty_token() {
        let config = LineConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_secret() {
        let config = LineConfig {
            channel_access_token: "token".to_string(),
            channel_secret: "".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_ok() {
        let config = LineConfig {
            channel_access_token: "test_token".to_string(),
            channel_secret: "test_secret".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_policy_defaults() {
        assert_eq!(LineDmPolicy::default(), LineDmPolicy::Pairing);
        assert_eq!(LineGroupPolicy::default(), LineGroupPolicy::Allowlist);
    }

    #[test]
    fn test_allowed_users_default_empty() {
        let config = LineConfig::default();
        assert!(config.allowed_users.is_empty());
    }

    #[test]
    fn test_allowed_groups_default_empty() {
        let config = LineConfig::default();
        assert!(config.allowed_groups.is_empty());
    }
}
