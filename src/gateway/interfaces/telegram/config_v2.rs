use super::config::{DmPolicy, GroupPolicy, StreamingOptions};
use serde::{Deserialize, Serialize};

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
}
