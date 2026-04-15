use crate::gateway::coalescer::CoalescingConfig;
use serde::{Deserialize, Serialize};

/// DM access policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    Disabled,
    #[default]
    Pairing,
    Allowlist,
    Open,
}

/// Group access policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    Disabled,
    #[default]
    Allowlist,
    Open,
}

/// Streaming delivery options for Telegram's edit-based streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamingOptions {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
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

fn default_true() -> bool {
    true
}

fn default_debounce_ms() -> u64 {
    800
}

fn default_min_initial_chars() -> usize {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicy {
    #[default]
    Reply,
    Silent,
    Once,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramTopicConfig {
    pub id: String,
    pub thread_id: i32,
    pub agent: Option<String>,
    pub block_streaming: Option<bool>,
    pub error_policy: Option<ErrorPolicy>,
    pub dm_policy: Option<DmPolicy>,
    pub group_policy: Option<GroupPolicy>,
    pub send_typing: Option<bool>,
    pub allowed_users: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramGroupConfig {
    pub id: String,
    pub chat_id: i64,
    pub agent: Option<String>,
    pub block_streaming: Option<bool>,
    pub error_policy: Option<ErrorPolicy>,
    pub group_policy: Option<GroupPolicy>,
    pub send_typing: Option<bool>,
    pub allowed_users: Option<Vec<i64>>,
    pub topics: Vec<TelegramTopicConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramAccountConfig {
    pub id: String,
    pub bot_token: String,
    pub bot_username: Option<String>,
    pub default_agent: Option<String>,
    pub dm_policy: Option<DmPolicy>,
    pub group_policy: Option<GroupPolicy>,
    pub send_typing: Option<bool>,
    pub allowed_users: Option<Vec<i64>>,
    pub allowed_groups: Option<Vec<i64>>,
    pub streaming: Option<StreamingOptions>,
    pub error_policy: Option<ErrorPolicy>,
    pub groups: Vec<TelegramGroupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramConfigV2 {
    pub accounts: Vec<TelegramAccountConfig>,
    #[serde(default)]
    pub coalescing: Option<CoalescingConfig>,
}
