// Core application state management for the TUI.
//
// Contains all state types (AppState, ChatMessage, Action, Focus, etc.)
// and the gateway event handler that maps StreamEvent -> state mutations.
// The two projection paths live in sibling modules: `events` (StreamEvent ->
// state) and `trace` (AgentTraceEvent -> state).

mod events;
mod trace;

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use aleph_protocol::providers::{rank_entries, CatalogEntry, RosterModel};
use aleph_protocol::{RunSummary, SessionSnapshot};
use chrono::{DateTime, Utc};

use super::btw_overlay::BtwOverlay;
use super::command_tree::{CommandEntry, DisplayEntry};
use super::slash::{LocalCommand, ToolProgressMode};

// ---------------------------------------------------------------------------
// Action
// ---------------------------------------------------------------------------

/// All possible actions that can result from user input or system events.
/// Actions are dispatched from the input handler and gateway event handler,
/// then consumed by the main loop to mutate state and trigger side effects.
#[derive(Debug)]
pub enum Action {
    /// No-op, nothing to do
    None,
    /// Quit the application
    Quit,
    /// Tick event (drives spinner animation, etc.)
    Tick,

    // -- Chat --
    /// Send a message to the agent
    SendMessage(String),
    /// Execute a local slash command (handled in TUI)
    LocalCommand(LocalCommand),
    /// Send a gateway command (slash command forwarded as chat message)
    GatewayCommand(String),
    /// Cancel a running agent run
    CancelRun(String),

    // -- Scrolling --
    /// Scroll the chat view up by N lines
    ScrollUp(usize),
    /// Scroll the chat view down by N lines
    ScrollDown(usize),
    /// Jump to the bottom of the chat
    ScrollToBottom,
    /// Scroll to bottom only if `auto_scroll` is enabled
    ScrollToBottomIfAutoScroll,

    // -- Focus --
    /// Focus the input textarea
    FocusInput,
    /// Focus the chat panel (for scrolling)
    FocusChat,

    // -- Overlays --
    /// Open the command palette
    OpenCommandPalette,
    /// Close any open overlay (palette, dialog)
    CloseOverlay,
    /// Move palette selection up
    PaletteUp,
    /// Move palette selection down
    PaletteDown,
    /// Confirm current palette selection
    PaletteConfirm,

    // -- Dialog response --
    /// Answer an `AskUser` dialog. Routed to `clarification.resolve` keyed by
    /// `session_key` (the reply routes by session, not by run).
    RespondToDialog { session_key: String, reply: String },

    // -- Tool approval (Ask exec tier) --
    /// Resolve the pending tool-approval overlay by option index into
    /// `APPROVAL_DECISIONS` (0 = allow once, 1 = allow session, 2 = deny).
    ResolveApproval { index: usize },

    // -- Side question (`/btw`) --
    /// Esc in the side-question overlay: abort the side run when one is still
    /// answering, close the overlay when none is. Aborting names the
    /// **overlay's own** run id — the screen's `current_run` is the main run,
    /// or nothing, so `/stop`'s helper would stop the wrong thing (or refuse).
    BtwAbortOrClose,
    /// Copy the shown side answer as raw markdown.
    BtwCopy,

    // -- Session picker --
    /// Move session-picker selection up
    SessionPickerUp,
    /// Move session-picker selection down
    SessionPickerDown,
    /// Confirm the selected session and switch to it
    SessionPickerConfirm,

    // -- Provider / model picker --
    /// Move provider-picker selection up
    ProviderPickerUp,
    /// Move provider-picker selection down
    ProviderPickerDown,
    /// Confirm the highlighted row: descend into a provider, or pin a model
    ProviderPickerConfirm,
    /// Ask the highlighted provider's vendor what it serves now, then re-read
    /// the catalogue so the discovered ids appear in its roster.
    ProviderPickerRefresh,
}

// ---------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------

/// Which UI panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Chat,
    CommandPalette,
    Dialog,
    SessionPicker,
    ProviderPicker,
    Approval,
    /// The `/btw` side-question overlay.
    Btw,
}

// ---------------------------------------------------------------------------
// Tool execution tracking
// ---------------------------------------------------------------------------

/// Current status of a tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Failed,
    /// The run ended without a terminal result ever reaching this row.
    ///
    /// Live tool frames ride the deliberately-lossy `agent_trace` mirror
    /// (bounded mpsc + `try_send`), so a busy run can drop a
    /// `ToolCallCompleted`. `RunComplete` reconciles against the authoritative
    /// `summary.tool_summaries`; anything still `Running` after that had no
    /// authoritative record either. Render it as unknown rather than guessing
    /// success — a spinner that never stops reads as "still working", which is
    /// the one thing it definitely is not.
    Unknown,
}

/// State of a single tool execution within an assistant message.
#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub id: String,
    pub name: String,
    pub params: String,
    pub status: ToolStatus,
    pub duration: Option<Duration>,
    pub progress: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Chat messages
// ---------------------------------------------------------------------------

/// A single message in the chat history.
#[derive(Debug, Clone)]
pub enum ChatMessage {
    User {
        content: String,
        timestamp: DateTime<Utc>,
    },
    Assistant {
        content: String,
        tools: Vec<ToolExecution>,
        reasoning: Option<String>,
        is_streaming: bool,
    },
    System {
        content: String,
    },
}

// ---------------------------------------------------------------------------
// Overlay state
// ---------------------------------------------------------------------------

/// State for the `AskUser` confirmation dialog.
///
/// # Why there is a text buffer here
///
/// The overlay used to render a menu and nothing else. A question with no
/// choices — `ask_user` accepts one, and free text is *always* a legal answer
/// even when choices are offered — arrived as an options list of length zero,
/// so `Enter` resolved `options.get(selected)` to `None` and did nothing, every
/// digit key fell through, and `Esc` is deliberately swallowed for this overlay
/// (the run is parked on a oneshot). The result was an unanswerable modal
/// holding the whole TUI: the ways out were Ctrl+C (cancel the run) and
/// Ctrl+C twice / Ctrl+D (quit) — abandoning the work was possible, answering
/// the question was not. A parking tool's client must be able to produce every
/// answer the server accepts.
#[derive(Debug, Clone)]
pub struct DialogState {
    /// Clarification registry key — the answer is posted back to
    /// `clarification.resolve` against this, NOT the run id (replies route by
    /// session). Carried on the `AskUser` frame.
    pub session_key: String,
    pub question: String,
    pub options: Vec<String>,
    pub selected: usize,
    /// Several picks are accepted, comma-separated.
    pub multi_select: bool,
    /// The answer is a credential: the buffer is rendered masked and never
    /// echoed. Purely a display property here — the transport rule that makes
    /// `secret` load-bearing is enforced server-side in `clarification::ask`,
    /// which is why a secret question can only ever reach this overlay.
    pub secret: bool,
    /// The typed answer, sent verbatim. Core's `interpret_reply` is the only
    /// interpreter, so a typed `2` and a pressed `2` are the same bytes.
    pub input: String,
    /// Keys go to [`Self::input`] rather than the menu.
    ///
    /// Structural at open time (see [`DialogState::has_quick_pick`]) and
    /// toggled by Tab afterwards, so it cannot be derived from
    /// `input.is_empty()`: clearing the buffer would silently drop a free-text
    /// question back into a menu it does not have.
    pub typing: bool,
}

impl DialogState {
    /// Whether one keypress can answer the question outright.
    ///
    /// Mirrors the server's own rule for suppressing an inline keyboard
    /// (`clarification::render::keyboard_for`): a single index cannot express a
    /// multi-select answer, so offering one would render a control that
    /// silently answers less than the question asks. Same predicate, so the
    /// terminal and a messaging channel cannot disagree about it.
    #[must_use]
    pub const fn has_quick_pick(&self) -> bool {
        !self.options.is_empty() && !self.multi_select
    }

    /// The reply `Enter` would send right now, if any.
    ///
    /// Driven by the mode, not by whether the buffer happens to be empty: a
    /// user who typed something and then tabbed back to the menu means the
    /// menu, and a blank buffer in text mode means "nothing to send yet"
    /// rather than "send the highlight".
    ///
    /// A pick sends the **1-based index**, not the label. Labels carry their
    /// `— description` suffix, and core's `interpret_reply` matches labels
    /// exactly, so a described choice sent by label arrives as free text with
    /// no selected index — the answer looks right to a human and is `custom`
    /// to the model. The index is also byte-identical to what the Panel and a
    /// channel's `clarify:<n>` button send, which keeps `interpret_reply` the
    /// single interpreter for all three surfaces.
    #[must_use]
    pub fn pending_reply(&self) -> Option<String> {
        if self.typing {
            let typed = self.input.trim();
            return (!typed.is_empty()).then(|| typed.to_string());
        }
        (self.selected < self.options.len()).then(|| (self.selected + 1).to_string())
    }
}

/// The question an `AskUser` frame is asking, flattened for the overlay.
///
/// A struct rather than four positional arguments because two of them are
/// adjacent bools: `show_dialog(key, q, opts, true, false)` reads identically
/// whichever way round the last two go, and one of them decides whether the
/// answer is masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskDialogView {
    /// Prompt as rendered — position marker, header chip, and hint included.
    pub question: String,
    /// Choice labels, already carrying their `— description` suffix. Empty for
    /// a free-text question.
    pub options: Vec<String>,
    /// Several picks accepted.
    pub multi_select: bool,
    /// Mask the typed answer.
    pub secret: bool,
}

/// Every decision the overlay knows how to render, in display order. Each is
/// `(label, wire_decision)`; the wire value is exactly what
/// `exec.approval.resolve` expects (kebab-case, server-validated).
///
/// This is the vocabulary, **not** the offer: which of these a given card may
/// use is the server's call (`exec::allowed_decisions::for_confirm_gate`),
/// carried on the pending record and enforced when the answer comes back. See
/// [`ApprovalState::decisions`].
pub const APPROVAL_DECISIONS: [(&str, &str); 4] = [
    ("Allow once", "allow-once"),
    ("Allow for session", "allow-session"),
    ("Always allow", "allow-always"),
    ("Deny", "deny"),
];

/// What a card offers when the server did not say (a core older than
/// 2026-08-11). The historical three, and never `allow-always`: a missing
/// field may narrow what a client offers, never widen it.
pub const DEFAULT_APPROVAL_DECISIONS: [(&str, &str); 3] = [
    ("Allow once", "allow-once"),
    ("Allow for session", "allow-session"),
    ("Deny", "deny"),
];

/// The renderable decisions for a card, given the wire values the server said
/// it may offer. Unknown wire values are dropped (a client cannot render a
/// decision it has no label for), and an empty result falls back to
/// [`DEFAULT_APPROVAL_DECISIONS`] — a card with no buttons is unanswerable,
/// which is worse than a card with the historical ones.
#[must_use]
pub fn offered_decisions(allowed: &[String]) -> Vec<(&'static str, &'static str)> {
    let offered: Vec<(&'static str, &'static str)> = APPROVAL_DECISIONS
        .iter()
        .filter(|(_, wire)| allowed.iter().any(|a| a == wire))
        .copied()
        .collect();
    if offered.is_empty() {
        DEFAULT_APPROVAL_DECISIONS.to_vec()
    } else {
        offered
    }
}

/// State for the tool-execution approval overlay (Ask exec tier). A parked
/// server run is waiting on `exec.approval.resolve` for this `id`. Kept
/// deliberately separate from [`DialogState`] (AskUser) so a security decision
/// can never be routed to `clarification.resolve` (the AskUser answer path) by
/// mistake — approvals resolve through `exec.approval.resolve` alone.
#[derive(Debug, Clone)]
pub struct ApprovalState {
    /// Approval id — the resolve key. Never shown to the user.
    pub id: String,
    /// Human-readable action being gated (the tool/command summary).
    pub command: String,
    /// Why the gate fired, when the server supplied a reason.
    pub reason: Option<String>,
    /// Highlighted decision index into [`Self::decisions`].
    pub selected: usize,
    /// The decisions THIS card offers, in display order — from the record's
    /// `allowed_decisions`, not from a fixed list. A card raised on a tool that
    /// declares its own confirmation floor does not offer "always"; a member's
    /// card does not either. Rendering a fixed set would put a button in front
    /// of the user that the server narrows on arrival.
    pub decisions: Vec<(&'static str, &'static str)>,
}

/// State for the command palette overlay.
#[derive(Debug, Clone)]
pub struct PaletteState {
    pub input: String,
    /// The argument tail of `input` — everything after the first whitespace,
    /// when the part before it was enough to narrow the list on its own (see
    /// [`super::command_tree::split_palette_input`]).
    ///
    /// Resolved by `recompute_palette_filter` at the same moment as
    /// `filtered`, and read by the confirm path, so the two cannot disagree
    /// about whether a word was a search term or an argument.
    pub args: String,
    pub filtered: Vec<DisplayEntry>,
    pub selected: usize,
    /// Stack of namespace names we have browsed into (e.g. `["session"]`)
    pub namespace_stack: Vec<String>,
}

impl PaletteState {
    /// The command line to run for the current selection: the selected
    /// entry's `full_command`, plus whatever arguments the input carried.
    ///
    /// `None` when the filter matched nothing — confirming an empty list is a
    /// no-op, not a guess.
    #[must_use]
    pub fn selected_command(&self) -> Option<String> {
        let entry = self.filtered.get(self.selected)?;
        let command = entry.full_command.trim();
        Some(if self.args.is_empty() {
            command.to_string()
        } else {
            format!("{command} {}", self.args)
        })
    }
}

/// A single browsable session in the resume/switch picker.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// The session key used by `chat.history` / to re-point the session.
    pub key: String,
    /// Human-facing label (name + message count, or the key as fallback).
    pub label: String,
}

/// State for the session resume/switch picker overlay. Populated from
/// `sessions.list`; confirming an entry loads `chat.history` and switches.
#[derive(Debug, Clone)]
pub struct SessionPickerState {
    pub input: String,
    pub entries: Vec<SessionEntry>,
    /// Indices into `entries` surviving the current input filter.
    pub filtered: Vec<usize>,
    pub selected: usize,
}

/// One row on screen in the provider picker.
///
/// Two levels share one list, the way the palette's namespace stack does: the
/// provider level offers rows to descend into, the model level offers ids to
/// pin.
///
/// `PartialEq` without `Eq`: a model row carries a [`RateCard`], and a price is
/// a float. Nothing here is a map key.
///
/// [`RateCard`]: aleph_protocol::providers::RateCard
#[derive(Debug, Clone, PartialEq)]
pub enum PickerRow {
    Provider {
        /// Index into [`ProviderPickerState::entries`].
        index: usize,
        /// How many of that provider's models the current filter matched. Equal
        /// to the roster length unless the row surfaced *because* a model id
        /// matched — the shared ranker narrows the roster in that case, and a
        /// provider found through one of its models should not then hide which.
        matched: usize,
    },
    Model {
        model: RosterModel,
    },
}

/// What confirming the highlighted row means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderPick {
    /// Descend into this provider's roster.
    Provider(usize),
    /// Pin this model id.
    Model(String),
}

/// State for the provider/model picker overlay (`/providers`).
///
/// Holds the rows `providers.catalog` returned, verbatim. This client never
/// re-derives a roster, re-ranks by cost or capability, or keeps a provider
/// table of its own (R4) — the merge behind `roster` includes an "operator
/// moved `base_url` ⇒ drop the curated rungs" rule a frontend has no way to
/// evaluate. Filtering goes through `aleph_protocol::providers::search`, the
/// same matcher the Panel's picker uses, so a bare Enter selects the same row
/// in both.
#[derive(Debug, Clone)]
pub struct ProviderPickerState {
    /// Filter text. Carried across a descent on purpose: the ranker searches
    /// provider ids, aliases, display names **and** model ids at both levels,
    /// so `gpt-5` → Enter lands on the gpt-5 rows rather than on a roster the
    /// user then has to re-narrow.
    pub input: String,
    /// The catalogue as the server sent it.
    pub entries: Vec<CatalogEntry>,
    /// Which provider we have descended into (index into `entries`), or `None`
    /// at the provider level.
    pub provider: Option<usize>,
    /// The rows on screen for `input` at the current level.
    pub rows: Vec<PickerRow>,
    pub selected: usize,
}

impl ProviderPickerState {
    /// The catalogue row Ctrl+R would act on, or `None` when there is none.
    ///
    /// Lives on the picker state, not on [`AppState`], so the footer that
    /// *advertises* the key and the handler that *performs* it resolve the row
    /// through one function. Two independent index walks — one in the widget,
    /// one in the command — would agree today and diverge the first time either
    /// level's row list changes shape.
    #[must_use]
    pub fn refresh_target(&self) -> Option<&CatalogEntry> {
        let index = match self.provider {
            Some(index) => index,
            None => match self.rows.get(self.selected)? {
                PickerRow::Provider { index, .. } => *index,
                // A model row at the provider level cannot happen (that level
                // only emits provider rows), but answering `None` is the
                // honest reading rather than an unwrap.
                PickerRow::Model { .. } => return None,
            },
        };
        self.entries.get(index)
    }
}

/// A catalogue row for tests, built from the contract type so a field rename
/// on the wire breaks every picker test at once rather than none of them.
///
/// Shared by the state tests here and the widget tests in
/// `widgets::provider_picker`; both need a whole `CatalogEntry` and neither
/// should be hand-writing twenty fields to get one.
#[cfg(test)]
#[must_use]
pub(crate) fn sample_catalog_entry(id: &str, models: &[&str]) -> CatalogEntry {
    use aleph_protocol::providers::{AuthKind, ModelSource};

    CatalogEntry {
        id: id.to_string(),
        display_name: id.to_uppercase(),
        default_model: models.first().copied().unwrap_or_default().to_string(),
        base_url: String::new(),
        protocol: "openai".to_string(),
        color: String::new(),
        homepage: None,
        notes: None,
        signup_url: None,
        aliases: Vec::new(),
        modalities: Vec::new(),
        models: Vec::new(),
        has_api_key: false,
        verified: false,
        enabled: false,
        is_default: false,
        auth_kind: AuthKind::ApiKey,
        endpoint: "cloud".to_string(),
        requires_explicit_model: false,
        discoverable: true,
        roster: models
            .iter()
            .map(|m| RosterModel::new(*m, ModelSource::PresetFallback))
            .collect(),
    }
}

/// Rank the catalogue for one level of the picker.
///
/// A free function so it is testable on its own, and so the two levels
/// provably share one matcher: the model level ranks a one-element slice, which
/// gives it the same rule the level above has — a query naming the provider
/// keeps the whole roster, a query naming a model narrows to it.
#[must_use]
pub fn provider_picker_rows(
    entries: &[CatalogEntry],
    provider: Option<usize>,
    query: &str,
) -> Vec<PickerRow> {
    match provider {
        None => rank_entries(entries, query)
            .into_iter()
            .map(|m| PickerRow::Provider {
                index: m.index,
                matched: m.models.len(),
            })
            .collect(),
        Some(index) => entries.get(index).map_or_else(Vec::new, |entry| {
            rank_entries(std::slice::from_ref(entry), query)
                .into_iter()
                .flat_map(|m| m.models)
                .map(|model| PickerRow::Model { model })
                .collect()
        }),
    }
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Central application state. Owned by the main loop, mutated through
/// methods that enforce invariants (e.g. `auto_scroll` toggling).
/// Which per-session knob a local command just wrote.
///
/// One enum rather than five setters so the status bar and the write paths
/// enumerate the same list — a knob added here without a renderer is a compile
/// error in the `match`, not a silently invisible setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKnob {
    Mode,
    ExecTier,
    ThinkLevel,
    MemoryMode,
}

/// The four session knobs as the status bar reads them. Borrowed from the
/// snapshot so the renderer cannot hold a stale copy across an attach.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionKnobs<'a> {
    pub mode: Option<&'a str>,
    pub exec_tier: Option<&'a str>,
    pub think_level: Option<&'a str>,
    pub memory_mode: Option<&'a str>,
}

#[derive(Debug)]
pub struct AppState {
    // -- Chat --
    pub messages: Vec<ChatMessage>,
    pub scroll_offset: usize,
    pub auto_scroll: bool,

    // -- Input history --
    pub send_history: Vec<String>,
    pub history_index: Option<usize>,

    // -- Session / model --
    pub session_key: String,
    /// The model name to display. Seeded from the gateway's default provider at
    /// launch and replaced by the conversation's own model the moment a
    /// [`SessionSnapshot`] arrives — a thread pinned with `select_model` must
    /// not be captioned with the install-wide default it overrode.
    pub model_name: String,
    /// Fallback model caption: the gateway's default-provider model, kept so a
    /// snapshot that names no model (a conversation that has not run yet) can
    /// restore the caption instead of leaving whatever the previous session had.
    default_model_name: String,
    pub total_tokens: u64,
    /// The conversation's durable settings as the server last reported them
    /// (`chat.history`'s `session` object). `None` until the first attach — and
    /// `None` is read as "the server has not told me", never as "nothing is
    /// set": an unset knob and an unknown knob look identical to a user, but
    /// only one of them is safe to render as a concrete value.
    ///
    /// Stored whole rather than fanned out into sibling fields: the snapshot is
    /// one fact with one producer, and six copies of it are six chances for a
    /// `switch_session` to reset five of them.
    pub session_snapshot: Option<SessionSnapshot>,
    /// Live context-window occupancy `(used_tokens, window_tokens)` from the
    /// latest `ContextGauge` event. `None` until the session's first gauge
    /// arrives. The pair always travels together (one event), so a single
    /// `Option` keeps the half-known state unrepresentable; the denominator is
    /// server-authoritative per model.
    pub context_gauge: Option<(u32, u32)>,
    /// Last-call prompt-cache hit rate as a rounded percentage (0–100), from
    /// the latest `ProviderUsage` trace event that reported cache activity,
    /// computed via `aleph_protocol::cache_hit_ratio` (`read / (input + read)`,
    /// the single canonical formula shared with core and the Panel Usage
    /// view). `None` until a call reports cache tokens — providers without
    /// prompt caching never surface a misleading 0%. Last-call (not
    /// cumulative) on purpose: a sudden drop is what tells you a prefix bust
    /// just happened.
    pub cache_stat: Option<u64>,
    /// Agent id behind `cache_stat` when it did *not* come from the session's
    /// root agent — sub-agents and MoA advisors are metered on the same trace
    /// stream, so a delegated cold start would otherwise silently overwrite
    /// the root agent's healthy reading with someone else's number. Rendered
    /// as a suffix so a mixed reading is labelled rather than misattributed.
    pub cache_stat_agent: Option<String>,
    /// First agent id observed since the session was (re)set. The root agent
    /// necessarily makes the first LLM call — it cannot delegate before it has
    /// taken a turn — so this identifies it without the TUI needing to know
    /// the run topology.
    cache_root_agent: Option<String>,
    pub is_connected: bool,

    /// `(run_id, session_key)` pairs learned from every `RunAccepted` this
    /// screen has observed, own or foreign. Every other run-scoped frame is
    /// checked against this before it is applied (see
    /// `events::run_scoped_id` and `frame_belongs_here`), which compares the
    /// recorded session against [`Self::session_key`] *at check time* — not
    /// baked in at learn time — so a run correctly resumes appearing here if
    /// a `/session` switch leaves and later returns to its actual session. A
    /// run id never recorded here is treated as "unknown", which counts as
    /// "kept", not "foreign": proof is required to drop, absence of proof is
    /// not.
    ///
    /// FIFO-bounded (`events::RUN_SESSION_CAP`) so a long-lived screen next
    /// to a busy install does not grow this without limit.
    run_sessions: std::collections::VecDeque<(String, String)>,

    // -- Run tracking --
    pub current_run: Option<String>,
    /// Wall-clock start of the active run (set on `RunAccepted`, cleared on any
    /// run-end). Drives the status-bar working indicator's elapsed timer.
    pub run_started_at: Option<Instant>,
    pub last_run_duration: Option<Duration>,
    pub current_run_uses_agent_trace: bool,
    pub current_run_trace_summary_applied: bool,
    /// Bytes of the current turn's assistant text already appended by
    /// `ResponseChunk` deltas.
    ///
    /// A streaming turn produces the same text TWICE on the wire: as
    /// `ResponseChunk` deltas and again, in full, as
    /// `AgentTrace{TextEmitted{Final}}` (`harness/agent/think.rs` emits the
    /// latter unconditionally — its `response_was_streamed` guard only
    /// suppresses a second in-process `on_delta`). The TUI subscribes to no
    /// topics, so the gateway's `should_receive` gives it both. Appending both
    /// doubled the answer inside one bubble.
    ///
    /// Reset per turn (`TurnStarted`) and per run (`RunAccepted`); the final
    /// text appends only its un-streamed suffix, so a non-streamed turn (mock
    /// provider, output guardrail) still lands in full.
    pub turn_streamed_len: usize,

    // -- Settings --
    pub verbose: bool,
    pub tool_progress_mode: ToolProgressMode,

    // -- Gateway commands (fetched at startup, tree-structured) --
    pub gateway_commands: Vec<CommandEntry>,

    // -- UI state --
    pub focus: Focus,
    pub dialog: Option<DialogState>,
    pub palette: Option<PaletteState>,
    pub session_picker: Option<SessionPickerState>,
    /// Provider/model picker overlay (`/providers`), populated from
    /// `providers.catalog`.
    pub provider_picker: Option<ProviderPickerState>,
    /// Pending tool-approval overlay (Ask exec tier), if one is being shown.
    /// Surfaced by the `exec.approvals.pending` poll, resolved via
    /// `exec.approval.resolve`.
    pub approval: Option<ApprovalState>,
    /// The `/btw` side-question overlay.
    ///
    /// Not an `Option` like its siblings: its history outlives any one
    /// showing (closing it hides it, the next `/btw` reopens onto what was
    /// already asked), and its run claims have to answer `accepts_frame` for
    /// frames that arrive after the user closed it. `BtwOverlay::open` says
    /// whether it is on screen.
    pub btw: BtwOverlay,

    // -- Control --
    pub ctrl_c_count: u8,
    pub spinner_frame: usize,
    pub should_quit: bool,
}

impl AppState {
    /// Create a new `AppState` with a welcome system message.
    pub fn new(session_key: String, model_name: String) -> Self {
        // An empty key is not a key — it means "the gateway has not routed this
        // conversation yet". Printing it verbatim renders `Session:  |`, which
        // reads like a bug; naming the state reads like the truth.
        let session_line = if session_key.is_empty() {
            "Session: new (the gateway names it on your first message)".to_string()
        } else {
            format!("Session: {session_key}")
        };
        let welcome = format!(
            "Welcome to Aleph CLI. {session_line} | Model: {model_name}. Type /help for commands."
        );
        Self {
            messages: vec![ChatMessage::System { content: welcome }],
            scroll_offset: 0,
            auto_scroll: true,

            send_history: Vec::new(),
            history_index: None,

            session_key,
            default_model_name: model_name.clone(),
            model_name,
            total_tokens: 0,
            session_snapshot: None,
            context_gauge: None,
            cache_stat: None,
            cache_stat_agent: None,
            cache_root_agent: None,
            is_connected: true,
            run_sessions: std::collections::VecDeque::new(),

            current_run: None,
            run_started_at: None,
            last_run_duration: None,
            current_run_uses_agent_trace: false,
            turn_streamed_len: 0,
            current_run_trace_summary_applied: false,

            verbose: false,
            tool_progress_mode: ToolProgressMode::default(),
            gateway_commands: Vec::new(),

            focus: Focus::Input,
            dialog: None,
            palette: None,
            session_picker: None,
            provider_picker: None,
            approval: None,
            btw: BtwOverlay::default(),

            ctrl_c_count: 0,
            spinner_frame: 0,
            should_quit: false,
        }
    }

    // -- Message helpers ------------------------------------------------

    /// Add a user message to the chat history.
    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(ChatMessage::User {
            content,
            timestamp: Utc::now(),
        });
        if self.auto_scroll {
            self.scroll_offset = 0;
        }
    }

    /// Add a system message to the chat history.
    pub fn add_system_message(&mut self, content: String) {
        self.messages.push(ChatMessage::System { content });
        if self.auto_scroll {
            self.scroll_offset = 0;
        }
    }

    /// Ensure the last message is an assistant message. If the last message
    /// is not an assistant message (or there are no messages), appends a new
    /// empty assistant message. This is idempotent: calling it twice in a row
    /// will not create a second empty assistant message.
    pub fn ensure_assistant_message(&mut self) {
        if !matches!(self.messages.last(), Some(ChatMessage::Assistant { .. })) {
            self.messages.push(ChatMessage::Assistant {
                content: String::new(),
                tools: Vec::new(),
                reasoning: None,
                is_streaming: true,
            });
        }
    }

    /// Return a mutable reference to the last assistant message.
    /// If none exists, defensively creates one first.
    pub fn current_assistant_mut(&mut self) -> &mut ChatMessage {
        self.ensure_assistant_message();
        // `ensure_assistant_message` guarantees an assistant message exists,
        // so the search from the end always succeeds. Resolve the index with an
        // immutable borrow first to avoid a double mutable borrow of `messages`.
        let idx = self
            .messages
            .iter()
            .rposition(|m| matches!(m, ChatMessage::Assistant { .. }))
            .unwrap_or_else(|| self.messages.len().saturating_sub(1));
        &mut self.messages[idx]
    }

    /// Find a tool execution by `tool_id` in the last assistant message.
    /// Returns None if not found or last message is not assistant.
    pub fn find_tool_mut(&mut self, tool_id: &str) -> Option<&mut ToolExecution> {
        // Search from the end to find the most recent assistant message
        for msg in self.messages.iter_mut().rev() {
            if let ChatMessage::Assistant { tools, .. } = msg {
                return tools.iter_mut().find(|t| t.id == tool_id);
            }
        }
        None
    }

    // -- Scrolling ------------------------------------------------------

    /// Scroll up by `n` lines. Disables `auto_scroll`.
    pub const fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.auto_scroll = false;
    }

    /// Scroll down by `n` lines. If offset reaches 0, re-enables `auto_scroll`.
    pub const fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if self.scroll_offset == 0 {
            self.auto_scroll = true;
        }
    }

    /// Jump to the bottom of the chat. Re-enables `auto_scroll`.
    pub const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = true;
    }

    // -- Overlays -------------------------------------------------------

    /// Return display entries for the current palette browse level.
    /// At root: local commands + gateway root entries.
    /// Inside a namespace: that namespace's children.
    pub fn palette_display_entries(&self, namespace_stack: &[String]) -> Vec<DisplayEntry> {
        if namespace_stack.is_empty() {
            // Root level: local commands + gateway root entries
            let mut entries: Vec<DisplayEntry> = super::slash::local_commands()
                .into_iter()
                .map(|(n, d)| DisplayEntry {
                    label: n.to_string(),
                    hint: d.to_string(),
                    is_namespace: false,
                    full_command: format!("{n} "),
                })
                .collect();
            entries.extend(CommandEntry::root_display_entries(&self.gateway_commands));
            entries
        } else {
            // Inside a namespace: drill down through the stack
            let mut current_entries = &self.gateway_commands;
            let mut found_ns: Option<&CommandEntry> = None;
            let empty: Vec<DisplayEntry> = Vec::new();

            for ns_name in namespace_stack {
                found_ns = current_entries
                    .iter()
                    .find(|e| e.is_namespace && e.name.eq_ignore_ascii_case(ns_name));
                if let Some(ns) = found_ns {
                    current_entries = &ns.children;
                } else {
                    return empty;
                }
            }

            found_ns.map_or_else(Vec::new, |ns| {
                let path = namespace_stack.join(" ");
                CommandEntry::namespace_display_entries(ns, &path)
            })
        }
    }

    /// Filter display entries by a prefix string (the palette input text).
    pub fn filter_display_entries(
        &self,
        namespace_stack: &[String],
        filter: &str,
    ) -> Vec<DisplayEntry> {
        let all = self.palette_display_entries(namespace_stack);
        if filter.is_empty() {
            return all;
        }
        let filter_lower = filter.to_lowercase();
        let mut matched: Vec<DisplayEntry> = all
            .into_iter()
            .filter(|e| {
                e.label.to_lowercase().contains(&filter_lower)
                    || e.hint.to_lowercase().contains(&filter_lower)
            })
            .collect();
        // Stable sort, so catalog order survives inside each rank — see
        // `command_tree::filter_rank` for why an exact name has to win.
        matched.sort_by_key(|e| super::command_tree::filter_rank(&e.label, filter));
        matched
    }

    /// Open the command palette, pre-populated with root-level commands.
    pub fn open_command_palette(&mut self) {
        let all = self.palette_display_entries(&[]);
        self.palette = Some(PaletteState {
            input: String::new(),
            args: String::new(),
            filtered: all,
            selected: 0,
            namespace_stack: Vec::new(),
        });
        self.focus = Focus::CommandPalette;
    }

    /// Enter a namespace in the palette, showing its children.
    pub fn palette_enter_namespace(&mut self, ns_name: &str) {
        // Build the new stack, then compute entries without holding a mutable borrow
        let new_stack = {
            let Some(palette) = &self.palette else {
                return;
            };
            let mut stack = palette.namespace_stack.clone();
            stack.push(ns_name.to_string());
            stack
        };
        let entries = self.palette_display_entries(&new_stack);
        if let Some(palette) = &mut self.palette {
            palette.namespace_stack = new_stack;
            palette.input.clear();
            // The args belong to the input that was just cleared; carrying
            // them into the namespace would attach a word the user typed at
            // one level to a command chosen at another.
            palette.args.clear();
            palette.selected = 0;
            palette.filtered = entries;
        }
    }

    /// Go back one level in the palette namespace stack.
    /// Returns true if we went back, false if already at root.
    pub fn palette_go_back(&mut self) -> bool {
        // Build the new stack, then compute entries without holding a mutable borrow
        let new_stack = {
            let Some(palette) = &self.palette else {
                return false;
            };
            if palette.namespace_stack.is_empty() {
                return false;
            }
            let mut stack = palette.namespace_stack.clone();
            stack.pop();
            stack
        };
        let entries = self.palette_display_entries(&new_stack);
        if let Some(palette) = &mut self.palette {
            palette.namespace_stack = new_stack;
            palette.input.clear();
            palette.selected = 0;
            palette.filtered = entries;
        }
        true
    }

    /// Where focus lands when a modal that was covering the screen goes away.
    ///
    /// Not unconditionally `Input`. The `/btw` overlay is the one surface that
    /// can still be **on screen** when another modal closes: a clarification
    /// or an approval card can open over it (both are raised by frames, not by
    /// keys, so neither needs the user to have left the overlay first), and
    /// answering one used to hand focus to the composer while the side
    /// question stayed painted over the transcript — visible, unreachable, and
    /// unclosable, because `Esc` at `Focus::Input` is a no-op and every key
    /// went to a textarea the user could not see.
    const fn focus_after_modal(&self) -> Focus {
        if self.btw.open {
            Focus::Btw
        } else {
            Focus::Input
        }
    }

    /// Close any open overlay (palette, dialog, session picker, provider
    /// picker) and return focus to whatever is still on screen.
    pub fn close_overlay(&mut self) {
        self.palette = None;
        self.dialog = None;
        self.session_picker = None;
        self.provider_picker = None;
        self.approval = None;
        self.focus = self.focus_after_modal();
    }

    // -- Session picker -------------------------------------------------

    /// Open the session picker with entries fetched from `sessions.list`.
    pub fn open_session_picker(&mut self, entries: Vec<SessionEntry>) {
        let filtered = (0..entries.len()).collect();
        self.session_picker = Some(SessionPickerState {
            input: String::new(),
            entries,
            filtered,
            selected: 0,
        });
        self.focus = Focus::SessionPicker;
    }

    /// Recompute the session picker's filtered index list from its input text.
    pub fn recompute_session_filter(&mut self) {
        if let Some(picker) = &mut self.session_picker {
            let filter = picker.input.to_lowercase();
            picker.filtered = picker
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    filter.is_empty()
                        || e.label.to_lowercase().contains(&filter)
                        || e.key.to_lowercase().contains(&filter)
                })
                .map(|(i, _)| i)
                .collect();
            picker.selected = 0;
        }
    }

    /// The session key currently highlighted in the picker, if any.
    pub fn selected_session_key(&self) -> Option<String> {
        let picker = self.session_picker.as_ref()?;
        let idx = *picker.filtered.get(picker.selected)?;
        picker.entries.get(idx).map(|e| e.key.clone())
    }

    // -- Provider / model picker ----------------------------------------

    /// Open the provider picker over the rows `providers.catalog` returned,
    /// pre-filtered by `query` (`/providers <query>`).
    pub fn open_provider_picker(&mut self, entries: Vec<CatalogEntry>, query: String) {
        self.provider_picker = Some(ProviderPickerState {
            input: query,
            entries,
            provider: None,
            rows: Vec::new(),
            selected: 0,
        });
        self.recompute_provider_filter();
        self.focus = Focus::ProviderPicker;
    }

    /// Recompute the rows on screen from the current filter and level.
    pub fn recompute_provider_filter(&mut self) {
        let Some(picker) = &mut self.provider_picker else {
            return;
        };
        picker.rows = provider_picker_rows(&picker.entries, picker.provider, &picker.input);
        picker.selected = 0;
    }

    /// Descend into a provider's roster.
    pub fn enter_provider(&mut self, index: usize) {
        if let Some(picker) = &mut self.provider_picker {
            picker.provider = Some(index);
        }
        self.recompute_provider_filter();
    }

    /// Go back to the provider level. `false` when already there, so the caller
    /// can fall through to closing the overlay.
    pub fn provider_picker_go_back(&mut self) -> bool {
        let Some(picker) = &mut self.provider_picker else {
            return false;
        };
        if picker.provider.is_none() {
            return false;
        }
        picker.provider = None;
        self.recompute_provider_filter();
        true
    }

    /// The provider id a refresh would ask about, if any.
    ///
    /// At the model level that is the provider we descended into; at the
    /// provider level it is the highlighted row. `None` when nothing is
    /// highlighted — refreshing "whatever" is a guess, and this key dials a
    /// vendor.
    #[must_use]
    pub fn provider_picker_refresh_target(&self) -> Option<String> {
        Some(self.provider_picker.as_ref()?.refresh_target()?.id.clone())
    }

    /// Replace the catalogue behind the picker after a refetch.
    ///
    /// Re-resolves the descended-into provider **by id**, not by index: the
    /// server sorts and filters the catalogue, so a row's position is not a
    /// stable handle across two calls. Keeping the index would silently move
    /// the open roster to a different vendor. The filter text is preserved
    /// because the user is mid-search.
    pub fn replace_provider_catalog(&mut self, entries: Vec<CatalogEntry>) {
        let Some(picker) = &mut self.provider_picker else {
            return;
        };
        let open_id = picker
            .provider
            .and_then(|i| picker.entries.get(i))
            .map(|e| e.id.clone());
        // Also by id: the row under the cursor is the one the user just asked
        // about, and `recompute_provider_filter` resets the cursor to the top.
        // Refreshing a provider and then finding yourself looking at a
        // different one is the same class of surprise as the open roster
        // moving — just one level up.
        let cursor_id = match picker.rows.get(picker.selected) {
            Some(PickerRow::Provider { index, .. }) => {
                picker.entries.get(*index).map(|e| e.id.clone())
            }
            _ => None,
        };

        picker.provider = open_id.and_then(|id| entries.iter().position(|e| e.id == id));
        picker.entries = entries;
        self.recompute_provider_filter();

        let Some(picker) = &mut self.provider_picker else {
            return;
        };
        if let Some(id) = cursor_id {
            if let Some(row) = picker.rows.iter().position(|r| match r {
                PickerRow::Provider { index, .. } => {
                    picker.entries.get(*index).is_some_and(|e| e.id == id)
                }
                PickerRow::Model { .. } => false,
            }) {
                picker.selected = row;
            }
        }
    }

    /// What confirming the highlighted row means, if anything.
    ///
    /// `None` when the filter matched nothing — confirming an empty list is a
    /// no-op, not a guess about which provider was meant.
    #[must_use]
    pub fn selected_provider_pick(&self) -> Option<ProviderPick> {
        let picker = self.provider_picker.as_ref()?;
        match picker.rows.get(picker.selected)? {
            PickerRow::Provider { index, .. } => Some(ProviderPick::Provider(*index)),
            PickerRow::Model { model } => Some(ProviderPick::Model(model.id.clone())),
        }
    }

    /// Show an `AskUser` dialog. `session_key` is the clarification key the
    /// answer resolves against (`clarification.resolve`).
    ///
    /// The overlay opens in text mode whenever there is nothing to quick-pick,
    /// so the first keystroke on a free-text (or multi-select) question already
    /// goes where the user means it to.
    pub fn show_dialog(&mut self, session_key: String, view: AskDialogView) {
        let mut dialog = DialogState {
            session_key,
            question: view.question,
            options: view.options,
            selected: 0,
            multi_select: view.multi_select,
            secret: view.secret,
            input: String::new(),
            typing: false,
        };
        dialog.typing = !dialog.has_quick_pick();
        self.dialog = Some(dialog);
        self.focus = Focus::Dialog;
    }

    /// Surface the tool-approval overlay for a pending, session-owned approval
    /// and steal focus so the parked run gets a decision. The caller
    /// (`commands::poll_approvals`) has already confirmed the `id` belongs to
    /// this session.
    pub fn open_approval(
        &mut self,
        id: String,
        command: String,
        reason: Option<String>,
        decisions: Vec<(&'static str, &'static str)>,
    ) {
        self.approval = Some(ApprovalState {
            id,
            command,
            reason,
            selected: 0,
            decisions,
        });
        self.focus = Focus::Approval;
    }

    /// Show the side-question overlay for a `/btw` just sent, and give it
    /// focus.
    ///
    /// The purely local overlays are dismissed first — nothing on the server
    /// is parked on any of them. The two that ARE parked on something
    /// (`AskUser`, tool approval) are deliberately not touched, and cannot be
    /// showing anyway: both hold focus, so no `/btw` can have been typed
    /// while one was up.
    pub fn open_btw(&mut self, question: String) {
        self.palette = None;
        self.session_picker = None;
        self.provider_picker = None;
        self.btw.begin(question);
        self.focus = Focus::Btw;
    }

    /// Hide the side-question overlay and return focus to input. The history
    /// and the run claims survive — see [`BtwOverlay`]'s docs for why the
    /// claims must.
    pub fn close_btw(&mut self) {
        self.btw.close();
        self.focus = Focus::Input;
    }

    /// Retract the approval overlay (resolved here, resolved elsewhere, or the
    /// server-side approval expired) and return focus to input.
    pub fn close_approval(&mut self) {
        self.approval = None;
        self.focus = self.focus_after_modal();
    }

    /// Drop a showing approval overlay when its run ends by any path (complete,
    /// error, cancel, session-complete). No-op when none is showing, so focus is
    /// reset only in the case where the modal was actually up (and thus held
    /// focus). Needed because the pending-approval poll runs only while a run is
    /// active — once the run ends it can no longer retract a stale overlay.
    pub(crate) fn dismiss_pending_approval(&mut self) {
        if self.approval.is_some() {
            self.close_approval();
        }
    }

    /// Adopt the canonical session key the gateway reports on the `agent.run`
    /// result.
    ///
    /// The gateway is the only authority on what key a run was routed to: an
    /// explicit key that fails `SessionKey::parse` does not fail the call, it
    /// makes `AgentRouter::route` mint a fresh epoch instead. A client that
    /// keeps its own guess afterwards addresses a session that does not exist,
    /// and every keyed RPC it makes (`chat.history`, `session.usage`,
    /// `sessions.patch`, `session.compact`) answers about nothing.
    ///
    /// A no-op when the key is unchanged or empty, so the common path costs
    /// nothing and a server that omits the field cannot blank the key.
    pub fn adopt_canonical_session_key(&mut self, canonical: &str) {
        if canonical.is_empty() || canonical == self.session_key {
            return;
        }
        self.session_key = canonical.to_string();
        // The settings we hold describe the key we just replaced. Dropping them
        // makes the status bar say "unknown" until the next attach, which is
        // true; keeping them would make it confidently describe someone else's
        // conversation.
        self.session_snapshot = None;
    }

    /// Restore this conversation's durable settings from the server's snapshot.
    ///
    /// Called on attach (`chat.history`) and after a session switch. Everything
    /// here is read back, never invented: the cumulative token count comes from
    /// the `sessions` row rather than from this process's own tally, which is
    /// why reopening a terminal mid-task no longer restarts the counter at 0.
    pub fn apply_session_snapshot(&mut self, snapshot: SessionSnapshot) {
        self.session_key = snapshot.session_key.clone();
        self.total_tokens = u64::try_from(snapshot.total_tokens).unwrap_or(0);
        self.model_name = snapshot
            .effective_model()
            .map_or_else(|| self.default_model_name.clone(), str::to_string);
        self.session_snapshot = Some(snapshot);
    }

    /// The conversation's usage mode, exec tier, thinking depth and memory mode
    /// as the status bar renders them.
    ///
    /// `None` in the tuple means the session follows the global default — the
    /// renderer prints nothing rather than guessing which default is live,
    /// because the TUI does not read the server's config and a guess would be
    /// indistinguishable from a fact.
    #[must_use]
    pub fn session_knobs(&self) -> SessionKnobs<'_> {
        let snap = self.session_snapshot.as_ref();
        SessionKnobs {
            mode: snap.and_then(|s| s.mode.as_deref()),
            exec_tier: snap.and_then(|s| s.exec_tier.as_deref()),
            think_level: snap.and_then(|s| s.think_level.as_deref()),
            memory_mode: snap.and_then(|s| s.memory_mode.as_deref()),
        }
    }

    /// Locally record a knob the user just changed, so the status bar reflects
    /// it before the next attach re-reads it from the server.
    ///
    /// Only ever called after the write RPC returned `Ok` — an optimistic
    /// update on a refused write is exactly the "confident false answer" the
    /// snapshot exists to prevent. `field` selects which knob; the value is
    /// `None` for "follow global".
    pub fn record_local_knob(&mut self, field: SessionKnob, value: Option<String>) {
        let snap = self
            .session_snapshot
            .get_or_insert_with(|| SessionSnapshot {
                session_key: self.session_key.clone(),
                ..SessionSnapshot::default()
            });
        match field {
            SessionKnob::Mode => snap.mode = value,
            SessionKnob::ExecTier => snap.exec_tier = value,
            SessionKnob::ThinkLevel => snap.think_level = value,
            SessionKnob::MemoryMode => snap.memory_mode = value,
        }
    }

    /// Switch to a different session and reset transient chat/run UI state.
    /// The caller then appends the fetched `chat.history` transcript.
    pub fn switch_session(&mut self, session_key: &str) {
        self.session_key = session_key.to_string();
        // Per-conversation facts, and this component is a singleton: a counter
        // that survives the switch reports the previous conversation's spend
        // under the new one's name. The caller restores the real figures from
        // the incoming snapshot immediately after.
        self.total_tokens = 0;
        self.session_snapshot = None;
        self.model_name.clone_from(&self.default_model_name);
        self.messages.clear();
        // The run left behind (if any) still belongs to the OLD session, and
        // nothing extra needs to be recorded for that here: its own
        // `RunAccepted` already taught `run_sessions` which session it is
        // home to (see that field's doc), so `frame_belongs_here` now
        // answers "no" for it against the new `self.session_key` above — and
        // "yes" again if a later switch returns to its actual session.
        self.current_run = None;
        self.run_started_at = None;
        self.current_run_uses_agent_trace = false;
        self.current_run_trace_summary_applied = false;
        // New session = different context window; drop the stale gauge until
        // the next run's first `ContextGauge` refreshes it.
        self.context_gauge = None;
        // Same for the cache stat: the old session's hit% is meaningless for
        // a different prefix, and a cache-less provider would otherwise show
        // it indefinitely (the stat only updates on real cache activity).
        self.cache_stat = None;
        self.cache_stat_agent = None;
        // The next session's first reporting agent becomes its new root.
        self.cache_root_agent = None;
        self.dialog = None;
        self.palette = None;
        // The picker's rows describe the install, not the conversation, but the
        // pick it is mid-way through would land on the session we just left.
        self.provider_picker = None;
        // Any approval prompt belonged to the old session's run; drop it.
        self.approval = None;
        self.focus = Focus::Input;
        self.scroll_to_bottom();
        self.add_system_message(format!("Switched to session {session_key}"));
    }

    // -- Settings -------------------------------------------------------

    /// Toggle verbose/debug output mode.
    pub const fn toggle_verbose(&mut self) {
        self.verbose = !self.verbose;
    }

    /// Clear the chat screen (keep session state).
    pub fn clear_screen(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.add_system_message("Screen cleared.".to_string());
    }

    /// Update token usage from a `RunSummary`.
    pub const fn update_token_usage(&mut self, summary: &RunSummary) {
        self.total_tokens = self.total_tokens.saturating_add(summary.total_tokens);
    }
}
