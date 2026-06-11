use crate::gateway::runtime_footer::RuntimeFooterConfig;

#[derive(Debug, Clone)]
pub struct ReplyEmitterConfig {
    /// Minimum buffer size before auto-flush (in characters)
    /// Default: 500 characters
    pub buffer_threshold: usize,

    /// Whether to stream responses to the channel (typewriter mode)
    /// Default: false
    pub stream_enabled: bool,

    /// Whether voice output is enabled for this emitter
    /// Default: false
    pub voice_enabled: bool,

    /// Whether the inbound message requested a voice reply
    /// Default: false
    pub voice_reply_hint: bool,

    /// Minimum interval between streaming edits in milliseconds.
    /// Default: 300 (global). Telegram overrides to 800.
    pub debounce_ms: u64,

    /// Minimum characters before sending the initial streaming message.
    /// Default: 30.
    pub min_initial_chars: usize,

    /// Maximum message length for the target channel (0 = unlimited).
    /// Used for overflow detection during streaming.
    pub max_message_length: usize,

    /// Optional runtime-metadata footer appended to the final reply.
    /// Defaults to a disabled config so existing callers stay no-op.
    pub footer: RuntimeFooterConfig,
}

impl Default for ReplyEmitterConfig {
    fn default() -> Self {
        Self {
            buffer_threshold: 500,
            stream_enabled: false,
            voice_enabled: false,
            voice_reply_hint: false,
            debounce_ms: 300,
            min_initial_chars: 30,
            max_message_length: 0,
            footer: RuntimeFooterConfig::default(),
        }
    }
}

impl ReplyEmitterConfig {
    /// Create config from `output_mode` string ("typewriter" or "instant")
    #[must_use]
    pub fn from_output_mode(mode: &str) -> Self {
        Self {
            stream_enabled: mode == "typewriter",
            ..Default::default()
        }
    }
}
