use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQConfig {
    pub accounts: Vec<QQAccountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQAccountConfig {
    pub id: String,
    pub app_id: String,
    pub client_secret: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub dm_policy: QQDmPolicy,
    #[serde(default)]
    pub group_policy: QQGroupPolicy,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QQDmPolicy {
    #[default]
    Allowlist,
    Open,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QQGroupPolicy {
    #[default]
    Allowlist,
    MentionOnly,
    Open,
}

fn default_true() -> bool {
    true
}
