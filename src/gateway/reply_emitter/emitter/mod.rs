use std::time::Duration;

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, AtomicU64};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::config::ReplyEmitterConfig;
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::inbound_context::ReplyRoute;
use crate::gateway::media::PendingMedia;
use crate::gateway::streaming::{StreamingConfig, StreamingController};

/// Streaming cursor appended to intermediate edits, removed on final.
const STREAMING_CURSOR: &str = "▍";

/// Routes Agent output back to the originating channel/conversation.
///
/// In streaming mode, tokens are pushed to a `StreamingController` as they
/// arrive. Once the character threshold is reached, an initial message is sent
/// and subsequent edits are debounced (300ms intervals) for real-time streaming.
///
/// In instant mode, the full response is sent as one message on completion.
pub struct ReplyEmitter {
    pub(crate) channel_registry: Arc<ChannelRegistry>,
    pub(crate) route: ReplyRoute,
    pub(crate) config: ReplyEmitterConfig,

    /// Accumulated response text (both modes)
    pub(crate) buffer: Mutex<String>,

    pub(crate) seq_counter: AtomicU64,
    pub(crate) has_sent: AtomicBool,
    /// Guard against duplicate `RunComplete` events (orchestrator drain + engine.rs both emit).
    pub(crate) run_complete_handled: AtomicBool,
    pub(crate) run_id: String,

    /// Cancellation token to stop the persistent typing indicator task
    pub(crate) typing_cancel: CancellationToken,

    /// Generation provider registry for TTS (voice mode)
    pub(crate) generation_registry: Option<Arc<crate::generation::GenerationProviderRegistry>>,
    /// Generation config for TTS (voice mode)
    pub(crate) generation_config:
        Option<Arc<tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>>>,
    /// Per-channel voice state
    pub(crate) voice_state: Mutex<crate::gateway::voice::VoiceState>,

    /// Resolved attachments from tool outputs, filled by the tool-dispatch
    /// chokepoint and drained by this emitter at run end.
    ///
    /// The emitter deliberately holds no `MediaCache`: it does not fetch. It
    /// used to, which meant every URL was downloaded twice and — because every
    /// drain site runs after the loop has ended — a failed fetch had no one
    /// left to report to.
    pub(crate) pending_media: PendingMedia,
    /// Real-time streaming state machine (replaces old typewriter mode)
    pub(crate) streaming: Mutex<StreamingController>,

    /// Native stream handler (if channel supports `StreamProtocol::Native`)
    pub(crate) native_handler: Option<Arc<dyn crate::gateway::channel::NativeStreamHandler>>,
    /// Active native stream state
    pub(crate) native_stream_state: Mutex<Option<NativeStreamState>>,
    /// Whether native streaming disabled due to error
    pub(crate) native_disabled: AtomicBool,

    /// Fallback model info — stored when `ModelResolved` fires with `is_fallback=true`.
    /// Appended as a notice line to non-Panel channel replies.
    pub(crate) fallback_info: Mutex<Option<crate::providers::health::ModelInfo>>,

    /// Model identifier observed on the last `ModelResolved` event for this run.
    /// Used to render the optional runtime-metadata footer; `None` if the run
    /// never emitted `ModelResolved` (older execution paths).
    pub(crate) model_label: Mutex<Option<String>>,

    /// Accumulated reasoning text from `StreamEvent::Reasoning` / `ReasoningBlock`.
    pub(crate) reasoning_buffer: Mutex<String>,
}

/// Tracks the state of an active native stream (e.g., Teams streaming info).
pub(crate) struct NativeStreamState {
    pub(crate) stream_id: String,
    pub(crate) sequence: u32,
    pub(crate) last_update: std::time::Instant,
}

impl ReplyEmitter {
    /// Create a new `ReplyEmitter` with default configuration (instant mode)
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
            streaming: Mutex::new(StreamingController::new(StreamingConfig {
                min_initial_chars: 30,
                debounce_interval: Duration::from_millis(300),
                enabled: false,
            })),
            native_handler: None,
            native_stream_state: Mutex::new(None),
            native_disabled: AtomicBool::new(false),
            fallback_info: Mutex::new(None),
            model_label: Mutex::new(None),
            reasoning_buffer: Mutex::new(String::new()),
        }
    }

    /// Create a new `ReplyEmitter` with custom configuration
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
            streaming: Mutex::new(StreamingController::new(StreamingConfig {
                min_initial_chars,
                debounce_interval: Duration::from_millis(debounce_ms),
                enabled: stream_enabled,
            })),
            native_handler: None,
            native_stream_state: Mutex::new(None),
            native_disabled: AtomicBool::new(false),
            fallback_info: Mutex::new(None),
            model_label: Mutex::new(None),
            reasoning_buffer: Mutex::new(String::new()),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub const fn route(&self) -> &ReplyRoute {
        &self.route
    }

    /// Overflow threshold in characters. Returns 0 when overflow detection
    /// is disabled (channel has no `max_message_length` or streaming is off).
    /// Subtracts a safety margin (~300) for HTML tag overhead.
    pub(crate) const fn overflow_threshold(&self) -> usize {
        let max = self.config.max_message_length;
        if max == 0 {
            return 0;
        }
        max.saturating_sub(300)
    }

    /// Attach voice mode dependencies, enabling TTS output.
    pub fn with_voice(
        mut self,
        voice_state: crate::gateway::voice::VoiceState,
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
}

impl Drop for ReplyEmitter {
    fn drop(&mut self) {
        self.typing_cancel.cancel();
    }
}

mod helpers;
mod streaming;
