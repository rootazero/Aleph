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

/// Status reaction configuration for streaming lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StatusReactionConfig {
    /// Reaction to set when agent starts processing
    #[serde(default)]
    pub processing: Option<String>,
    /// Reaction to set when tools are executing
    #[serde(default)]
    pub tool_active: Option<String>,
    /// Reaction to set when streaming is complete
    #[serde(default)]
    pub complete: Option<String>,
}

/// Streaming mode for Telegram delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StreamingMode {
    /// Edit-based streaming (default): updates message via editMessageText
    #[default]
    Edit,
    /// Draft API: uses Telegram's Draft API for streaming (experimental)
    Draft,
    /// Disabled: no streaming, send complete message at once
    Disabled,
}

/// Streaming delivery options for Telegram.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamingOptions {
    /// Overall streaming toggle (legacy, prefer `mode`)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Streaming delivery mode
    #[serde(default)]
    pub mode: StreamingMode,
    /// Debounce interval between edits (ms)
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    /// Minimum characters before first edit
    #[serde(default = "default_min_initial_chars")]
    pub min_initial_chars: usize,
    /// Maximum time between edits (ms) - prevents too frequent updates
    #[serde(default = "default_max_edit_interval_ms")]
    pub max_edit_interval_ms: u64,
    /// Preserve Markdown/HTML formatting during streaming
    #[serde(default = "default_true")]
    pub preserve_formatting: bool,
    /// Buffer size for streaming events
    #[serde(default = "default_stream_buffer_size")]
    pub buffer_size: usize,
    /// Enable experimental Draft API support (deprecated, use `mode = Draft`)
    #[serde(default = "default_false")]
    pub draft_api_enabled: bool,
    /// Enable reasoning lane extraction (<think> tags)
    #[serde(default = "default_false")]
    pub reasoning_lane_enabled: bool,
    /// Status reaction configuration
    #[serde(default)]
    pub status_reactions: StatusReactionConfig,
}

impl Default for StreamingOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: StreamingMode::Edit,
            debounce_ms: 800,
            min_initial_chars: 30,
            max_edit_interval_ms: 200,
            preserve_formatting: true,
            buffer_size: 256,
            draft_api_enabled: false,
            reasoning_lane_enabled: false,
            status_reactions: StatusReactionConfig::default(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_debounce_ms() -> u64 {
    800
}

fn default_min_initial_chars() -> usize {
    30
}

fn default_max_edit_interval_ms() -> u64 {
    200
}

fn default_stream_buffer_size() -> usize {
    256
}

fn default_max_retries() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicyMode {
    #[default]
    Reply,
    Silent,
    Once,
    AdminOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorPolicy {
    #[serde(default)]
    pub mode: ErrorPolicyMode,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

impl Default for ErrorPolicy {
    fn default() -> Self {
        Self {
            mode: ErrorPolicyMode::default(),
            template: None,
            max_retries: 3,
        }
    }
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
    /// HTTP proxy URL for Telegram API requests (e.g., "http://proxy.example.com:8080")
    pub proxy_url: Option<String>,
    /// When true (default), fallback to plain text if HTML parsing fails.
    pub html_fallback: Option<bool>,
    #[serde(default)]
    pub groups: Vec<TelegramGroupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramConfigV2 {
    pub accounts: Vec<TelegramAccountConfig>,
    #[serde(default)]
    pub coalescing: Option<CoalescingConfig>,
}

impl TelegramConfigV2 {
    /// Validate configuration at load time.
    ///
    /// Returns a list of validation errors (empty if valid).
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.accounts.is_empty() {
            errors.push("No Telegram accounts configured".to_string());
        }

        for account in &self.accounts {
            // Bot token must contain a colon (format: <bot_id>:<token>)
            if !account.bot_token.contains(':') {
                errors.push(format!(
                    "Account '{}': bot_token format invalid (expected: <bot_id>:<token>)",
                    account.id
                ));
            }

            // Proxy URL validation
            if let Some(proxy_url) = &account.proxy_url {
                if !proxy_url.is_empty() {
                    if let Err(e) = url::Url::parse(proxy_url) {
                        errors.push(format!(
                            "Account '{}': Invalid proxy_url '{}': {}",
                            account.id, proxy_url, e
                        ));
                    }
                }
            }

            // Streaming options validation
            if let Some(streaming) = &account.streaming {
                if streaming.debounce_ms == 0 {
                    errors.push(format!(
                        "Account '{}': streaming.debounce_ms must be > 0",
                        account.id
                    ));
                }
                if streaming.buffer_size == 0 {
                    errors.push(format!(
                        "Account '{}': streaming.buffer_size must be > 0",
                        account.id
                    ));
                }
            }
        }

        errors
    }
}
