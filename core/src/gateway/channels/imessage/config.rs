//! iMessage Channel Configuration

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default database path
fn default_db_path() -> String {
    "~/Library/Messages/chat.db".to_string()
}

/// Default poll interval (1 second)
fn default_poll_interval() -> u64 {
    1000
}

/// Default DM policy
fn default_dm_policy() -> DmPolicy {
    DmPolicy::Pairing
}

/// DM (Direct Message) policy for unknown senders
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicy {
    /// Require pairing code for unknown senders
    Pairing,
    /// Only allow senders in the allowlist
    Allowlist,
    /// Allow all senders (open)
    Open,
    /// Disable DMs entirely
    Disabled,
}

impl Default for DmPolicy {
    fn default() -> Self {
        Self::Pairing
    }
}

/// Group message policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupPolicy {
    /// Allow all groups (require mention by default)
    Open,
    /// Only allow groups in the allowlist
    Allowlist,
    /// Disable group messages
    Disabled,
}

impl Default for GroupPolicy {
    fn default() -> Self {
        Self::Open
    }
}

/// iMessage channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IMessageConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,

    /// Path to the Messages database
    #[serde(default = "default_db_path")]
    pub db_path: String,

    /// Poll interval in milliseconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u64,

    /// DM policy for unknown senders
    #[serde(default = "default_dm_policy")]
    pub dm_policy: DmPolicy,

    /// Group message policy
    #[serde(default)]
    pub group_policy: GroupPolicy,

    /// Allowlist of phone numbers/emails that can send DMs
    #[serde(default)]
    pub allow_from: Vec<String>,

    /// Allowlist of phone numbers/emails that can send group messages
    #[serde(default)]
    pub group_allow_from: Vec<String>,

    /// Whether to require @mention in groups
    #[serde(default = "default_true")]
    pub require_mention: bool,

    /// Bot's name for mention detection
    #[serde(default)]
    pub bot_name: Option<String>,

    /// Whether to include attachments in inbound messages
    #[serde(default = "default_true")]
    pub include_attachments: bool,

    /// Maximum attachment size in bytes (0 = unlimited)
    #[serde(default)]
    pub max_attachment_size: u64,

    /// Inbound message debounce time in milliseconds
    #[serde(default = "default_debounce")]
    pub inbound_debounce_ms: u64,
}

fn default_true() -> bool {
    true
}

fn default_debounce() -> u64 {
    500
}

impl Default for IMessageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            db_path: default_db_path(),
            poll_interval_ms: default_poll_interval(),
            dm_policy: default_dm_policy(),
            group_policy: GroupPolicy::default(),
            allow_from: Vec::new(),
            group_allow_from: Vec::new(),
            require_mention: true,
            bot_name: None,
            include_attachments: true,
            max_attachment_size: 0,
            inbound_debounce_ms: default_debounce(),
        }
    }
}

impl IMessageConfig {
    /// Get the expanded database path
    pub fn db_path(&self) -> PathBuf {
        expand_path(&self.db_path)
    }

    /// Check if a sender is allowed based on DM policy
    pub fn is_dm_allowed(&self, sender: &str) -> bool {
        match self.dm_policy {
            DmPolicy::Open => true,
            DmPolicy::Disabled => false,
            DmPolicy::Pairing => true, // Will prompt for pairing
            DmPolicy::Allowlist => {
                crate::gateway::channels::imessage::target::is_allowed_sender(
                    sender,
                    &self.allow_from,
                )
            }
        }
    }

    /// Check if a group is allowed based on group policy
    pub fn is_group_allowed(&self, chat_id: &str) -> bool {
        match self.group_policy {
            GroupPolicy::Open => true,
            GroupPolicy::Disabled => false,
            GroupPolicy::Allowlist => self.group_allow_from.iter().any(|a| a == chat_id),
        }
    }
}

/// Expand ~ to home directory
fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IMessageConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.poll_interval_ms, 1000);
        assert_eq!(config.dm_policy, DmPolicy::Pairing);
    }

    #[test]
    fn test_expand_path() {
        let expanded = expand_path("~/Library/Messages/chat.db");
        assert!(expanded.to_string_lossy().contains("Library/Messages/chat.db"));
        assert!(!expanded.to_string_lossy().starts_with("~/"));
    }

    #[test]
    fn test_deserialize() {
        let toml = r#"
            enabled = true
            db_path = "~/Library/Messages/chat.db"
            poll_interval_ms = 2000
            dm_policy = "allowlist"
            allow_from = ["+15551234567", "user@example.com"]
        "#;

        let config: IMessageConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.poll_interval_ms, 2000);
        assert_eq!(config.dm_policy, DmPolicy::Allowlist);
        assert_eq!(config.allow_from.len(), 2);
    }

    #[test]
    fn test_is_dm_allowed() {
        let mut config = IMessageConfig::default();
        config.dm_policy = DmPolicy::Allowlist;
        config.allow_from = vec!["+15551234567".to_string()];

        assert!(config.is_dm_allowed("+15551234567"));
        assert!(!config.is_dm_allowed("+19998887777"));

        config.dm_policy = DmPolicy::Open;
        assert!(config.is_dm_allowed("+19998887777"));
    }
}
