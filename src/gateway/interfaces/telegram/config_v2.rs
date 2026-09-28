use crate::gateway::coalescer::CoalescingConfig;
use serde::{Deserialize, Serialize};

/// DM access policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    Disabled,
    #[default]
    Pairing,
    Allowlist,
    Open,
}

/// Group access policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    Disabled,
    #[default]
    Allowlist,
    Open,
}

/// Status reaction configuration for streaming lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StatusReactionConfig {
    /// Reaction to set when agent starts processing
    #[serde(default)]
    pub processing: Option<String>,
    /// Reaction to set while the model is emitting a `<think>` block.
    ///
    /// Defaults to `processing` (a single emoji for the whole "thinking"
    /// phase). Set to a distinct symbol (e.g. `"🤔"`) when a deploy wants
    /// the user to be able to tell at-a-glance whether the model is
    /// mid-thought or post-thought-but-pre-tool — the latter is the same
    /// emoji as `processing` so they look identical without this knob.
    #[serde(default)]
    pub thinking: Option<String>,
    /// Reaction to set when tools are executing
    #[serde(default)]
    pub tool_active: Option<String>,
    /// Reaction to set when streaming is complete
    #[serde(default)]
    pub complete: Option<String>,
}

/// Link preview generation policy for outbound messages.
///
/// AI assistants frequently cite URLs; Telegram's default behaviour expands the
/// first link into a large preview card that clutters the conversation. This
/// enum lets a deployment control that behaviour per account/group/topic.
///
/// Maps to Telegram's `LinkPreviewOptions`. `SmallMedia`/`LargeMedia` are
/// intentionally omitted because they are ignored unless an explicit preview
/// URL is supplied, which Aleph never does.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkPreviewMode {
    /// Telegram's automatic preview of the first link (default, current behaviour).
    #[default]
    Enabled,
    /// Suppress link preview cards entirely.
    Disabled,
    /// Keep the automatic preview but render it above the message text.
    Above,
}

/// Streaming mode for Telegram delivery.
///
/// **`draft_api_enabled` deliberately CUT (P1-A, 2026-09)** — Telegram's
/// `sendMessageDraft` API was never wired through Aleph's orchestrator, the
/// `streaming/draft_api.rs` shim that wrapped it was removed in 2026-07, and
/// openclaw's § 5.14 doc explicitly warns against shipping it (the API is
/// restricted to a small set of opted-in bot owners and silently no-ops for
/// everyone else). If/when the restriction changes and we want it back, the
/// field is recoverable from git history — it used to gate the orchestrator
/// path in `inbound_router/executor.rs::try_create_telegram_emitter`. The
/// orchestrator now lights up purely on `reasoning_lane_enabled` /
/// `status_reactions`, which are the two Telegram-streaming capabilities
/// Aleph actually delivers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
            reasoning_lane_enabled: false,
            status_reactions: StatusReactionConfig::default(),
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_false() -> bool {
    false
}

const fn default_debounce_ms() -> u64 {
    800
}

const fn default_min_initial_chars() -> usize {
    30
}

const fn default_max_edit_interval_ms() -> u64 {
    200
}

const fn default_stream_buffer_size() -> usize {
    256
}

const fn default_max_retries() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicyMode {
    #[default]
    Reply,
    Silent,
    Once,
    AdminOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// SHA-256 fingerprint (lowercase hex) of the bot token this account is
    /// expected to use. Verified at boot and by the doctor against the
    /// running `bot_token`. A mismatch is logged at `warn` (the bot still
    /// starts — a mismatched token surfaces later as a `get_me` failure, which
    /// the operator can already diagnose) and surfaces as a diagnostic entry
    /// so a misconfigured deployment is observable without a reboot.
    ///
    /// Why optional: emitting this field is a deliberate choice that costs a
    /// config writer a `echo -n "$TOKEN" | sha256sum` step. We do not want
    /// that friction to block deployments that already validate at the bot
    /// level; we want it cheap to enable on hosts where the token is shared
    /// with another system and a drift check is worth one extra line of YAML.
    #[serde(default)]
    pub token_fingerprint: Option<String>,
    pub bot_username: Option<String>,
    pub default_agent: Option<String>,
    pub dm_policy: Option<DmPolicy>,
    pub group_policy: Option<GroupPolicy>,
    pub send_typing: Option<bool>,
    /// Only respond to group messages that address the bot — an `@username`
    /// mention, a `/cmd@username` command, or a reply to one of the bot's own
    /// messages. Defaults to `false` so existing deployments keep responding to
    /// every group message; enable it for privacy-mode-disabled bots that
    /// should stay quiet unless actually addressed. DMs are never gated.
    pub require_mention: Option<bool>,
    pub allowed_users: Option<Vec<i64>>,
    pub allowed_groups: Option<Vec<i64>>,
    pub streaming: Option<StreamingOptions>,
    pub error_policy: Option<ErrorPolicy>,
    /// HTTP proxy URL for Telegram API requests (e.g., "<http://proxy.example.com:8080>")
    pub proxy_url: Option<String>,
    /// When true (default), fallback to plain text if HTML parsing fails.
    pub html_fallback: Option<bool>,
    /// Link preview policy for outbound messages (defaults to `Enabled`).
    pub link_preview: Option<LinkPreviewMode>,
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
    #[must_use]
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
