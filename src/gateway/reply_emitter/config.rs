use crate::gateway::channel::{ChannelCapabilities, StreamProtocol};
use crate::gateway::i18n::Locale;
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

    /// Maximum message length the target channel will accept (0 = unknown /
    /// unlimited), copied verbatim from
    /// [`ChannelCapabilities::max_message_length`].
    ///
    /// **Two consumers, and it is not only a streaming concern**: the
    /// streaming overflow threshold
    /// ([`ReplyEmitter::overflow_threshold`](crate::gateway::reply_emitter::ReplyEmitter))
    /// *and* the non-streamed outbound chunker
    /// (`ReplyEmitter::outbound_chunk_len`). The chunker used to hold a second,
    /// wrong answer — a hardcoded 4000 — which every channel with a smaller cap
    /// (Discord's 2000, and Discord's `Channel::send` does not split) rejected
    /// outright, losing the whole answer.
    ///
    /// Declared in **characters** (that is the unit every adapter populates the
    /// capability with); the splitter counts **bytes**, which is the
    /// conservative direction — see `outbound_chunk_len`.
    pub max_message_length: usize,

    /// Optional runtime-metadata footer appended to the final reply.
    /// Defaults to a disabled config so existing callers stay no-op.
    pub footer: RuntimeFooterConfig,

    /// This run answers a `/btw` side question, so its final text is marked
    /// (`crate::gateway::btw::format_side_answer`) before it reaches the
    /// channel.
    ///
    /// **Resolved once, at emitter construction, from
    /// [`crate::gateway::btw::BtwTurn::resolve`]** — the one resolver every
    /// surface shares. It cannot come from the run's metadata, and the reason
    /// is ordering, not distance: `BTW_METADATA_KEY` is stamped by
    /// `stamp_btw`, which this same router calls ~300 lines further down
    /// (`inbound_router/executor.rs`, just before the busy lane) and which
    /// `execute()` then re-runs idempotently. The emitter is built before
    /// either — before the `RunRequest` it would read even exists. Asking the
    /// resolver here is therefore the same derivation `stamp_btw` makes, from
    /// the same bytes, not a second one — and it must stay that way:
    /// re-deriving from a `/btw` string prefix on this side would be exactly
    /// the duplicate predicate `btw/mod.rs` deleted.
    ///
    /// Carried in the *config* rather than on the emitter so the two custom
    /// emitters that clone `reply_config` (Feishu's streaming card, Telegram's
    /// orchestrated lanes) inherit it without a second construction argument to
    /// forget.
    pub side_answer: bool,

    /// The language this run's channel reply is written in.
    ///
    /// One message, two producers. When a run halts with no text of its own,
    /// `helpers::run_dispatch_and_drain_classified` renders the paragraph the
    /// user reads through `i18n::render_loop_halt`, in the language
    /// `[general] language` names; the emitter then appends a one-line halt tag
    /// beneath it. That tag was English for every deployment, so a `zh` install
    /// shipped a Chinese paragraph with an English label stuck to the bottom.
    ///
    /// Resolved at construction from the same `app_config` read that assembles
    /// the rest of this struct, which is the **same derivation** the
    /// `metadata["locale"]` stamp makes a few hundred lines later in
    /// `inbound_router::executor` (`Locale::from_config(cfg.general.language)`)
    /// and which `run_loop::inner` then re-reads for the paragraph — not a
    /// second answer. Carried in the config for the reason
    /// [`Self::side_answer`] gives: the Feishu and Telegram emitters clone this
    /// struct, and a second constructor argument is a thing to forget.
    pub locale: Locale,
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
            side_answer: false,
            // Not a literal `Zh`. This is the same call the stamping site
            // makes with an absent `[general] language`, so an emitter built
            // without config lands on whatever the halt paragraph beside it
            // will land on — which is the entire property this field buys.
            locale: Locale::from_config(None),
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

    /// Reconcile the operator's global preference with what this channel can
    /// physically do.
    ///
    /// Streaming is **two** questions and only one of them used to be asked
    /// here. `[behavior] output_mode` is the *preference*; the channel's
    /// capabilities are the *possibility*. Edit-based streaming sends one
    /// message and then rewrites it, so the possibility is exactly
    /// [`ChannelCapabilities::editing`] — on a channel that cannot edit,
    /// `Channel::edit`'s default body is an `Err` that the streaming emitter
    /// drops on the floor with no send fallback, so the reader receives the
    /// first flush and *nothing else*, silently.
    ///
    /// The `EditBased` arm may only widen and it runs first; the capability
    /// floor runs last and may only narrow. Note the two declarations are
    /// independent: `slack`/`mattermost` can edit while declaring
    /// `StreamProtocol::None` (they stream, and keep streaming), and
    /// `line`/`wechat` used to declare `EditBased` while their `edit` returns
    /// `UnsupportedFeature` (they were forced *into* the broken path by the
    /// widening arm — a floor that only ran in the `else` would not have
    /// reached them).
    /// `max_message_length` is copied **unconditionally**, deliberately: it is
    /// a fact about the transport, not about streaming. It used to live inside
    /// the `EditBased` arm, which is the "one-directional override" shape —
    /// the widening arm carried a fact that has nothing to do with widening.
    /// The cost was that every `StreamProtocol::None` channel (slack 3000,
    /// mattermost 16383, irc 400, …) left it at 0, disabling both the
    /// streaming overflow guard *and* — once the chunker started reading it —
    /// the outbound length cap, on exactly the channels whose cap is known.
    pub fn apply_channel_capabilities(&mut self, caps: &ChannelCapabilities) {
        self.max_message_length = caps.max_message_length;
        if caps.stream_protocol == StreamProtocol::EditBased {
            self.stream_enabled = true;
        }
        self.stream_enabled &= caps.editing;
    }
}
