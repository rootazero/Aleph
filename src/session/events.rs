//! Event types for the session log.

use serde::{Deserialize, Serialize};

pub type Timestamp = i64; // unix milliseconds
pub type EventSeq = u64;
pub type TurnId = uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnTrigger {
    UserMessage,
    SubagentRequest,
    Scheduled,
    Wake,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Who authorized a gated tool call.
///
/// `Autoconfirm` was removed: it had no constructor anywhere in the tree, so no
/// stored event can carry it and nothing could ever read it back. A variant
/// with no producer is a claim the enum cannot honour.
pub enum ApprovalSource {
    /// A human answered the prompt for this call.
    User,
    /// A grant taken earlier in the session satisfied the gate — nobody was
    /// asked this time. Produced by the session-approval-memory short circuit
    /// in `tools::scoped::dispatch::confirm_with_memory`.
    Trusted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// What kind of failure a [`SessionEvent::Error`] receipt records.
///
/// Deliberately NOT open vocabulary. `Llm`, `Tool`, `Sandbox`, `Harness`,
/// `Serialization` and `Other` were removed for the same reason
/// [`ApprovalSource::Autoconfirm`] was: nothing constructed them, so no stored
/// event could carry one and nothing could ever read one back. The next kind
/// arrives in the same commit as the producer that emits it — a variant is a
/// claim about what the log can contain, and an unproduced one is false.
pub enum ErrorKind {
    /// A guardrail refused the run's input. Produced by
    /// [`crate::orchestrator::harness_bridge`] when a run finishes having
    /// screened its input and said nothing.
    Guardrail,
    /// A pre-seed lifecycle hook refused or stopped the run before it started
    /// (§5.4) — the set of seams is derived by
    /// `run_loop::hook_stop_tests::every_pre_seed_hook_exit_journals_the_stop`,
    /// not listed here. Produced by
    /// `gateway::execution_engine::run_loop::hook_stop_receipt`, which closes
    /// the run in the same batch, so the log says "a run happened and the hook
    /// stopped it" — never `Unanswered`.
    HookStop,
}

/// Why a dispatched call stopped at a gate instead of running (§6.1).
///
/// Serde name = the wire word; `Display` = the clause the boundary repair
/// reads to the model ("… was still waiting for {reason}"). Two spellings of
/// one fact, pinned to each other by
/// `tests::park_reason_wire_word_is_the_serde_name_and_display_is_the_clause`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    /// A confirmation / operator card
    /// (`tools::scoped::dispatch::confirm_with_memory`), or the sandbox's
    /// capability-elevation card raised inside a shell tool's own `execute`
    /// (`sandbox::workspace`) — the set of stamped park sites is derived by
    /// `tools::scoped::tests::every_production_approval_park_is_stamped_or_named_exempt`,
    /// not listed here.
    Approval,
    /// A question delivered to the person and waiting for the answer
    /// (`clarification::ask` — reached by `ask_user` and by the scratchpad
    /// plan gate). Written only once delivery is proven, so the question WAS
    /// shown: the call did not "never run", the answer is what is missing.
    Clarification,
    /// A card a `BeforeToolCall` hook's `Ask` raised. NOT "the hook script
    /// was running": a crash inside a hook script stays OUTCOME UNKNOWN,
    /// because no release fact exists for it.
    PreHook,
}

impl ParkReason {
    /// Every variant, for the tests that walk them and for a wire face
    /// that wants to enumerate. Pinned complete by
    /// `tests::park_reason_all_lists_every_variant_once` (an exhaustive
    /// match, so a new variant does not compile until it is added here).
    pub const ALL: [ParkReason; 3] = [Self::Approval, Self::Clarification, Self::PreHook];

    /// The serde word, for a caller that wants the wire spelling without a
    /// serializer round-trip.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Clarification => "clarification",
            Self::PreHook => "pre_hook",
        }
    }
}

impl std::fmt::Display for ParkReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Approval => "operator approval",
            Self::Clarification => "the answer to your question",
            Self::PreHook => "a pre-tool hook",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageContent {
    /// Free-form text body (UI-displayable).
    pub text: String,
    /// Optional rich blocks (images, `tool_use`). Uses JSON to avoid pulling in
    /// provider-specific types at this layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<serde_json::Value>,
    /// Thinking/reasoning trace from extended-thinking models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Opaque signature accompanying the thinking content. Anthropic requires
    /// a signed thinking block to be replayed verbatim on subsequent turns
    /// whenever the same assistant message also contains `tool_use` blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
}

impl SessionEvent {
    /// A plain-text `UserMessage` the harness wrote itself — a grace nudge, a
    /// stop-hook halt, a verifier veto.
    ///
    /// Three call sites in `src/harness/` had built this literal by hand, which
    /// is one past the point where the duplication should collapse (P6). The
    /// reason to collapse it *here* rather than leave three copies is the last
    /// field: a harness-authored message has no human author by construction,
    /// and this is what makes that unforgettable rather than merely true today.
    /// Adding a fourth synthetic message in the loop must not require
    /// remembering spec §6.2 — and it must not spend R10 budget on remembering.
    #[must_use]
    pub fn synthetic_user(turn_id: TurnId, text: String) -> Self {
        SessionEvent::UserMessage {
            turn_id,
            content: MessageContent {
                text,
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: true,
            author_user_id: None,
        }
    }

    /// The 1+1 seed pair a single-text user turn opens with: `TurnStarted`
    /// then the `UserMessage`, sharing one `turn_id` and one `at`. Two
    /// writers (the bridge's `seed_history` and the L0 fast path) used to
    /// spell it by hand. `at` is the caller's so a batch that carries the
    /// pair alongside other rows stamps one instant on all of them.
    #[must_use]
    pub fn user_turn(
        turn_id: TurnId,
        content: MessageContent,
        author_user_id: Option<String>,
        at: Timestamp,
    ) -> [SessionEvent; 2] {
        [
            SessionEvent::TurnStarted {
                turn_id,
                trigger: TurnTrigger::UserMessage,
                at,
            },
            SessionEvent::UserMessage {
                turn_id,
                content,
                at,
                synthetic: false,
                author_user_id,
            },
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutput {
    pub value: serde_json::Value,
    #[serde(default)]
    pub metadata: ToolOutputMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutputMetadata {
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub cost_cents: Option<u64>,
    /// Out-of-band image payloads carried alongside the (text) `value`.
    ///
    /// Some tools — desktop screenshots above all — produce an image the
    /// vision-capable model must actually *see*. The text result budget
    /// (`apply_layer_two`) would otherwise flatten and truncate the base64
    /// into oblivion, so the image is hoisted here BEFORE truncation and
    /// re-emitted as a `ContentBlock::Image` when the tool result is rendered
    /// into the prompt. Empty for the overwhelming majority of tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolImage>,
    /// Structured UI presentation (file diffs) hoisted out of the tool's
    /// JSON by `apply_layer_two` BEFORE the value is flattened to model text.
    /// Rides `session_events` for replay and the callback for the live frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<aleph_protocol::Presentation>,
}

/// A single out-of-band image attached to a tool result (base64 + MIME).
///
/// Mirrors UI-TARS-desktop's "screenshot re-injection as a post-tool side
/// effect": the screen the model acted on is fed back as a viewable image on
/// the next turn, closing the perceive→act loop.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolImage {
    /// Base64-encoded image bytes (no `data:` URL prefix).
    pub data: String,
    /// MIME type, e.g. `image/png` or `image/jpeg`.
    pub mime_type: String,
}

/// Terminal disposition of a harness run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Run reached its natural end (model stop / final reply).
    Completed,
    /// Run was deliberately cancelled (user `/stop`). NOT resumed.
    Cancelled,
    /// Run ended with an error. NOT resumed (the error is in the log;
    /// re-running would likely hit the same error).
    Errored,
    /// Resume gave up on this run — cap reached or too old. Terminal.
    Abandoned,
}

/// The session-knob envelope a run started under, frozen onto its
/// `RunStarted` marker so a resume replays the crashed run's configuration
/// instead of re-deriving it from whatever the knobs say now.
///
/// Every knob field is a **String**, spelled with the same literal word the
/// `identity_meta.custom` bag uses, so the snapshot, the session row and the
/// client-facing `SessionSnapshot` share one vocabulary rather than three
/// enums that have to be kept convertible. The key set is pinned by a census
/// test against [`crate::gateway::session_snapshot::RUN_ENVELOPE_KNOB_KEYS`]
/// ∪ [`crate::gateway::resume_coordinator::RUN_ENVELOPE_FACT_KEYS`]: a new
/// field has to be filed as a knob or as a fact, or that test fails.
///
/// `model` / `model_provider` are the pair the run was **actually bound to**
/// after provider validation — not the pin that was asked for. A resume that
/// replayed the unvalidated hint would re-derive a route the crashed run never
/// took.
///
/// The last two fields are per-run **facts**, not knobs: they have no
/// `custom` twin and no session / global rung, so a resume replays them from
/// this snapshot or not at all, and their absence is the normal case rather
/// than a loss.
///
/// Absent from the wire when `None` — a legacy log deserialises to `None`,
/// which `ResumeReport::unsnapshotted` counts rather than papers over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunEnvelopeSnapshot {
    /// `ExecTier::id()` — the tier the run was executing under. On resume this
    /// is a **ceiling**, never a request: recovery may only tighten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,
    /// `SessionMode::id()` — chat / work / code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_mode: Option<String>,
    /// `ThinkLevel::id()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_level: Option<String>,
    /// `MemoryMode::id()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mode: Option<String>,
    /// Model id the run was **served by** — the directive's model when it
    /// carried one, else what the provider chain said it was about to serve.
    ///
    /// `None` means the writer could not name a model, not "the run carried
    /// no pin" — either because the chain could not say what served (a
    /// dynamic route with no `serving_model_hint`) or because nothing served
    /// at all (the slash-command fast path makes no LLM call). The narrower
    /// reading is load-bearing: a resume that finds `None` here re-derives
    /// the model from today's session, which is a different model than the
    /// crashed run used whenever the session was re-pinned in between — so
    /// `plan_resume` treats `None` as a degrade and says so, rather than
    /// answering on a substitute in silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider the model above was pinned to, or `None` for an unqualified
    /// pin (the resolver picks the provider by model-name heuristic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// `/<skill>` `allowed-tools` this run executed under. `Some(vec![])` is
    /// deny-all, `None` "declared nothing" (the `slash_skill_scope` tri-state).
    /// A per-run FACT, not a knob: on resume it has one rung — this snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// The `/btw` stamp exactly as `btw::BTW_METADATA_KEY` carried it, so a
    /// resumed side question keeps its read-only ceiling
    /// (`turn_permissions`'s one `contains_key` read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btw: Option<String>,
}

impl RunEnvelopeSnapshot {
    /// The knob half of a snapshot, spelled once.
    ///
    /// Both writers — the harness bridge (`runner_impl::run_envelope_snapshot`)
    /// and the slash-command fast path (`slash_command::slash_gate_reason`) —
    /// go through this, so the `id()` vocabulary the resume parses back has a
    /// single derivation. The model pair and the two per-run facts are left
    /// `None` for the caller: which of them a writer can name differs per
    /// path, and that difference is the caller's to state.
    #[must_use]
    pub fn from_knobs(
        exec_tier: Option<crate::config::types::policies::ExecTier>,
        session_mode: Option<crate::config::types::policies::SessionMode>,
        think_level: Option<crate::agents::thinking::ThinkLevel>,
        memory_mode: Option<crate::memory::session_memory_mode::MemoryMode>,
    ) -> Self {
        Self {
            exec_tier: exec_tier.map(|t| t.id().to_string()),
            session_mode: session_mode.map(|m| m.id().to_string()),
            think_level: think_level.map(|l| l.id().to_string()),
            memory_mode: memory_mode.map(|m| m.id().to_string()),
            ..Self::default()
        }
    }

    /// True when the writer resolved nothing at all.
    ///
    /// Distinct from a `None` envelope: `None` means *no writer captured one*
    /// (a legacy marker, or a producer that has no envelope to capture — a
    /// hook-stop receipt closes a run that never ran a turn), while an empty
    /// one means the capture happened and the gateway had resolved nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exec_tier.is_none()
            && self.session_mode.is_none()
            && self.think_level.is_none()
            && self.memory_mode.is_none()
            && self.model.is_none()
            && self.model_provider.is_none()
            && self.allowed_tools.is_none()
            && self.btw.is_none()
    }
}

// NOTE: `PartialEq` is intentionally omitted from `SessionEvent` because
// some variants carry types that do not implement it.
// Tests that need comparison should compare on the serialized JSON form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
// rust-doctor-disable-next-line large-enum-variant
pub enum SessionEvent {
    SessionWoken {
        at: Timestamp,
        prior_head: EventSeq,
    },

    /// A harness run began on this session.
    RunStarted {
        run_id: String,
        at: Timestamp,
        /// Project workspace this run was scoped to, when project-mode is
        /// active. Persisted so [`crate::gateway::resume_coordinator`] can
        /// re-trigger an interrupted run in the same project folder
        /// instead of falling back to `~/.aleph/workspaces/{agent_id}/`.
        /// Stored as a string (rather than `PathBuf`) so the JSON form
        /// stays platform-portable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_root: Option<String>,
        /// The knob envelope this run started under (see
        /// [`RunEnvelopeSnapshot`]). `None` on every marker written before
        /// the snapshot existed and on markers whose writer did not capture
        /// one; omitted from the wire when `None` so the legacy forms stay
        /// byte-identical.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        envelope: Option<RunEnvelopeSnapshot>,
    },
    /// A harness run reached a terminal state on this session.
    RunFinished {
        run_id: String,
        outcome: RunOutcome,
        at: Timestamp,
    },
    /// Intent stamp written by `ResumeCoordinator` BEFORE it re-triggers a run
    /// (§5.1). `target` = seq of the `RunStarted` being resumed, or of the
    /// unanswered `UserMessage`. Marker-class: no turn, not prompt-bearing.
    ResumeAttempted {
        target: EventSeq,
        attempt: u32,
    },

    /// A turn opened. There is deliberately no closing `TurnEnded` marker: a
    /// turn ends when the next one opens or when the run does, and the crash
    /// boundary is read off the `RunStarted`/`RunFinished` pair instead
    /// ([`crate::session::reduction::reduce_disposition`]). A
    /// `TurnEnded` variant existed here for a long time with no producer, so
    /// every turn matched "crashed mid-turn" and nothing could use it.
    TurnStarted {
        turn_id: TurnId,
        trigger: TurnTrigger,
        at: Timestamp,
    },
    UserMessage {
        turn_id: TurnId,
        content: MessageContent,
        at: Timestamp,
        /// `true` when this entry was injected by the harness itself rather
        /// than coming from the real end-user (e.g. verifier-veto nudge,
        /// grace-turn `MAX_STEPS` hint). Defaults to `false` for backward
        /// compatibility with on-disk session logs that pre-date this field.
        ///
        /// The prompt builder (G2) wraps every *real* mid-loop user message
        /// in `<system-reminder>` so the model treats it as an interjection;
        /// synthetic messages are passed through unchanged.
        #[serde(default)]
        synthetic: bool,
        /// Who typed this, in a multi-human project room (spec §6.2). `None`
        /// for every single-author session, for every harness-authored
        /// message, and for every event written before P2 — absent means "the
        /// session's own user", the same adoption-by-absence rule the rest of
        /// the multi-user arc uses.
        ///
        /// Stamped from [`crate::scope::room_author`], which is the single
        /// source for *when* a message needs an author at all. Only the id is
        /// stored: a display name is presentation, resolved fresh at render
        /// time through `scope::directory`, so a rename shows up in history
        /// instead of being frozen into it.
        ///
        /// Mirrors the [`SessionEvent::RunStarted::project_root`] precedent —
        /// an optional payload field, not a side channel — and like it,
        /// `skip_serializing_if` keeps it off the wire and out of the prompt's
        /// cached prefix for the single-author sessions that are still the
        /// overwhelming majority.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author_user_id: Option<String>,
    },
    AssistantMessage {
        turn_id: TurnId,
        content: MessageContent,
        /// What the provider billed for the ONE LLM call that produced this
        /// message. The harness emits an `AssistantMessage` per Think step, so
        /// calls and assistant rows are 1:1 and the attribution is exact — there
        /// is no "which of the run's N calls does this row own" to guess at.
        ///
        /// This is what `messages.input_tokens` / `output_tokens` are projected
        /// from. They had a column, a `MessageRecord` field, and were handed to
        /// the model (the `sessions` tool) and the Panel — as zeros, forever,
        /// because their only feeder was a `SessionEvent::LlmCallEnded` that no
        /// production code has ever emitted. A fabricated 0 reads as a
        /// measurement; this is the measurement.
        ///
        /// `None` on replayed pre-existing logs (hence `serde(default)`) and on
        /// a provider that reported no usage — absent, not zero.
        #[serde(default)]
        usage: Option<crate::orchestrator::dispatch::TokenBreakdown>,
        at: Timestamp,
    },
    /// Stamped after every completed run. Carries what the usage fold
    /// (`session::usage_fold`) cannot derive: the run_id join, context-window
    /// occupancy, the priced cost and the serving model. Token counters were
    /// removed 2026-09-12 — they are folded from `AssistantMessage.usage`
    /// (`session::usage_fold::run_usage_totals`) when the projector lands this
    /// stamp; rows written before then still carry `input_tokens` /
    /// `output_tokens`, which serde ignores on the way in.
    ///
    /// The three gauge fields are `None` when the run resolved no occupancy —
    /// a provider that reported no usage, a hook that stopped the run before
    /// its first Think. The meta is still emitted so the run_id join lands on
    /// the row the run did produce; the projector stamps the gauge only when
    /// all three are `Some`, and a missing key reads as absent, never as 0.
    /// Rows written before 2026-09-13 always carried the three as numbers and
    /// decode as `Some`.
    AssistantRunMeta {
        turn_id: TurnId,
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_tokens: Option<u64>,
        /// This run's cost in USD, or `None` when it could not be priced.
        /// `None` ≠ 0.0 — an unpriced run must not silently understate the
        /// session total.
        #[serde(default)]
        cost_usd: Option<f64>,
        /// Model that served this run, and its provider — recorded onto
        /// `sessions.model` / `sessions.model_provider`.
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        model_provider: Option<String>,
        at: Timestamp,
    },
    SystemMessage {
        turn_id: TurnId,
        content: String,
        at: Timestamp,
    },

    ToolCallRequested {
        turn_id: TurnId,
        call_id: String,
        name: String,
        input: serde_json::Value,
        at: Timestamp,
    },
    ToolCallApproved {
        turn_id: TurnId,
        call_id: String,
        by: ApprovalSource,
        at: Timestamp,
    },
    ToolCallDenied {
        turn_id: TurnId,
        call_id: String,
        reason: String,
        at: Timestamp,
    },
    /// Written by the gate BEFORE it parks (§6.1) — an intent stamp, so a
    /// crash while parked reads "never ran" instead of "outcome unknown".
    /// Normal durability (U3): a lost stamp reads as "outcome unknown", the
    /// safe direction. No `tool_name`: the dispatch this pairs with owns it,
    /// and `clarification::ask` cannot know its caller's name. Written
    /// through ONE writer, `session::call_log::emit_for_ambient_call`.
    ToolCallParked {
        turn_id: TurnId,
        call_id: String,
        reason: ParkReason,
    },
    ToolResult {
        turn_id: TurnId,
        call_id: String,
        output: ToolOutput,
        at: Timestamp,
    },
    ToolError {
        turn_id: TurnId,
        call_id: String,
        error: String,
        at: Timestamp,
    },

    SubagentSpawned {
        turn_id: TurnId,
        child_id: crate::routing::session_key::SessionKey,
        flow: String,
        at: Timestamp,
    },
    SubagentReturned {
        turn_id: TurnId,
        child_id: crate::routing::session_key::SessionKey,
        summary: String,
        at: Timestamp,
    },

    CompactionPerformed {
        from_seq: EventSeq,
        to_seq: EventSeq,
        /// Turn id of the summary `SystemMessage` written in the same batch
        /// (before 2026-09-12: its seq). No production reader resolves it; a
        /// future one must accept both.
        summary_ref: String,
        at: Timestamp,
    },

    /// Recorded as the first event of a child session created by
    /// compaction-driven session-split. `parent_session_id` is the parent
    /// session key string (`SessionKey::to_key_string()`).
    SessionForked {
        parent_session_id: String,
        at: Timestamp,
    },

    /// A durable receipt for a failure that produced no other trace.
    ///
    /// Its one producer is the input-guardrail block receipt in
    /// [`crate::orchestrator::harness_bridge`]: a screened-out input ends the
    /// run `Ok`, so without this the log reads as a clean empty run and every
    /// re-attaching client (reload, second tab, room peer) sees an unanswered
    /// user message. Projected to a `system` row by
    /// [`crate::session::projection::project_row`] so `chat.history` serves it;
    /// NOT prompt-bearing — the model must not be told its own refusal was
    /// something it said.
    Error {
        turn_id: Option<TurnId>,
        kind: ErrorKind,
        message: String,
        recoverable: bool,
        at: Timestamp,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEventRecord {
    pub seq: EventSeq,
    pub event: SessionEvent,
    pub created_at_ms: Timestamp,
}

/// What a batch retires in the same transaction as its inserts.
///
/// Both bounds are **inclusive**. Retiring is a soft delete (`retired_at` is
/// stamped, the row stays, seq allocation is unaffected) and idempotent: an
/// already-retired row keeps its original stamp and is not counted again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retire {
    /// Retire every live event with `seq <= through` — the head side, what
    /// `/compact` uses. The BM25 mirror is **kept**: compacted turns leave
    /// the prompt but must stay recallable.
    ///
    /// Conditional on the head being the one the caller read: the
    /// transaction rolls back with [`SessionError::RetireSpanChanged`] unless
    /// it retires exactly `live` rows. A head-side retire is always written
    /// on behalf of something computed from those rows (a summary of them),
    /// and `chat.clear` / `chat.rewind` retire from the tail without going
    /// through the session actor. A clear landing between the caller's read
    /// and this commit would otherwise put a summary of erased turns at the
    /// head of every future prompt. The count is exact because seqs only
    /// grow: no row with `seq <= through` can appear after the read, so the
    /// only way the count moves is a row the caller summarized having been
    /// retired by someone else.
    ///
    /// [`SessionError::RetireSpanChanged`]: crate::session::service::SessionError::RetireSpanChanged
    Through { through: EventSeq, live: usize },
    /// Retire every live event with `seq >= n` — the tail side, what
    /// `chat.clear` / `chat.rewind` / `/undo` use. The BM25 mirror rows for
    /// the same range are **deleted** in the same transaction, so erased
    /// turns cannot be searched back into the prompt.
    From(EventSeq),
}

/// Commit durability. `Barrier` fsyncs the WAL at commit (`PRAGMA synchronous=FULL`
/// for that one transaction); `Normal` is the store's resting level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Durability {
    Normal,
    Barrier,
}

/// THE durability policy table (U3). One function; no wildcard arm on purpose —
/// a new variant must state its column here or the crate does not compile.
pub const fn durability_of(event: &SessionEvent) -> Durability {
    match event {
        SessionEvent::ToolCallRequested { .. }
        | SessionEvent::RunStarted { .. }
        | SessionEvent::ResumeAttempted { .. }
        | SessionEvent::UserMessage { .. } => Durability::Barrier,
        SessionEvent::SessionWoken { .. }
        | SessionEvent::RunFinished { .. }
        | SessionEvent::TurnStarted { .. }
        | SessionEvent::AssistantMessage { .. }
        | SessionEvent::AssistantRunMeta { .. }
        | SessionEvent::SystemMessage { .. }
        | SessionEvent::ToolCallApproved { .. }
        | SessionEvent::ToolCallDenied { .. }
        // U3: an intent stamp whose LOSS reads "outcome unknown" — the safe
        // direction — so it does not buy an fsync.
        | SessionEvent::ToolCallParked { .. }
        | SessionEvent::ToolResult { .. }
        | SessionEvent::ToolError { .. }
        | SessionEvent::SubagentSpawned { .. }
        | SessionEvent::SubagentReturned { .. }
        | SessionEvent::CompactionPerformed { .. }
        | SessionEvent::SessionForked { .. }
        | SessionEvent::Error { .. } => Durability::Normal,
    }
}

/// A batch is as durable as its most durable member.
pub fn batch_durability<'a>(events: impl Iterator<Item = &'a SessionEvent>) -> Durability {
    events
        .map(durability_of)
        .max()
        .unwrap_or(Durability::Normal)
}

/// Second column of the same table (§7.3): may an older binary skip this row
/// unread? Nothing declares it this round. Its one consumer is
/// [`crate::session::store::encode_row`], which writes `"ignorable": true`
/// on the row so a build that does not know the variant can skip it
/// (`decode_row` → `DecodedRow::Skipped`) instead of refusing the session.
pub const fn ignorable(event: &SessionEvent) -> bool {
    let _ = event;
    false
}

/// Current wall-clock in unix ms.
#[must_use]
pub fn now_ms() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |e| {
            tracing::warn!(error = %e, "System clock went backwards — returning 0");
            0
        },
        |d| d.as_millis() as i64,
    )
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// One constructed event per variant — moved here from `fork/tests.rs` so
    /// the fork guard and the durability census share ONE list. Completeness is
    /// pinned against the enum source in
    /// `tests::durability_barrier_set_is_exactly_the_four_ruled_events`.
    ///
    /// Kept as `(name, event)` pairs so the guards compare names, not shapes.
    pub(crate) fn sample_of_every_kind() -> Vec<(&'static str, SessionEvent)> {
        let t = uuid::Uuid::new_v4();
        let content = |text: &str| MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        };
        vec![
            (
                "SessionWoken",
                SessionEvent::SessionWoken {
                    at: 0,
                    prior_head: 0,
                },
            ),
            (
                "RunStarted",
                SessionEvent::RunStarted {
                    run_id: "r".into(),
                    at: 0,
                    project_root: None,
                    envelope: None,
                },
            ),
            (
                "RunFinished",
                SessionEvent::RunFinished {
                    run_id: "r".into(),
                    outcome: RunOutcome::Cancelled,
                    at: 0,
                },
            ),
            (
                "ResumeAttempted",
                SessionEvent::ResumeAttempted {
                    target: 1,
                    attempt: 1,
                },
            ),
            (
                "TurnStarted",
                SessionEvent::TurnStarted {
                    turn_id: t,
                    trigger: TurnTrigger::SubagentRequest,
                    at: 0,
                },
            ),
            (
                "UserMessage",
                SessionEvent::UserMessage {
                    turn_id: t,
                    content: content("u"),
                    at: 0,
                    synthetic: false,
                    author_user_id: None,
                },
            ),
            (
                "AssistantMessage",
                SessionEvent::AssistantMessage {
                    turn_id: t,
                    content: content("a"),
                    usage: None,
                    at: 0,
                },
            ),
            (
                "SystemMessage",
                SessionEvent::SystemMessage {
                    turn_id: t,
                    content: "s".into(),
                    at: 0,
                },
            ),
            (
                "ToolCallRequested",
                SessionEvent::ToolCallRequested {
                    turn_id: t,
                    call_id: "c".into(),
                    name: "bash".into(),
                    input: serde_json::json!({}),
                    at: 0,
                },
            ),
            (
                "ToolCallApproved",
                SessionEvent::ToolCallApproved {
                    turn_id: t,
                    call_id: "c".into(),
                    by: ApprovalSource::Trusted,
                    at: 0,
                },
            ),
            (
                "ToolCallDenied",
                SessionEvent::ToolCallDenied {
                    turn_id: t,
                    call_id: "c".into(),
                    reason: "no".into(),
                    at: 0,
                },
            ),
            (
                "ToolCallParked",
                SessionEvent::ToolCallParked {
                    turn_id: t,
                    call_id: "c".into(),
                    reason: ParkReason::Approval,
                },
            ),
            (
                "ToolResult",
                SessionEvent::ToolResult {
                    turn_id: t,
                    call_id: "c".into(),
                    output: ToolOutput {
                        value: serde_json::Value::String("o".into()),
                        metadata: ToolOutputMetadata::default(),
                    },
                    at: 0,
                },
            ),
            (
                "ToolError",
                SessionEvent::ToolError {
                    turn_id: t,
                    call_id: "c".into(),
                    error: "e".into(),
                    at: 0,
                },
            ),
            (
                "AssistantRunMeta",
                SessionEvent::AssistantRunMeta {
                    turn_id: t,
                    run_id: "r".into(),
                    context_tokens: Some(0),
                    context_window: Some(0),
                    total_tokens: Some(0),
                    cost_usd: None,
                    model: None,
                    model_provider: None,
                    at: 0,
                },
            ),
            (
                "SubagentSpawned",
                SessionEvent::SubagentSpawned {
                    turn_id: t,
                    child_id: crate::routing::session_key::SessionKey::parse("agent:main:peer:x")
                        .expect("fixture key parses"),
                    flow: "f".into(),
                    at: 0,
                },
            ),
            (
                "SubagentReturned",
                SessionEvent::SubagentReturned {
                    turn_id: t,
                    child_id: crate::routing::session_key::SessionKey::parse("agent:main:peer:x")
                        .expect("fixture key parses"),
                    summary: "s".into(),
                    at: 0,
                },
            ),
            (
                "CompactionPerformed",
                SessionEvent::CompactionPerformed {
                    from_seq: 0,
                    to_seq: 1,
                    summary_ref: "s".into(),
                    at: 0,
                },
            ),
            (
                "SessionForked",
                SessionEvent::SessionForked {
                    parent_session_id: "p".into(),
                    at: 0,
                },
            ),
            (
                "Error",
                SessionEvent::Error {
                    turn_id: None,
                    kind: ErrorKind::Guardrail,
                    message: "m".into(),
                    recoverable: false,
                    at: 0,
                },
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_forked_event_round_trips_through_json() {
        let event = SessionEvent::SessionForked {
            parent_session_id: "agent:a/main:k:s2".to_string(),
            at: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            SessionEvent::SessionForked {
                parent_session_id, ..
            } => {
                assert_eq!(parent_session_id, "agent:a/main:k:s2");
            }
            other => panic!("expected SessionForked, got {other:?}"),
        }
    }

    /// The intent stamp's two numbers survive the wire byte-for-byte: `target`
    /// is what the reducer matches against a `RunStarted` seq, `attempt` is
    /// the ordinal the operator reads in a boot log.
    #[test]
    fn resume_attempted_event_round_trips_through_json() {
        let event = SessionEvent::ResumeAttempted {
            target: 41,
            attempt: 3,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            SessionEvent::ResumeAttempted { target, attempt } => {
                assert_eq!((target, attempt), (41, 3));
            }
            other => panic!("expected ResumeAttempted, got {other:?}"),
        }
        assert_eq!(
            serde_json::to_string(&serde_json::from_str::<SessionEvent>(&json).unwrap()).unwrap(),
            json
        );
    }

    #[test]
    fn run_started_serde_round_trips() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-abc".into(),
            at: 1_700_000_000_000,
            project_root: None,
            envelope: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        assert!(json.contains("\"type\":\"run_started\""));
        // Optional fields are omitted on the wire when None so the legacy
        // 2-field form stays byte-identical for old event-log readers.
        assert!(!json.contains("project_root"));
        assert!(!json.contains("envelope"));
    }

    /// New optional `project_root` field round-trips and survives the
    /// `#[serde(default)]` re-read path used by old logs (where the field
    /// simply doesn't exist).
    #[test]
    fn run_started_with_project_root_round_trips() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-pr".into(),
            at: 1_700_000_000_000,
            project_root: Some("/Users/alice/proj".into()),
            envelope: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"project_root\":\"/Users/alice/proj\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        match back {
            SessionEvent::RunStarted { project_root, .. } => {
                assert_eq!(project_root.as_deref(), Some("/Users/alice/proj"));
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    /// Backward compatibility: deserialising a legacy 2-field RunStarted
    /// (no `project_root` key, no `envelope` key) and the 3-field form that
    /// predates `envelope` both yield `None` for every absent optional field
    /// thanks to `#[serde(default)]`.
    #[test]
    fn run_started_legacy_log_deserialises_with_none() {
        let two_field = r#"{"type":"run_started","run_id":"old","at":1700000000000}"#;
        let three_field =
            r#"{"type":"run_started","run_id":"old","at":1700000000000,"project_root":"/p"}"#;
        for (legacy, expected_root) in [(two_field, None), (three_field, Some("/p"))] {
            let back: SessionEvent = serde_json::from_str(legacy).unwrap();
            match back {
                SessionEvent::RunStarted {
                    project_root,
                    envelope,
                    ..
                } => {
                    assert_eq!(project_root.as_deref(), expected_root);
                    assert!(
                        envelope.is_none(),
                        "legacy log {legacy} must carry no envelope"
                    );
                }
                other => panic!("expected RunStarted, got {other:?}"),
            }
        }
    }

    /// Third generation: a marker written by a build that captures the ④
    /// envelope. Round-trips, and every field survives.
    #[test]
    fn run_started_with_an_envelope_round_trips() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-env".into(),
            at: 1_700_000_000_000,
            project_root: Some("/p".into()),
            envelope: Some(RunEnvelopeSnapshot {
                exec_tier: Some("full".into()),
                session_mode: Some("code".into()),
                think_level: Some("high".into()),
                memory_mode: Some("off".into()),
                model: Some("m-old".into()),
                model_provider: Some("p-old".into()),
                allowed_tools: Some(vec!["grep".into()]),
                btw: Some("is it green?".into()),
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        match back {
            SessionEvent::RunStarted { envelope, .. } => {
                let env = envelope.expect("envelope survives the round trip");
                assert_eq!(env.exec_tier.as_deref(), Some("full"));
                assert_eq!(env.session_mode.as_deref(), Some("code"));
                assert_eq!(env.think_level.as_deref(), Some("high"));
                assert_eq!(env.memory_mode.as_deref(), Some("off"));
                assert_eq!(env.model.as_deref(), Some("m-old"));
                assert_eq!(env.model_provider.as_deref(), Some("p-old"));
                assert_eq!(env.allowed_tools.as_deref(), Some(&["grep".to_string()][..]));
                assert_eq!(env.btw.as_deref(), Some("is it green?"));
                assert!(!env.is_empty());
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    /// The two per-run FACTS (skill scope, `/btw` stamp) are additive on the
    /// wire: a marker written before they existed decodes to `None` for both,
    /// `None` is never serialised, and an EMPTY scope is still a declaration
    /// (`[]` on the wire, `is_empty() == false`) — the `slash_skill_scope`
    /// tri-state survives the marker.
    #[test]
    fn an_old_run_started_without_the_fact_keys_decodes_to_none_and_none_stays_off_the_wire() {
        let old = r#"{"type":"run_started","run_id":"r","at":1,"envelope":{"exec_tier":"ask"}}"#;
        let SessionEvent::RunStarted {
            envelope: Some(env),
            ..
        } = serde_json::from_str(old).unwrap()
        else {
            panic!("a run_started with an envelope object decodes to Some");
        };
        assert_eq!((env.allowed_tools.as_ref(), env.btw.as_ref()), (None, None));
        let json = serde_json::to_string(&env).unwrap();
        assert!(
            !json.contains("allowed_tools") && !json.contains("btw"),
            "{json}"
        );
        let scoped = RunEnvelopeSnapshot {
            allowed_tools: Some(vec![]),
            btw: Some("q?".into()),
            ..Default::default()
        };
        assert!(
            serde_json::to_string(&scoped)
                .unwrap()
                .contains("\"allowed_tools\":[]"),
            "an empty list is a declaration"
        );
        assert!(!scoped.is_empty());
    }

    /// A captured-but-empty envelope is NOT the same answer as an absent one:
    /// it serialises as `{}` and deserialises back to `Some`, which is what
    /// lets `ResumeReport::unsnapshotted` mean "no writer captured one"
    /// instead of "the gateway had resolved nothing".
    #[test]
    fn an_empty_envelope_is_still_some() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-empty".into(),
            at: 1,
            project_root: None,
            envelope: Some(RunEnvelopeSnapshot::default()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"envelope\":{}"), "{json}");
        match serde_json::from_str::<SessionEvent>(&json).unwrap() {
            SessionEvent::RunStarted { envelope, .. } => {
                let env = envelope.expect("an empty object is Some, not None");
                assert!(env.is_empty());
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    /// Census: the envelope's key set IS the knob vocabulary
    /// `session_snapshot` publishes PLUS the per-run facts
    /// `resume_coordinator::RUN_ENVELOPE_FACT_KEYS` names. A field added to
    /// the struct and filed with neither list fails here — which is the whole
    /// reason both arrays exist: the author has to say which kind the new
    /// field is.
    #[test]
    fn the_envelope_carries_exactly_the_published_knob_keys() {
        use crate::gateway::resume_coordinator::RUN_ENVELOPE_FACT_KEYS;
        let all = RunEnvelopeSnapshot {
            exec_tier: Some("a".into()),
            session_mode: Some("b".into()),
            think_level: Some("c".into()),
            memory_mode: Some("d".into()),
            model: Some("e".into()),
            model_provider: Some("f".into()),
            allowed_tools: Some(vec!["g".into()]),
            btw: Some("h".into()),
        };
        let value = serde_json::to_value(&all).unwrap();
        let mut got: Vec<String> = value
            .as_object()
            .expect("the envelope serialises as an object")
            .keys()
            .cloned()
            .collect();
        got.sort();
        let mut want: Vec<String> = crate::gateway::session_snapshot::RUN_ENVELOPE_KNOB_KEYS
            .iter()
            .chain(RUN_ENVELOPE_FACT_KEYS.iter())
            .map(|k| (*k).to_string())
            .collect();
        want.sort();
        assert_eq!(got, want);
    }

    /// One derivation for the knob half: both writers — the harness bridge
    /// and the slash-command fast path — spell the `id()` vocabulary through
    /// this, so the resume parses one spelling. Everything that is not a knob
    /// (the model pair, the two per-run facts) is left for the caller.
    #[test]
    fn from_knobs_spells_each_knob_by_its_id_and_leaves_the_rest_unset() {
        use crate::agents::thinking::ThinkLevel;
        use crate::config::types::policies::{ExecTier, SessionMode};
        use crate::memory::session_memory_mode::MemoryMode;

        let snap = RunEnvelopeSnapshot::from_knobs(
            Some(ExecTier::Ask),
            Some(SessionMode::Code),
            Some(ThinkLevel::High),
            Some(MemoryMode::Off),
        );
        assert_eq!(snap.exec_tier.as_deref(), Some(ExecTier::Ask.id()));
        assert_eq!(snap.session_mode.as_deref(), Some(SessionMode::Code.id()));
        assert_eq!(snap.think_level.as_deref(), Some(ThinkLevel::High.id()));
        assert_eq!(snap.memory_mode.as_deref(), Some(MemoryMode::Off.id()));
        assert_eq!(
            (snap.model, snap.model_provider, snap.allowed_tools, snap.btw),
            (None, None, None, None)
        );
        assert!(RunEnvelopeSnapshot::from_knobs(None, None, None, None).is_empty());
    }

    /// `{}` parses iff every field is `#[serde(default)]`: an envelope written
    /// by a build that had fewer fields must still decode on this one, or the
    /// row that carries it turns undecodable and refuses its whole session. A
    /// field added without the attribute turns this red.
    #[test]
    fn every_envelope_field_is_defaultable() {
        let e: RunEnvelopeSnapshot = serde_json::from_str("{}").unwrap();
        assert!(e.is_empty());
    }

    /// The four `custom`-bag names in that array are the ones the session
    /// snapshot's decoder actually reads — not just four strings that happen
    /// to match the struct. Feeds a metadata bag keyed by the array itself and
    /// asserts each value comes back out.
    #[test]
    fn the_custom_bag_names_in_the_array_are_the_ones_the_decoder_reads() {
        use crate::gateway::session_manager::SessionIdentityMeta;
        use crate::gateway::session_snapshot::{snapshot_from_metadata, RUN_ENVELOPE_KNOB_KEYS};
        use crate::gateway::session_store::types::SessionMetadata;

        let mut identity = SessionIdentityMeta::default();
        for (i, value) in ["full", "code", "high", "off"].iter().enumerate() {
            identity
                .custom
                .insert(RUN_ENVELOPE_KNOB_KEYS[i].to_string(), (*value).into());
        }
        let meta = SessionMetadata {
            identity_meta: Some(identity),
            model: Some("m".to_string()),
            model_provider: Some("p".to_string()),
            ..SessionMetadata::default()
        };
        let snap = snapshot_from_metadata(&meta);
        assert_eq!(snap.exec_tier.as_deref(), Some("full"));
        assert_eq!(snap.mode.as_deref(), Some("code"));
        assert_eq!(snap.think_level.as_deref(), Some("high"));
        assert_eq!(snap.memory_mode.as_deref(), Some("off"));
        assert_eq!(snap.model.as_deref(), Some("m"));
        assert_eq!(snap.model_provider.as_deref(), Some("p"));
    }

    #[test]
    fn run_finished_serde_round_trips_each_outcome() {
        for outcome in [
            RunOutcome::Completed,
            RunOutcome::Cancelled,
            RunOutcome::Errored,
            RunOutcome::Abandoned,
        ] {
            let ev = SessionEvent::RunFinished {
                run_id: "run-xyz".into(),
                outcome,
                at: 1_700_000_000_000,
            };
            let json = serde_json::to_string(&ev).unwrap();
            let back: SessionEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
            assert!(json.contains("\"type\":\"run_finished\""));
        }
    }

    #[test]
    fn a_project_rooms_user_message_carries_its_author() {
        let ev = SessionEvent::UserMessage {
            turn_id: TurnId::new_v4(),
            content: MessageContent {
                text: "ship it".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: 1,
            synthetic: false,
            author_user_id: Some("u-alice".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"author_user_id\":\"u-alice\""));
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn a_single_author_message_puts_no_author_on_the_wire() {
        // `skip_serializing_if` is not cosmetic here: these bytes sit in the
        // prompt's cached prefix, and a `"author_user_id":null` on every
        // message of every single-author session is a per-turn tax paid by the
        // sessions that get nothing back for it.
        let ev = SessionEvent::synthetic_user(TurnId::new_v4(), "hint".into());
        assert!(!serde_json::to_string(&ev)
            .unwrap()
            .contains("author_user_id"));
    }

    #[test]
    fn a_pre_p2_event_without_an_author_still_deserializes() {
        // On-disk session logs predate this field. `#[serde(default)]` is what
        // keeps every historical log readable, and reading it back as `None` is
        // adoption-by-absence: an unlabelled message belongs to the session's
        // own user, exactly as it always did.
        let legacy = r#"{"type":"user_message","turn_id":"11111111-1111-4111-8111-111111111111","content":{"text":"hi"},"at":7,"synthetic":false}"#;
        let ev: SessionEvent = serde_json::from_str(legacy).unwrap();
        match ev {
            SessionEvent::UserMessage {
                author_user_id,
                content,
                ..
            } => {
                assert_eq!(author_user_id, None);
                assert_eq!(content.text, "hi");
            }
            other => panic!("expected a user message, got {other:?}"),
        }
    }

    /// Rows written before 2026-09-12 carry `input_tokens` / `output_tokens`
    /// on the meta. The enum has no `deny_unknown_fields`, so they decode to
    /// the trimmed variant with the counters dropped on the floor — the fold
    /// re-derives them from the run's `AssistantMessage.usage`. Pinned so the
    /// counters are ignored rather than refused, by name. The same old row
    /// carries the three gauge numbers bare; they decode as `Some`.
    #[test]
    fn an_old_run_meta_with_token_counters_still_decodes() {
        let old = r#"{"type":"assistant_run_meta","turn_id":"11111111-1111-4111-8111-111111111111","run_id":"r-old","context_tokens":1234,"context_window":200000,"total_tokens":70,"input_tokens":45,"output_tokens":25,"cost_usd":0.12,"model":"claude","model_provider":"anthropic","at":3}"#;
        match serde_json::from_str::<SessionEvent>(old).unwrap() {
            SessionEvent::AssistantRunMeta {
                run_id,
                context_tokens,
                context_window,
                total_tokens,
                cost_usd,
                model,
                ..
            } => {
                assert_eq!(run_id, "r-old");
                assert_eq!(
                    (context_tokens, context_window, total_tokens),
                    (Some(1234), Some(200_000), Some(70))
                );
                assert_eq!(cost_usd, Some(0.12));
                assert_eq!(model.as_deref(), Some("claude"));
            }
            other => panic!("expected a run meta, got {other:?}"),
        }
    }

    /// A meta whose run resolved no gauge carries no gauge keys on the wire
    /// (`skip_serializing_if`), and a row without them decodes as `None` —
    /// absent, not zero, on both sides of the store.
    #[test]
    fn a_run_meta_without_a_gauge_keeps_the_keys_off_the_wire_and_decodes_as_none() {
        let meta = SessionEvent::AssistantRunMeta {
            turn_id: uuid::Uuid::new_v4(),
            run_id: "r-no-gauge".into(),
            context_tokens: None,
            context_window: None,
            total_tokens: None,
            cost_usd: None,
            model: None,
            model_provider: None,
            at: 3,
        };
        let json = serde_json::to_string(&meta).unwrap();
        for key in ["context_tokens", "context_window", "total_tokens"] {
            assert!(
                !json.contains(key),
                "{key} must be absent, not null: {json}"
            );
        }
        match serde_json::from_str::<SessionEvent>(&json).unwrap() {
            SessionEvent::AssistantRunMeta {
                run_id,
                context_tokens,
                context_window,
                total_tokens,
                ..
            } => {
                assert_eq!(run_id, "r-no-gauge");
                assert_eq!(
                    (context_tokens, context_window, total_tokens),
                    (None, None, None)
                );
            }
            other => panic!("expected a run meta, got {other:?}"),
        }
    }

    #[test]
    fn run_outcome_renames_snake_case() {
        assert_eq!(
            serde_json::to_string(&RunOutcome::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&RunOutcome::Abandoned).unwrap(),
            "\"abandoned\""
        );
    }

    /// `ParkReason` has two spellings of one fact: the serde word (what the
    /// wire and `as_str` say) and the `Display` clause (what the boundary
    /// repair reads to the model). They must differ — a wire word read aloud
    /// is not a sentence — and the event must round-trip under its own tag.
    #[test]
    fn park_reason_wire_word_is_the_serde_name_and_display_is_the_clause() {
        for r in ParkReason::ALL {
            assert_eq!(
                serde_json::to_value(r).unwrap(),
                serde_json::json!(r.as_str())
            );
            assert_ne!(r.as_str(), r.to_string());
        }
        let ev = SessionEvent::ToolCallParked {
            turn_id: TurnId::new_v4(),
            call_id: "c".into(),
            reason: ParkReason::PreHook,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"type\":\"tool_call_parked\"")
                && json.contains("\"reason\":\"pre_hook\""),
            "{json}"
        );
        let _: SessionEvent = serde_json::from_str(&json).unwrap();
    }

    /// `ParkReason::ALL` is a hand-written list, so the compiler does not
    /// force a new variant into it. This exhaustive match does: a new variant
    /// fails to compile here until it has an index, and the `seen` array then
    /// fails until `ALL` carries it exactly once.
    #[test]
    fn park_reason_all_lists_every_variant_once() {
        fn index(r: ParkReason) -> usize {
            match r {
                ParkReason::Approval => 0,
                ParkReason::Clarification => 1,
                ParkReason::PreHook => 2,
            }
        }
        let mut seen = [0usize; ParkReason::ALL.len()];
        for r in ParkReason::ALL {
            seen[index(r)] += 1;
        }
        assert!(seen.iter().all(|n| *n == 1), "{seen:?}");
    }

    // -----------------------------------------------------------------------
    // Durability policy census (U3)
    // -----------------------------------------------------------------------

    /// Every variant name declared in `pub enum SessionEvent`, read off this
    /// file's production text — the owning type, not a remembered list.
    fn variant_names_from_source() -> Vec<String> {
        let src = crate::utils::source_scan::production_text(
            std::path::Path::new(file!()),
            include_str!("events.rs"),
        );
        let src = crate::utils::source_scan::strip_comment_lines(&src);
        let body = src
            .split("pub enum SessionEvent {")
            .nth(1)
            .expect("enum present");
        let body = body.split("\n}").next().expect("enum closes");
        body.lines()
            .filter(|l| {
                l.starts_with("    ") && !l.starts_with("     ") && !l.trim_start().starts_with('#')
            })
            .filter_map(|l| l.trim().split([' ', '{', '(']).next().map(str::to_string))
            .filter(|n| n.chars().next().is_some_and(char::is_uppercase))
            .collect()
    }

    /// The Barrier column of the durability table is exactly the four events
    /// U3 ruled on, the sample list constructs every declared variant, nothing
    /// is ignorable yet, and `durability_of` has no wildcard arm to hide a
    /// new variant behind.
    #[test]
    fn durability_barrier_set_is_exactly_the_four_ruled_events() {
        let sample = fixtures::sample_of_every_kind();
        let mut sampled: Vec<&str> = sample.iter().map(|(n, _)| *n).collect();
        sampled.sort_unstable();
        let mut declared = variant_names_from_source();
        declared.sort_unstable();
        assert_eq!(
            sampled, declared,
            "sample_of_every_kind() must construct every variant"
        );
        let mut barrier: Vec<&str> = sample
            .iter()
            .filter(|(_, e)| durability_of(e) == Durability::Barrier)
            .map(|(n, _)| *n)
            .collect();
        barrier.sort_unstable();
        assert_eq!(
            barrier,
            [
                "ResumeAttempted",
                "RunStarted",
                "ToolCallRequested",
                "UserMessage"
            ]
        ); // U3
        assert!(
            sample.iter().all(|(_, e)| !ignorable(e)),
            "no event is ignorable this round"
        );
        let src = crate::utils::source_scan::production_text(
            std::path::Path::new(file!()),
            include_str!("events.rs"),
        );
        let fn_body = src
            .split("fn durability_of")
            .nth(1)
            .unwrap()
            .split("\n}")
            .next()
            .unwrap();
        assert!(
            !fn_body.contains("_ =>"),
            "durability_of must force a decision on every new variant"
        );
        assert_eq!(
            batch_durability(
                [
                    &sample[0].1,
                    &SessionEvent::UserMessage {
                        turn_id: uuid::Uuid::new_v4(),
                        content: MessageContent {
                            text: "u".into(),
                            blocks: vec![],
                            thinking: None,
                            thinking_signature: None,
                        },
                        at: 0,
                        synthetic: false,
                        author_user_id: None,
                    },
                ]
                .into_iter()
            ),
            Durability::Barrier
        );
    }
}
