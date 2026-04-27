//! Reply Emitter - Routes Agent output back to channels
//!
//! The ReplyEmitter implements EventEmitter to capture streaming events from the
//! agent loop and route responses back to the originating channel/conversation.
//!
//! # Output Modes
//!
//! - **Streaming** (`stream_enabled = true`): Sends an initial message once a
//!   character threshold is reached, then progressively edits in real-time as
//!   tokens arrive (debounced). Uses `StreamingController` for state management.
//! - **Instant** (`stream_enabled = false`): Buffers all content, sends once on
//!   completion.
//!
//! Mode is controlled by `BehaviorConfig.output_mode` in config.toml.

use std::borrow::Cow;
use std::sync::LazyLock;
use std::time::Duration;

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, AtomicU64, Ordering};
use async_trait::async_trait;
use regex::Regex;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::channel::OutboundMessage;
use super::channel_registry::ChannelRegistry;
use super::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use super::inbound_context::ReplyRoute;
use crate::gateway::media::PendingMedia;
use crate::gateway::streaming::{StreamAction, StreamingConfig, StreamingController};
use crate::media::cache::MediaCache;

/// Streaming cursor appended to intermediate edits, removed on final.
const STREAMING_CURSOR: &str = "▍";

// ── LLM output sanitization ────────────────────────────────────────────────

/// Strip LLM-internal tags that should never reach the user or TTS engine.
///
/// Removes:
/// - `<think|thinking|thought|antthinking>…</…>` — chain-of-thought reasoning
/// - `<completion-check>…</completion-check>` — agent loop completion markers
/// - `<task-complete/>` — agent loop task boundary
/// - Trailing incomplete tags (e.g. `[[`, `<completion-check`)
///
/// **Code-block aware**: tags inside backtick spans or fenced code blocks are
/// preserved, preventing accidental stripping of example/documentation code.
///
/// Returns `Cow::Borrowed` when no tags are found (zero-alloc fast path).
pub(crate) fn sanitize_llm_output(text: &str) -> Cow<'_, str> {
    // Fast path: quick probe for any tag-like pattern before doing real work.
    static QUICK_PROBE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"<(?:think|thinking|thought|antthinking|completion-check|task-complete)[\s/>]")
            .expect("quick probe regex")
    });
    static BLANK_LINES_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\n{3,}").expect("blank-lines regex"));

    let has_tags = QUICK_PROBE.is_match(text);
    let has_trailing = text.ends_with("[[") || text.ends_with('[');
    // Check for trailing incomplete tag: last '<' has no closing '>'
    let has_incomplete_tag = text
        .rfind('<')
        .is_some_and(|pos| !text[pos..].contains('>'));

    if !has_tags && !has_trailing && !has_incomplete_tag {
        return Cow::Borrowed(text);
    }

    let stripped = strip_tags_code_aware(text);

    // Clean trailing incomplete directives (e.g. "answer text[[" or "<completion-check")
    let cleaned = strip_trailing_incomplete(&stripped);

    let collapsed = BLANK_LINES_RE.replace_all(&cleaned, "\n\n");
    Cow::Owned(collapsed.trim().to_string())
}

/// Tag names that should be stripped (all ASCII, case-insensitive).
const THINKING_TAGS: &[&str] = &["think", "thinking", "thought", "antthinking"];
const OTHER_STRIP_TAGS: &[&str] = &["completion-check"];

/// Strip tags while respecting code block boundaries.
///
/// Operates on `&[u8]` byte slices (all tag names are ASCII) to avoid the
/// `Vec<char>` allocation of the previous implementation. Supports fenced
/// code blocks (```) and multi-backtick inline code spans (`` ` ``, ``` `` ```).
fn strip_tags_code_aware(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;
    let mut in_fenced_code = false;
    // For inline code: 0 = not in code, N = need N backticks to close
    let mut inline_backtick_count: usize = 0;

    while i < len {
        // Track fenced code blocks (3+ backticks at line start or after whitespace)
        if inline_backtick_count == 0
            && i + 2 < len
            && bytes[i] == b'`'
            && bytes[i + 1] == b'`'
            && bytes[i + 2] == b'`'
        {
            // Count consecutive backticks
            let fence_start = i;
            while i < len && bytes[i] == b'`' {
                i += 1;
            }
            result.push_str(&text[fence_start..i]);
            in_fenced_code = !in_fenced_code;
            continue;
        }

        // Track inline code spans (1 or 2 backticks)
        if !in_fenced_code && bytes[i] == b'`' {
            let bt_start = i;
            let mut bt_count = 0;
            while i < len && bytes[i] == b'`' && bt_count < 3 {
                bt_count += 1;
                i += 1;
            }
            // 3+ backticks handled above; here we have 1-2 backticks
            result.push_str(&text[bt_start..i]);
            if inline_backtick_count == 0 {
                // Opening: need matching count to close
                inline_backtick_count = bt_count;
            } else if bt_count == inline_backtick_count {
                // Closing: matched
                inline_backtick_count = 0;
            }
            // else: different count inside span, just content
            continue;
        }

        // Inside code — pass through unchanged
        if in_fenced_code || inline_backtick_count > 0 {
            // Fast: copy to next backtick or end
            let start = i;
            while i < len && bytes[i] != b'`' {
                i += 1;
            }
            result.push_str(&text[start..i]);
            continue;
        }

        // Outside code — check for tags to strip
        if bytes[i] == b'<' {
            // Try self-closing: <task-complete /> or <task-complete/>
            if let Some(end) = try_match_self_closing_bytes(bytes, i, b"task-complete") {
                i = end;
                continue;
            }

            // Try paired tags
            let mut matched = false;
            for tag in THINKING_TAGS.iter().chain(OTHER_STRIP_TAGS.iter()) {
                if let Some(end) = try_skip_paired_tag_bytes(bytes, i, tag.as_bytes()) {
                    i = end;
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
        }

        // Copy one UTF-8 character (may be multi-byte)
        let ch_len = utf8_char_len(bytes[i]);
        let end = (i + ch_len).min(len);
        result.push_str(&text[i..end]);
        i = end;
    }

    result
}

/// Try to match `<tag_name>…</tag_name>` at byte position `pos`.
/// Returns the byte position after the closing tag, or None.
fn try_skip_paired_tag_bytes(bytes: &[u8], pos: usize, tag: &[u8]) -> Option<usize> {
    let open_len = 1 + tag.len() + 1; // < + tag + >
    if pos + open_len > bytes.len() || bytes[pos] != b'<' {
        return None;
    }
    // Match opening tag (case-insensitive)
    if !bytes[pos + 1..pos + 1 + tag.len()]
        .iter()
        .zip(tag.iter())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
    {
        return None;
    }
    if bytes[pos + 1 + tag.len()] != b'>' {
        return None;
    }

    // Find closing </tag>
    let close_tag_len = 2 + tag.len() + 1; // </ + tag + >
    let search_start = pos + open_len;
    for k in search_start..bytes.len().saturating_sub(close_tag_len - 1) {
        if bytes[k] == b'<'
            && bytes[k + 1] == b'/'
            && k + close_tag_len <= bytes.len()
            && bytes[k + 2..k + 2 + tag.len()]
                .iter()
                .zip(tag.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
            && bytes[k + 2 + tag.len()] == b'>'
        {
            return Some(k + close_tag_len);
        }
    }
    None // No closing tag — don't strip
}

/// Try to match `<tag_name/>` or `<tag_name />` at byte position `pos`.
fn try_match_self_closing_bytes(bytes: &[u8], pos: usize, tag: &[u8]) -> Option<usize> {
    if bytes[pos] != b'<' {
        return None;
    }
    let mut j = pos + 1;
    for &t in tag {
        if j >= bytes.len() || bytes[j].to_ascii_lowercase() != t {
            return None;
        }
        j += 1;
    }
    // Skip optional spaces
    while j < bytes.len() && bytes[j] == b' ' {
        j += 1;
    }
    // Must end with />
    if j + 1 < bytes.len() && bytes[j] == b'/' && bytes[j + 1] == b'>' {
        Some(j + 2)
    } else {
        None
    }
}

/// Length of a UTF-8 character from its leading byte.
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

/// Strip trailing incomplete directives that LLMs sometimes emit at stream end.
fn strip_trailing_incomplete(text: &str) -> String {
    let mut s = text.to_string();

    // Strip trailing "[[" or "[" (incomplete wiki-link / directive)
    while s.ends_with("[[") || s.ends_with('[') {
        s.pop();
    }

    // Strip trailing incomplete opening tag (e.g. "<completion-check" without ">")
    // Only strip if it looks like a tag start (<letter or </), not math like "< 10"
    if let Some(last_lt) = s.rfind('<') {
        let tail = &s[last_lt..];
        if !tail.contains('>') {
            let after_lt = tail.as_bytes().get(1);
            let looks_like_tag = after_lt.is_some_and(|&b| b.is_ascii_alphabetic() || b == b'/');
            if looks_like_tag {
                s.truncate(last_lt);
            }
        }
    }

    s
}

/// Split text into (reasoning, answer) by extracting `<think>…</think>` blocks.
///
/// Code-block aware: tags inside backtick spans or fenced code blocks are
/// preserved, preventing accidental extraction from example code.
pub(crate) fn split_reasoning(text: &str) -> (Option<String>, String) {
    static QUICK_PROBE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)<(?:think|thinking|thought|antthinking)[\s/>]").expect("quick probe regex")
    });

    if !QUICK_PROBE.is_match(text) {
        return (None, text.to_string());
    }

    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut reasoning_parts: Vec<String> = Vec::new();
    let mut answer = String::with_capacity(len);
    let mut i = 0;
    let mut in_fenced_code = false;
    let mut inline_backtick_count: usize = 0;

    while i < len {
        // Track fenced code blocks
        if inline_backtick_count == 0
            && i + 2 < len
            && bytes[i] == b'`'
            && bytes[i + 1] == b'`'
            && bytes[i + 2] == b'`'
        {
            let fence_start = i;
            while i < len && bytes[i] == b'`' {
                i += 1;
            }
            answer.push_str(&text[fence_start..i]);
            in_fenced_code = !in_fenced_code;
            continue;
        }

        // Track inline code spans
        if !in_fenced_code && bytes[i] == b'`' {
            let bt_start = i;
            let mut bt_count = 0;
            while i < len && bytes[i] == b'`' && bt_count < 3 {
                bt_count += 1;
                i += 1;
            }
            answer.push_str(&text[bt_start..i]);
            if inline_backtick_count == 0 {
                inline_backtick_count = bt_count;
            } else if bt_count == inline_backtick_count {
                inline_backtick_count = 0;
            }
            continue;
        }

        // Inside code — pass through to answer
        if in_fenced_code || inline_backtick_count > 0 {
            let start = i;
            while i < len && bytes[i] != b'`' {
                i += 1;
            }
            answer.push_str(&text[start..i]);
            continue;
        }

        // Outside code — check for thinking tags to extract
        if bytes[i] == b'<' {
            for tag in THINKING_TAGS.iter() {
                if let Some((content, end)) = try_extract_paired_tag_bytes(bytes, i, tag.as_bytes())
                {
                    if !content.is_empty() {
                        reasoning_parts.push(content);
                    }
                    i = end;
                    break;
                }
            }
            // If we matched a tag, continue; otherwise copy the '<'
            if i < len && bytes[i] == b'<' {
                // No match — copy '<' and continue normally
                answer.push('<');
                i += 1;
                continue;
            }
            if i >= len {
                break;
            }
            continue;
        }

        // Copy one UTF-8 character
        let ch_len = utf8_char_len(bytes[i]);
        let end = (i + ch_len).min(len);
        answer.push_str(&text[i..end]);
        i = end;
    }

    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join("\n\n"))
    };

    (reasoning, answer)
}

/// Try to extract `<tag_name>…</tag_name>` at byte position `pos`.
/// Returns the extracted content and the byte position after the closing tag.
fn try_extract_paired_tag_bytes(bytes: &[u8], pos: usize, tag: &[u8]) -> Option<(String, usize)> {
    let open_len = 1 + tag.len() + 1; // < + tag + >
    if pos + open_len > bytes.len() || bytes[pos] != b'<' {
        return None;
    }
    // Match opening tag (case-insensitive)
    if !bytes[pos + 1..pos + 1 + tag.len()]
        .iter()
        .zip(tag.iter())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
    {
        return None;
    }
    if bytes[pos + 1 + tag.len()] != b'>' {
        return None;
    }

    // Find closing </tag>
    let close_tag_len = 2 + tag.len() + 1; // </ + tag + >
    let search_start = pos + open_len;
    for k in search_start..bytes.len().saturating_sub(close_tag_len - 1) {
        if bytes[k] == b'<'
            && bytes[k + 1] == b'/'
            && k + close_tag_len <= bytes.len()
            && bytes[k + 2..k + 2 + tag.len()]
                .iter()
                .zip(tag.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
            && bytes[k + 2 + tag.len()] == b'>'
        {
            let content = String::from_utf8_lossy(&bytes[search_start..k])
                .trim()
                .to_string();
            return Some((content, k + close_tag_len));
        }
    }
    None
}

/// Configuration for ReplyEmitter behavior
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
        }
    }
}

impl ReplyEmitterConfig {
    /// Create config from output_mode string ("typewriter" or "instant")
    pub fn from_output_mode(mode: &str) -> Self {
        Self {
            stream_enabled: mode == "typewriter",
            ..Default::default()
        }
    }
}

/// Routes Agent output back to the originating channel/conversation.
///
/// In streaming mode, tokens are pushed to a `StreamingController` as they
/// arrive. Once the character threshold is reached, an initial message is sent
/// and subsequent edits are debounced (300ms intervals) for real-time streaming.
///
/// In instant mode, the full response is sent as one message on completion.
pub struct ReplyEmitter {
    channel_registry: Arc<ChannelRegistry>,
    route: ReplyRoute,
    config: ReplyEmitterConfig,

    /// Accumulated response text (both modes)
    buffer: Mutex<String>,

    seq_counter: AtomicU64,
    has_sent: AtomicBool,
    /// Guard against duplicate RunComplete events (orchestrator drain + engine.rs both emit).
    run_complete_handled: AtomicBool,
    run_id: String,

    /// Cancellation token to stop the persistent typing indicator task
    typing_cancel: CancellationToken,

    /// Generation provider registry for TTS (voice mode)
    generation_registry: Option<Arc<crate::generation::GenerationProviderRegistry>>,
    /// Generation config for TTS (voice mode)
    generation_config:
        Option<Arc<tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>>>,
    /// Per-channel voice state
    voice_state: Mutex<super::voice::VoiceState>,

    /// Pending media items from tool outputs (shared with StreamCallback)
    pending_media: PendingMedia,
    /// Media cache for downloading media items
    media_cache: MediaCache,
    /// Real-time streaming state machine (replaces old typewriter mode)
    streaming: Mutex<StreamingController>,

    /// Native stream handler (if channel supports StreamProtocol::Native)
    native_handler: Option<Arc<dyn crate::gateway::channel::NativeStreamHandler>>,
    /// Active native stream state
    native_stream_state: Mutex<Option<NativeStreamState>>,
    /// Whether native streaming disabled due to error
    native_disabled: AtomicBool,

    /// Fallback model info — stored when ModelResolved fires with is_fallback=true.
    /// Appended as a notice line to non-Panel channel replies.
    fallback_info: Mutex<Option<crate::providers::health::ModelInfo>>,

    /// Accumulated reasoning text from StreamEvent::Reasoning / ReasoningBlock.
    reasoning_buffer: Mutex<String>,
}

/// Tracks the state of an active native stream (e.g., Teams streaming info).
struct NativeStreamState {
    stream_id: String,
    sequence: u32,
    last_update: std::time::Instant,
}

impl ReplyEmitter {
    /// Create a new ReplyEmitter with default configuration (instant mode)
    pub fn new(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
        pending_media: PendingMedia,
    ) -> Self {
        Self {
            channel_registry,
            route,
            config: ReplyEmitterConfig::default(),
            buffer: Mutex::new(String::new()),
            seq_counter: AtomicU64::new(0),
            has_sent: AtomicBool::new(false),
            run_complete_handled: AtomicBool::new(false),
            run_id,
            typing_cancel: CancellationToken::new(),
            generation_registry: None,
            generation_config: None,
            voice_state: Mutex::new(Default::default()),
            pending_media,
            media_cache: MediaCache::new(),
            streaming: Mutex::new(StreamingController::new(StreamingConfig {
                min_initial_chars: 30,
                debounce_interval: Duration::from_millis(300),
                enabled: false,
            })),
            native_handler: None,
            native_stream_state: Mutex::new(None),
            native_disabled: AtomicBool::new(false),
            fallback_info: Mutex::new(None),
            reasoning_buffer: Mutex::new(String::new()),
        }
    }

    /// Create a new ReplyEmitter with custom configuration
    pub fn with_config(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
        config: ReplyEmitterConfig,
        pending_media: PendingMedia,
    ) -> Self {
        let stream_enabled = config.stream_enabled;
        let debounce_ms = config.debounce_ms;
        let min_initial_chars = config.min_initial_chars;
        Self {
            channel_registry,
            route,
            config,
            buffer: Mutex::new(String::new()),
            seq_counter: AtomicU64::new(0),
            has_sent: AtomicBool::new(false),
            run_complete_handled: AtomicBool::new(false),
            run_id,
            typing_cancel: CancellationToken::new(),
            generation_registry: None,
            generation_config: None,
            voice_state: Mutex::new(Default::default()),
            pending_media,
            media_cache: MediaCache::new(),
            streaming: Mutex::new(StreamingController::new(StreamingConfig {
                min_initial_chars,
                debounce_interval: Duration::from_millis(debounce_ms),
                enabled: stream_enabled,
            })),
            native_handler: None,
            native_stream_state: Mutex::new(None),
            native_disabled: AtomicBool::new(false),
            fallback_info: Mutex::new(None),
            reasoning_buffer: Mutex::new(String::new()),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn route(&self) -> &ReplyRoute {
        &self.route
    }

    /// Overflow threshold in characters. Returns 0 when overflow detection
    /// is disabled (channel has no max_message_length or streaming is off).
    /// Subtracts a safety margin (~300) for HTML tag overhead.
    fn overflow_threshold(&self) -> usize {
        let max = self.config.max_message_length;
        if max == 0 {
            return 0;
        }
        max.saturating_sub(300)
    }

    /// Attach voice mode dependencies, enabling TTS output.
    pub fn with_voice(
        mut self,
        voice_state: super::voice::VoiceState,
        generation_registry: Arc<crate::generation::GenerationProviderRegistry>,
        generation_config: Arc<
            tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>,
        >,
    ) -> Self {
        self.voice_state = Mutex::new(voice_state);
        self.generation_registry = Some(generation_registry);
        self.generation_config = Some(generation_config);
        self
    }

    /// Attach a native stream handler, enabling real-time streaming for
    /// channels that support `StreamProtocol::Native` (e.g., Microsoft Teams).
    pub fn with_native_handler(
        mut self,
        handler: Arc<dyn crate::gateway::channel::NativeStreamHandler>,
    ) -> Self {
        self.native_handler = Some(handler);
        self
    }

    /// Returns true when voice output should be attempted.
    /// Dynamically check if voice output should be used.
    ///
    /// Re-reads VoiceState from ChannelRegistry on every call so that
    /// mid-request voice_mode_set tool calls take effect immediately
    /// (e.g. the confirmation message itself is voiced).
    async fn should_voice(&self) -> bool {
        // Static hint: user sent an audio message
        if self.config.voice_reply_hint {
            debug!("should_voice=true (voice_reply_hint)");
            return true;
        }
        // Dynamic check: read current voice state from registry
        // Check both the specific channel and "default" (fallback when
        // voice_mode_set tool is called without explicit channel_id)
        let channel_id = self.route.channel_id.as_str();
        let live_state = self.channel_registry.get_voice_state(channel_id).await;
        debug!(
            "should_voice: channel={}, enabled={}",
            channel_id,
            live_state.is_active()
        );
        if live_state.is_active() {
            return true;
        }
        // Fallback: check "default" key (used when tool doesn't know channel)
        if channel_id != "default" {
            let default_state = self.channel_registry.get_voice_state("default").await;
            debug!(
                "should_voice: fallback 'default' enabled={}, has_gen_registry={}",
                default_state.is_active(),
                self.generation_registry.is_some()
            );
            if default_state.is_active() {
                debug!("should_voice => TRUE (default fallback)");
                return true;
            }
        }
        debug!("should_voice => FALSE");
        false
    }

    /// Try to generate TTS audio and send as a voice message.
    ///
    /// On success the message includes both text and an audio attachment.
    /// On failure it records the failure on `voice_state` and falls back to
    /// plain text via `send_to_channel`.
    async fn send_as_voice(&self, text: &str) {
        let sanitized = sanitize_llm_output(text);
        let text: &str = &sanitized;
        if text.is_empty() {
            return;
        }
        debug!(
            "send_as_voice called, text_len={}, has_gen_registry={}",
            text.len(),
            self.generation_registry.is_some()
        );
        if let Some(ref registry) = self.generation_registry {
            // Read live voice state from registry (not the stale local copy)
            // Check channel-specific first, then "default" fallback
            let channel_id = self.route.channel_id.as_str();
            let mut voice_state = self.channel_registry.get_voice_state(channel_id).await;
            if !voice_state.is_active() && channel_id != "default" {
                voice_state = self.channel_registry.get_voice_state("default").await;
            }
            let gen_config = match self.generation_config {
                Some(ref cfg) => cfg.read().await.clone(),
                None => {
                    self.send_to_channel(text).await;
                    return;
                }
            };

            if let Some(attachment) =
                super::voice::outbound::generate_tts(text, &voice_state, registry, &gen_config)
                    .await
            {
                // Success — reset failure counter
                self.voice_state.lock().await.record_success();

                // Send voice-only message (no text — voice replaces text)
                let message = OutboundMessage {
                    conversation_id: self.route.conversation_id.clone(),
                    text: String::new(),
                    attachments: vec![attachment],
                    reply_to: self.route.reply_to.clone(),
                    inline_keyboard: None,
                    metadata: Default::default(),
                };

                match self
                    .channel_registry
                    .send(&self.route.channel_id, message)
                    .await
                {
                    Ok(result) => {
                        debug!(
                            "Sent voice reply to channel {} (message_id: {})",
                            self.route.channel_id,
                            result.message_id.as_str(),
                        );
                        self.has_sent.store(true, Ordering::SeqCst);
                    }
                    Err(e) => {
                        error!("Failed to send voice reply: {}", e);
                        // Fall back to text
                        self.send_to_channel(text).await;
                    }
                }
            } else {
                // TTS generation failed — record and maybe auto-disable
                let auto_disabled = self.voice_state.lock().await.record_failure();
                if auto_disabled {
                    warn!(
                        "Voice auto-disabled for channel {} after 3 consecutive TTS failures",
                        self.route.channel_id
                    );
                }
                // Fallback to plain text
                self.send_to_channel(text).await;
            }
        } else {
            // No generation registry — fall back to text
            self.send_to_channel(text).await;
        }

        let media_attachments = self.drain_and_send_media().await;
        self.send_media_standalone(media_attachments).await;
    }

    /// Drain accumulated explicit reasoning text, returning None if empty.
    async fn take_reasoning_buffer(&self) -> Option<String> {
        let mut buf = self.reasoning_buffer.lock().await;
        let content = std::mem::take(&mut *buf);
        if content.is_empty() {
            None
        } else {
            Some(content)
        }
    }

    /// Drain pending media, download in parallel via MediaCache, return Attachments.
    async fn drain_and_send_media(&self) -> Vec<crate::gateway::channel::Attachment> {
        let pending_count = self
            .pending_media
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        debug!(run_id = %self.run_id, pending_count = pending_count, "drain_and_send_media called");
        let media_items =
            std::mem::take(&mut *self.pending_media.lock().unwrap_or_else(|e| e.into_inner()));
        if media_items.is_empty() {
            return vec![];
        }

        info!(
            run_id = %self.run_id,
            count = media_items.len(),
            urls = ?media_items.iter().map(|i| &i.url).collect::<Vec<_>>(),
            "Draining pending media for download"
        );

        let attachments = futures::future::join_all(
            media_items
                .iter()
                .map(|item| self.media_cache.download_media_item(item, &self.run_id)),
        )
        .await;

        for att in &attachments {
            info!(
                run_id = %self.run_id,
                mime = %att.mime_type,
                has_path = att.path.is_some(),
                has_url = att.url.is_some(),
                size = ?att.size,
                "Media attachment ready"
            );
        }

        attachments
    }

    /// Send media as a separate standalone message.
    async fn send_media_standalone(&self, attachments: Vec<crate::gateway::channel::Attachment>) {
        if attachments.is_empty() {
            return;
        }
        let count = attachments.len();
        info!(
            run_id = %self.run_id,
            channel = %self.route.channel_id,
            count = count,
            "Sending media standalone message"
        );
        let message = OutboundMessage {
            conversation_id: self.route.conversation_id.clone(),
            text: String::new(),
            attachments,
            reply_to: None,
            inline_keyboard: None,
            metadata: Default::default(),
        };
        match self
            .channel_registry
            .send(&self.route.channel_id, message)
            .await
        {
            Ok(_) => {
                info!(run_id = %self.run_id, count = count, "Media standalone message sent successfully")
            }
            Err(e) => warn!(error = %e, "Failed to send media standalone message"),
        }
    }

    fn format_content(&self, content: &str, _is_first: bool) -> String {
        content.to_string()
    }

    // ── Shared helpers ──────────────────────────────────────────────────

    const MAX_MESSAGE_LENGTH: usize = 4000;

    /// Send content to the channel, sanitizing first and splitting into chunks.
    async fn send_to_channel(&self, content: &str) {
        if content.is_empty() {
            return;
        }

        let content = sanitize_llm_output(content);
        self.send_to_channel_sanitized(&content, None).await;
    }

    /// Send content to the channel, extracting embedded reasoning, sanitizing,
    /// and splitting into chunks if too long.
    async fn send_to_channel_with_reasoning(&self, content: &str, reasoning: Option<&str>) {
        if content.is_empty() && reasoning.is_none_or(|r| r.is_empty()) {
            return;
        }

        let (embedded_reasoning, answer) = split_reasoning(content);
        let final_reasoning = match (reasoning, embedded_reasoning) {
            (Some(r), Some(e)) if !r.is_empty() && !e.is_empty() => Some(format!("{}\n\n{}", r, e)),
            (Some(r), _) if !r.is_empty() => Some(r.to_string()),
            (None, Some(e)) => Some(e),
            _ => None,
        };

        let sanitized = sanitize_llm_output(&answer);
        self.send_to_channel_sanitized(&sanitized, final_reasoning.as_deref())
            .await;
    }

    /// Send pre-sanitized content to the channel, splitting into chunks if too long.
    ///
    /// Callers that have already called `sanitize_llm_output` should use this
    /// to avoid redundant sanitization passes.
    async fn send_to_channel_sanitized(&self, content: &str, reasoning: Option<&str>) {
        if content.is_empty() && reasoning.is_none_or(|r| r.is_empty()) {
            return;
        }

        let is_first_send = !self.has_sent.load(Ordering::SeqCst);
        self.has_sent.store(true, Ordering::SeqCst);

        let content = self.format_content(content, is_first_send);

        let chunks = Self::split_message(&content, Self::MAX_MESSAGE_LENGTH);
        let total_chunks = chunks.len();

        let metadata = reasoning
            .filter(|r| !r.is_empty())
            .map(|r| {
                let mut m = std::collections::HashMap::new();
                m.insert("reasoning".to_string(), r.to_string());
                m
            })
            .unwrap_or_default();

        for (i, chunk) in chunks.into_iter().enumerate() {
            let message = OutboundMessage {
                conversation_id: self.route.conversation_id.clone(),
                text: chunk,
                attachments: vec![],
                reply_to: if i == 0 {
                    self.route.reply_to.clone()
                } else {
                    None
                },
                inline_keyboard: None,
                metadata: metadata.clone(),
            };

            match self
                .channel_registry
                .send(&self.route.channel_id, message)
                .await
            {
                Ok(result) => {
                    debug!(
                        "Sent reply to channel {} (message_id: {}, chunk {}/{})",
                        self.route.channel_id,
                        result.message_id.as_str(),
                        i + 1,
                        total_chunks
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to send reply to channel {} (chunk {}/{}): {}",
                        self.route.channel_id,
                        i + 1,
                        total_chunks,
                        e
                    );
                    break;
                }
            }
        }

        let media_attachments = self.drain_and_send_media().await;
        self.send_media_standalone(media_attachments).await;
    }

    fn split_message(content: &str, max_len: usize) -> Vec<String> {
        if content.len() <= max_len {
            return vec![content.to_string()];
        }

        let mut chunks = Vec::new();
        let mut buf = content.to_string();
        let mut open_fence: Option<String> = None; // e.g. "```rust"

        while !buf.is_empty() {
            if buf.len() <= max_len {
                chunks.push(buf);
                break;
            }

            let split_at = Self::find_split_point(&buf, max_len);
            let chunk_raw = buf[..split_at].trim_end().to_string();
            let rest = buf[split_at..].trim_start_matches('\n').to_string();

            // Track code fence state through this chunk
            for line in chunk_raw.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("```") {
                    if open_fence.is_some() {
                        open_fence = None;
                    } else {
                        open_fence = Some(trimmed.to_string());
                    }
                }
            }

            // If we're splitting inside an open code block, close + reopen
            if let Some(ref fence) = open_fence {
                let mut chunk_closed = chunk_raw;
                chunk_closed.push_str("\n```");
                chunks.push(chunk_closed);
                buf = format!("{}\n{}", fence, rest);
            } else {
                chunks.push(chunk_raw);
                buf = rest;
            }
        }

        chunks
    }

    fn find_split_point(text: &str, max_len: usize) -> usize {
        let mut safe_max = max_len;
        while safe_max > 0 && !text.is_char_boundary(safe_max) {
            safe_max -= 1;
        }

        let search_range = &text[..safe_max];

        // Check if we're inside a code block (odd number of ``` before split point)
        let fence_count = search_range.matches("```").count();
        if fence_count % 2 == 1 {
            // Inside a code block — try to split before the opening fence
            if let Some(last_fence) = search_range.rfind("```") {
                if last_fence > 0 {
                    if let Some(nl) = text[..last_fence].rfind('\n') {
                        return nl + 1;
                    }
                    return last_fence;
                }
            }
        }

        // Check for partial HTML entities at the split point
        let entity_start = search_range[..safe_max].rfind('&');
        if let Some(amp_pos) = entity_start {
            let after_amp = &search_range[amp_pos..safe_max];
            if !after_amp.contains(';') && after_amp.len() <= 8 {
                safe_max = amp_pos;
            }
        }

        // Existing logic: prefer paragraph boundary, then newline
        let search_range = &text[..safe_max];
        if let Some(pos) = search_range.rfind("\n\n") {
            if pos > safe_max / 4 {
                return pos + 1;
            }
        }
        if let Some(pos) = search_range.rfind('\n') {
            if pos > safe_max / 4 {
                return pos + 1;
            }
        }
        safe_max
    }

    async fn send_error(&self, error: &str) {
        let error_message = format!("Error: {}", error);
        self.send_to_channel(&error_message).await;
    }

    /// Spawn a background task that sends typing indicators every 4 seconds
    /// until the cancellation token is triggered.
    fn start_typing_indicator(&self) {
        let registry = self.channel_registry.clone();
        let channel_id = self.route.channel_id.clone();
        let conversation_id = self.route.conversation_id.clone();
        let cancel = self.typing_cancel.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(4));
            interval.tick().await; // Skip first immediate tick
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let _ = registry.send_typing(&channel_id, &conversation_id).await;
                    }
                    _ = cancel.cancelled() => {
                        break;
                    }
                }
            }
        });
    }

    /// React on the original inbound message (best-effort, non-blocking)
    async fn react_on_inbound(&self, emoji: &str) {
        if let Some(ref msg_id) = self.route.inbound_message_id {
            tracing::info!(
                target: "multimodal",
                probe = "P7_reaction",
                run_id = %self.run_id,
                emoji = %emoji,
                message_id = %msg_id.as_str(),
                "Processing status reaction"
            );
            let _ = self
                .channel_registry
                .react(
                    &self.route.channel_id,
                    &self.route.conversation_id,
                    msg_id,
                    emoji,
                )
                .await;
        }
    }
}

impl Drop for ReplyEmitter {
    fn drop(&mut self) {
        self.typing_cancel.cancel();
    }
}

#[async_trait]
impl EventEmitter for ReplyEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        match event {
            StreamEvent::ResponseChunk {
                content,
                is_final,
                is_intermediate,
                ..
            } => {
                if is_intermediate {
                    if content.is_empty() {
                        // Intermediate boundary marker from DeltaSink: the LLM
                        // finished one iteration (tool use) and will continue.
                        if self.config.stream_enabled {
                            // Voice mode: streaming was skipped, buffer holds
                            // the text for RunComplete to convert to TTS.
                            // Do NOT finalize or clear — just reset controller.
                            if self.should_voice().await {
                                let mut ctrl = self.streaming.lock().await;
                                ctrl.reset();
                            } else {
                                // Streaming mode: the text was already delivered via
                                // real-time edits.  Finalize the current streamed
                                // message (perform any pending edit), then reset the
                                // controller so the next iteration starts a fresh
                                // message.  Do NOT send a new standalone message —
                                // the text is already visible to the user.
                                let mut ctrl = self.streaming.lock().await;
                                match ctrl.finalize() {
                                    StreamAction::EditFinal(text) => {
                                        let text = sanitize_llm_output(&text);
                                        if let Some(msg_id) = ctrl.message_id().cloned() {
                                            let _ = self
                                                .channel_registry
                                                .edit(
                                                    &self.route.channel_id,
                                                    &self.route.conversation_id,
                                                    &msg_id,
                                                    &text,
                                                )
                                                .await;
                                        }
                                    }
                                    StreamAction::SendFinal(text) => {
                                        // Rare: text never reached initial threshold.
                                        // Send it now as a new message.
                                        drop(ctrl);
                                        self.send_to_channel(&text).await;
                                        // Re-acquire for reset below
                                        let mut ctrl = self.streaming.lock().await;
                                        ctrl.reset();
                                        let mut buffer = self.buffer.lock().await;
                                        let _ = std::mem::take(&mut *buffer);
                                        return Ok(());
                                    }
                                    _ => {}
                                }
                                ctrl.reset();
                                drop(ctrl);
                                // Clear buffer (text was already streamed)
                                let mut buffer = self.buffer.lock().await;
                                let _ = std::mem::take(&mut *buffer);
                            } // end else (non-voice)
                        } else {
                            // Non-streaming: flush buffer as standalone message
                            let mut buffer = self.buffer.lock().await;
                            let accumulated = std::mem::take(&mut *buffer);
                            drop(buffer);
                            if !accumulated.is_empty() {
                                if self.should_voice().await {
                                    self.send_as_voice(&accumulated).await;
                                } else {
                                    self.send_to_channel(&accumulated).await;
                                }
                            }
                        }
                    } else {
                        // Non-empty intermediate: send immediately as standalone message
                        if self.should_voice().await {
                            self.send_as_voice(&content).await;
                        } else {
                            self.send_to_channel(&content).await;
                        }
                    }
                    // Do NOT buffer — this is a separate message from the final response
                } else {
                    // React with 👀 on first non-intermediate chunk and start typing indicator
                    if !self.has_sent.load(Ordering::SeqCst) && !content.is_empty() {
                        self.react_on_inbound("👀").await;
                        self.start_typing_indicator();
                    }

                    // Existing behavior: accumulate text into buffer
                    if !content.is_empty() {
                        self.buffer.lock().await.push_str(&content);
                    }

                    // Real-time streaming: push chunks to controller and act
                    // Skip streaming text when voice mode is active — buffer only,
                    // voice reply will be sent on RunComplete.
                    if self.config.stream_enabled
                        && !content.is_empty()
                        && !self.should_voice().await
                    {
                        let mut ctrl = self.streaming.lock().await;
                        ctrl.push_chunk(&content);
                        match ctrl.poll_action() {
                            StreamAction::SendInitial(text) => {
                                let mut text = sanitize_llm_output(&text).into_owned();
                                text.push_str(STREAMING_CURSOR);
                                let message = OutboundMessage {
                                    conversation_id: self.route.conversation_id.clone(),
                                    text,
                                    attachments: vec![],
                                    reply_to: self.route.reply_to.clone(),
                                    inline_keyboard: None,
                                    metadata: Default::default(),
                                };
                                if let Ok(result) = self
                                    .channel_registry
                                    .send(&self.route.channel_id, message)
                                    .await
                                {
                                    ctrl.record_sent(result.message_id);
                                    self.has_sent.store(true, Ordering::SeqCst);
                                    self.typing_cancel.cancel();
                                }
                            }
                            StreamAction::Edit(text) => {
                                let text = sanitize_llm_output(&text).into_owned();
                                let overflow_threshold = self.overflow_threshold();
                                if overflow_threshold > 0
                                    && text.chars().count() > overflow_threshold
                                {
                                    // Overflow: finalize current message, start new one
                                    let char_boundary = text
                                        .char_indices()
                                        .nth(overflow_threshold)
                                        .map(|(i, _)| i)
                                        .unwrap_or(text.len());
                                    let (head, tail) = text.split_at(char_boundary);

                                    // Edit current message with head (clean, no cursor)
                                    if let Some(msg_id) = ctrl.message_id() {
                                        let msg_id = msg_id.clone();
                                        let _ = self
                                            .channel_registry
                                            .edit(
                                                &self.route.channel_id,
                                                &self.route.conversation_id,
                                                &msg_id,
                                                head,
                                            )
                                            .await;
                                    }

                                    // Reset controller and push overflow text back
                                    ctrl.reset();
                                    ctrl.push_chunk(tail);

                                    // Send new message with overflow + cursor
                                    let mut overflow_text = tail.to_string();
                                    overflow_text.push_str(STREAMING_CURSOR);
                                    let message = OutboundMessage {
                                        conversation_id: self.route.conversation_id.clone(),
                                        text: overflow_text,
                                        attachments: vec![],
                                        reply_to: None,
                                        inline_keyboard: None,
                                        metadata: Default::default(),
                                    };
                                    if let Ok(result) = self
                                        .channel_registry
                                        .send(&self.route.channel_id, message)
                                        .await
                                    {
                                        ctrl.record_sent(result.message_id);
                                    }
                                } else {
                                    // Normal edit with cursor
                                    let mut text_with_cursor = text;
                                    text_with_cursor.push_str(STREAMING_CURSOR);
                                    if let Some(msg_id) = ctrl.message_id() {
                                        let msg_id = msg_id.clone();
                                        if self
                                            .channel_registry
                                            .edit(
                                                &self.route.channel_id,
                                                &self.route.conversation_id,
                                                &msg_id,
                                                &text_with_cursor,
                                            )
                                            .await
                                            .is_ok()
                                        {
                                            ctrl.record_edit();
                                        }
                                    }
                                }
                            }
                            StreamAction::Wait => {}
                            _ => {}
                        }
                    }

                    // Native streaming: forward chunks in real-time
                    if !self.native_disabled.load(Ordering::SeqCst) {
                        if let Some(ref handler) = self.native_handler {
                            let accumulated = {
                                let buffer = self.buffer.lock().await;
                                sanitize_llm_output(&buffer).into_owned()
                            };

                            let mut state = self.native_stream_state.lock().await;
                            if state.is_none() && accumulated.chars().count() >= 20 {
                                // First chunk with enough content: start streaming
                                let status =
                                    crate::gateway::interfaces::msteams::types::pick_status_text();
                                match handler
                                    .stream_start(&self.route.conversation_id, status)
                                    .await
                                {
                                    Ok(stream_id) => {
                                        *state = Some(NativeStreamState {
                                            stream_id,
                                            sequence: 0,
                                            last_update: std::time::Instant::now(),
                                        });
                                    }
                                    Err(e) => {
                                        warn!("Native stream_start failed, falling back: {}", e);
                                        self.native_disabled.store(true, Ordering::SeqCst);
                                    }
                                }
                            } else if let Some(ref mut s) = *state {
                                // Throttle updates at 1500ms
                                if s.last_update.elapsed() >= std::time::Duration::from_millis(1500)
                                {
                                    s.sequence += 1;
                                    if let Err(e) = handler
                                        .stream_update(
                                            &self.route.conversation_id,
                                            &s.stream_id,
                                            &accumulated,
                                            s.sequence,
                                        )
                                        .await
                                    {
                                        warn!("Native stream_update failed: {}", e);
                                        // Don't disable — try to finalize later
                                    }
                                    s.last_update = std::time::Instant::now();
                                }
                            }
                        }
                    }

                    // Instant mode: flush on final chunk (skip if native streaming active)
                    if !self.config.stream_enabled && is_final && self.native_handler.is_none() {
                        let mut buffer = self.buffer.lock().await;
                        if !buffer.is_empty() {
                            let text = std::mem::take(&mut *buffer);
                            drop(buffer);
                            let reasoning = self.take_reasoning_buffer().await;
                            if self.should_voice().await {
                                self.send_as_voice(&text).await;
                            } else {
                                self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                                    .await;
                            }
                        }
                    }
                    // Typewriter mode: do nothing here, wait for RunComplete
                }
            }

            StreamEvent::RunComplete { summary, .. } => {
                // Guard against duplicate RunComplete (orchestrator drain + engine.rs both emit).
                if self.run_complete_handled.swap(true, Ordering::SeqCst) {
                    tracing::debug!(run_id = %self.run_id, "Ignoring duplicate RunComplete");
                    return Ok(());
                }

                // Stop the persistent typing indicator
                self.typing_cancel.cancel();

                // Finalize native stream if active
                if !self.native_disabled.load(Ordering::SeqCst) {
                    if let Some(ref handler) = self.native_handler {
                        let state = self.native_stream_state.lock().await.take();
                        if let Some(s) = state {
                            let text = {
                                let mut buffer = self.buffer.lock().await;
                                let raw = std::mem::take(&mut *buffer);
                                sanitize_llm_output(&raw).into_owned()
                            };
                            if !text.is_empty() {
                                let message = OutboundMessage::text(
                                    self.route.conversation_id.as_str(),
                                    &text,
                                );
                                match handler
                                    .stream_finalize(
                                        &self.route.conversation_id,
                                        &s.stream_id,
                                        message,
                                    )
                                    .await
                                {
                                    Ok(_result) => {
                                        self.has_sent.store(true, Ordering::SeqCst);
                                        // Stop typing, react success, clean up media — then return
                                        self.typing_cancel.cancel();
                                        self.react_on_inbound("\u{1f44d}").await;
                                        let _ = self.drain_and_send_media().await;
                                        return Ok(());
                                    }
                                    Err(e) => {
                                        warn!("Native stream_finalize failed, falling back: {}", e);
                                        // Re-fill buffer for normal path
                                        *self.buffer.lock().await = text;
                                    }
                                }
                            }
                        }
                    }
                }

                // Append fallback notice for non-Panel channels (Telegram, CLI, etc.)
                if let Some(info) = self.fallback_info.lock().await.take() {
                    let original = info.original_model.as_deref().unwrap_or("unknown");
                    let notice = format!(
                        "\n\n\u{26a1} {} \u{2192} {} ({})",
                        original, info.model, info.provider,
                    );
                    self.buffer.lock().await.push_str(&notice);
                }

                // Flush accumulated buffer (always flush — intermediate messages
                // may have set has_sent, but the buffer holds the final response)
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                let reasoning = self.take_reasoning_buffer().await;

                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else if self.config.stream_enabled {
                        // Send explicit reasoning first if available
                        if let Some(ref r) = reasoning {
                            self.send_to_channel(&format!("🤔 {}", r)).await;
                        }
                        // Finalize the streaming controller
                        let mut ctrl = self.streaming.lock().await;
                        match ctrl.finalize() {
                            StreamAction::SendFinal(final_text) => {
                                drop(ctrl);
                                if self.should_voice().await {
                                    self.send_as_voice(&final_text).await;
                                } else {
                                    self.send_to_channel(&final_text).await;
                                }
                            }
                            StreamAction::EditFinal(final_text) => {
                                let final_text = sanitize_llm_output(&final_text);
                                let msg_id = ctrl.message_id().cloned();
                                drop(ctrl);
                                if let Some(msg_id) = msg_id {
                                    let _ = self
                                        .channel_registry
                                        .edit(
                                            &self.route.channel_id,
                                            &self.route.conversation_id,
                                            &msg_id,
                                            &final_text,
                                        )
                                        .await;
                                }
                            }
                            StreamAction::Done => {
                                drop(ctrl);
                            }
                            _ => {
                                drop(ctrl);
                            }
                        }
                        let media = self.drain_and_send_media().await;
                        self.send_media_standalone(media).await;
                    } else {
                        self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                            .await;
                    }
                }

                // Fallback: if buffer was empty (race with fire-and-forget emit),
                // use final_response from summary
                if text.is_empty() {
                    if let Some(ref final_response) = summary.final_response {
                        if !final_response.is_empty() {
                            debug!(
                                "Run {} complete, sending final_response as fallback (length: {})",
                                self.run_id,
                                final_response.len()
                            );
                            if self.should_voice().await {
                                self.send_as_voice(final_response).await;
                            } else {
                                self.send_to_channel_with_reasoning(
                                    final_response,
                                    reasoning.as_deref(),
                                )
                                .await;
                            }
                        }
                    }
                }

                // React with 👍 on successful completion
                self.react_on_inbound("👍").await;

                // Clean up media temp files for this run
                if let Err(e) = crate::media::cache::MediaCache::cleanup_session(&self.run_id) {
                    tracing::warn!(error = %e, "Failed to cleanup media session");
                }
            }

            StreamEvent::RunError { error, .. } => {
                // Stop the persistent typing indicator
                self.typing_cancel.cancel();

                // Flush any partial response
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                let reasoning = self.take_reasoning_buffer().await;
                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else {
                        self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                            .await;
                    }
                }

                warn!("Run {} failed: {}", self.run_id, error);
                self.send_error(&error).await;

                // React with 👎 on error
                self.react_on_inbound("👎").await;
            }

            StreamEvent::AskUser { question, .. } => {
                // Drain any pending media before sending the question
                let media_attachments = self.drain_and_send_media().await;
                self.send_media_standalone(media_attachments).await;

                // Flush buffer first
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                let reasoning = self.take_reasoning_buffer().await;
                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else {
                        self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                            .await;
                    }
                }

                if self.should_voice().await {
                    self.send_as_voice(&question).await;
                } else {
                    self.send_to_channel(&question).await;
                }
            }

            // Accumulate explicit reasoning chunks for separate delivery
            StreamEvent::Reasoning { content, .. } => {
                if !content.is_empty() {
                    let mut buf = self.reasoning_buffer.lock().await;
                    if !buf.is_empty() && !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                    buf.push_str(&content);
                }
            }

            StreamEvent::ReasoningBlock { content, .. } => {
                if !content.is_empty() {
                    let mut buf = self.reasoning_buffer.lock().await;
                    if !buf.is_empty() {
                        buf.push_str("\n\n");
                    }
                    buf.push_str(&content);
                }
            }

            // Store fallback info for non-Panel notification
            StreamEvent::ModelResolved { model_info, .. } => {
                if model_info.is_fallback {
                    *self.fallback_info.lock().await = Some(model_info);
                }
            }

            StreamEvent::ToolStart { tool_name, .. } => {
                self.react_on_inbound("🔧").await;
                debug!(target: "multimodal", probe = "P7_reaction", run_id = %self.run_id, tool = %tool_name, "Tool started — reaction set");
            }

            StreamEvent::ToolEnd { .. } => {
                // Tool finished — back to thinking state
                self.react_on_inbound("👀").await;
            }

            // Other events are not routed to channels
            StreamEvent::RunAccepted { .. }
            | StreamEvent::ToolUpdate { .. }
            | StreamEvent::AgentTrace { .. }
            | StreamEvent::UncertaintySignal { .. }
            | StreamEvent::SessionUpdated { .. } => {
                debug!("Ignoring event for channel routing: {:?}", event);
            }
        }

        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId};
    use crate::sync_primitives::Mutex;

    #[test]
    fn test_config_defaults() {
        let config = ReplyEmitterConfig::default();
        assert_eq!(config.buffer_threshold, 500);
        assert!(!config.stream_enabled);
    }

    #[test]
    fn test_config_from_output_mode() {
        let tw = ReplyEmitterConfig::from_output_mode("typewriter");
        assert!(tw.stream_enabled);

        let instant = ReplyEmitterConfig::from_output_mode("instant");
        assert!(!instant.stream_enabled);
    }

    #[test]
    fn test_reply_route() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(
            registry,
            route.clone(),
            "run-123".to_string(),
            Arc::new(Mutex::new(Vec::<crate::gateway::media::MediaItem>::new())),
        );

        assert_eq!(emitter.run_id(), "run-123");
        assert_eq!(emitter.route().channel_id.as_str(), "imessage");
        assert_eq!(emitter.route().conversation_id.as_str(), "+15551234567");
    }

    #[test]
    fn test_custom_config() {
        let route = ReplyRoute::new(ChannelId::new("telegram"), ConversationId::new("12345"));

        let config = ReplyEmitterConfig {
            buffer_threshold: 1000,
            stream_enabled: true,
            voice_enabled: false,
            voice_reply_hint: false,
            debounce_ms: 800,
            min_initial_chars: 30,
            max_message_length: 4096,
        };

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::with_config(
            registry,
            route,
            "run-456".to_string(),
            config,
            Arc::new(Mutex::new(Vec::<crate::gateway::media::MediaItem>::new())),
        );

        assert_eq!(emitter.config.buffer_threshold, 1000);
        assert!(emitter.config.stream_enabled);
    }

    #[tokio::test]
    async fn test_sequence_counter() {
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("conv-1"));

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(
            registry,
            route,
            "run-789".to_string(),
            Arc::new(Mutex::new(Vec::<crate::gateway::media::MediaItem>::new())),
        );

        assert_eq!(emitter.next_seq(), 0);
        assert_eq!(emitter.next_seq(), 1);
        assert_eq!(emitter.next_seq(), 2);
    }

    #[tokio::test]
    async fn test_buffer_accumulation() {
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("conv-1"));

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(
            registry,
            route,
            "run-test".to_string(),
            Arc::new(Mutex::new(Vec::<crate::gateway::media::MediaItem>::new())),
        );

        emitter.buffer.lock().await.push_str("Hello ");
        emitter.buffer.lock().await.push_str("World!");

        let buffer = emitter.buffer.lock().await;
        assert_eq!(*buffer, "Hello World!");
    }

    #[test]
    fn test_split_message_short() {
        let chunks = ReplyEmitter::split_message("Hello World", 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello World");
    }

    #[test]
    fn test_split_message_at_paragraph() {
        let content = format!("{}\n\n{}", "A".repeat(50), "B".repeat(50));
        let chunks = ReplyEmitter::split_message(&content, 60);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with('A'));
        assert!(chunks[1].starts_with('B'));
    }

    #[test]
    fn test_split_message_at_newline() {
        let content = format!("{}\n{}", "A".repeat(50), "B".repeat(50));
        let chunks = ReplyEmitter::split_message(&content, 60);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn test_split_message_no_boundary() {
        let content = "A".repeat(200);
        let chunks = ReplyEmitter::split_message(&content, 100);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 100);
        assert_eq!(chunks[1].len(), 100);
    }

    #[test]
    fn test_split_message_utf8_safe() {
        let content = "中".repeat(100);
        let chunks = ReplyEmitter::split_message(&content, 150);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 150);
        }
    }

    #[test]
    fn test_split_message_preserves_code_block() {
        let code = "x".repeat(100);
        let content = format!("Before text\n\n```rust\n{}\n```\n\nAfter text", code);
        let chunks = ReplyEmitter::split_message(&content, 80);
        for chunk in &chunks {
            let count = chunk.matches("```").count();
            assert!(
                count % 2 == 0 || count == 0,
                "Chunk has unbalanced code fence: {:?}",
                chunk
            );
        }
    }

    #[test]
    fn test_split_message_html_entity_safe() {
        let content = format!("{}a&amp;b{}", "x".repeat(95), "y".repeat(100));
        let chunks = ReplyEmitter::split_message(&content, 100);
        for chunk in &chunks {
            assert!(
                !chunk.ends_with('&') && !chunk.ends_with("&a") && !chunk.ends_with("&am"),
                "Chunk ends with partial HTML entity: {:?}",
                chunk
            );
        }
    }

    // ── sanitize_llm_output tests ──────────────────────────────────────

    #[test]
    fn sanitize_no_tags_returns_borrowed() {
        let input = "Hello, this is a normal response.";
        let result = sanitize_llm_output(input);
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*result, input);
    }

    #[test]
    fn sanitize_strips_think_block() {
        let input = "<think>\nLet me analyze this...\n</think>\nThe answer is 42.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "The answer is 42.");
    }

    #[test]
    fn sanitize_strips_completion_check() {
        let input = "Here is the result.\n<completion-check>\n• Task done\n</completion-check>";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Here is the result.");
    }

    #[test]
    fn sanitize_strips_task_complete() {
        let input = "Done.\n<task-complete/>";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Done.");
    }

    #[test]
    fn sanitize_strips_all_tags_combined() {
        let input = "<think>\nthinking...\n</think>\nHere is the answer.\n<completion-check>\n• ok\n</completion-check>\n<task-complete/>";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Here is the answer.");
    }

    #[test]
    fn sanitize_collapses_blank_lines() {
        let input = "<think>x</think>\n\n\n\nActual response.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Actual response.");
    }

    #[test]
    fn sanitize_self_closing_with_space() {
        let input = "Done.\n<task-complete />";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Done.");
    }

    // ── Code-block awareness tests ────────────────────────────────────

    #[test]
    fn sanitize_preserves_think_in_fenced_code() {
        let input = "Example:\n```\n<think>code here</think>\n```\nDone.";
        let result = sanitize_llm_output(input);
        assert!(
            result.contains("<think>code here</think>"),
            "think tag inside fenced code should be preserved, got: {}",
            result
        );
    }

    #[test]
    fn sanitize_preserves_think_in_inline_code() {
        let input = "Use `<think>` tags for reasoning. The answer is 42.";
        let result = sanitize_llm_output(input);
        assert!(
            result.contains("<think>"),
            "think tag inside inline code should be preserved, got: {}",
            result
        );
    }

    #[test]
    fn sanitize_strips_think_outside_but_preserves_inside_code() {
        let input =
            "<think>reasoning</think>\nHere is code:\n```\n<think>example</think>\n```\nDone.";
        let result = sanitize_llm_output(input);
        assert!(
            !result.starts_with("<think>"),
            "think outside code should be stripped"
        );
        assert!(
            result.contains("<think>example</think>"),
            "think inside code should be preserved, got: {}",
            result
        );
    }

    // ── Multi-format thinking tag tests ───────────────────────────────

    #[test]
    fn sanitize_strips_thinking_tag() {
        let input = "<thinking>\nAnalyzing...\n</thinking>\nResult here.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Result here.");
    }

    #[test]
    fn sanitize_strips_thought_tag() {
        let input = "<thought>internal</thought>Answer.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Answer.");
    }

    #[test]
    fn sanitize_strips_antthinking_tag() {
        let input = "<antthinking>\nplanning...\n</antthinking>\nHere you go.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Here you go.");
    }

    // ── Trailing incomplete directive tests ────────────────────────────

    #[test]
    fn sanitize_strips_trailing_brackets() {
        let input = "The answer is 42.[[";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "The answer is 42.");
    }

    #[test]
    fn sanitize_strips_trailing_incomplete_tag() {
        let input = "Done.\n<completion-check";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Done.");
    }

    #[test]
    fn sanitize_preserves_less_than_in_text() {
        // "< 10" should NOT be stripped — it's math, not a tag
        let input = "if x < 10 then stop";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "if x < 10 then stop");
    }

    // ── split_reasoning tests ─────────────────────────────────────────

    #[test]
    fn split_reasoning_no_tags() {
        let input = "Hello, world!";
        let (reasoning, answer) = split_reasoning(input);
        assert!(reasoning.is_none());
        assert_eq!(answer, "Hello, world!");
    }

    #[test]
    fn split_reasoning_extracts_think_block() {
        let input = "<think>Let me think...</think>\nAnswer is 42.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("Let me think..."));
        assert_eq!(answer, "\nAnswer is 42.");
    }

    #[test]
    fn split_reasoning_extracts_multiple_thinking_tags() {
        let input = "<think>first</think>\n<thinking>second</thinking>\nFinal.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("first\n\nsecond"));
        assert_eq!(answer, "\n\nFinal.");
    }

    #[test]
    fn split_reasoning_preserves_think_in_code() {
        let input = "<think>reasoning</think>\n```\n<think>code</think>\n```\nDone.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("reasoning"));
        assert!(answer.contains("<think>code</think>"));
    }

    #[test]
    fn split_reasoning_case_insensitive() {
        let input = "<THINK>caps</THINK>Answer.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("caps"));
        assert_eq!(answer, "Answer.");
    }
}
