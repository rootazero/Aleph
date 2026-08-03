//! Chat reactive state — signals for chat messages, streaming, and UI mode.

use super::plan::{PlanUpdate, PlanView};
use crate::api::teams::CoordTaskDto;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use shared_ui_logic::state::merge_recalled_draft;

/// File staged for upload as part of the next outbound message.
///
/// Lives on `ChatState` so both the composer's paperclip input AND the
/// chat-surface drop zone (cycle-2 G5) can append to the same list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
    pub size: u64,
}

/// A follow-up prompt the user lined up while a turn was still running.
///
/// Mirrors hermes-agent's `QueuedPromptEntry`. Reuses [`PendingAttachment`]
/// so a queued prompt carries the exact same payload as a live send — the
/// drain path just replays it through the normal composer pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedPrompt {
    pub text: String,
    pub attachments: Vec<PendingAttachment>,
}

/// One-line preview for a queued prompt: trimmed text (UTF-8-safe truncation,
/// P7), or an attachment-count fallback when attachments-only. Pure — the
/// ghost bubble renders whatever this returns.
#[must_use]
pub fn queue_preview_label(entry: &QueuedPrompt) -> String {
    const MAX: usize = 64;
    let text = entry.text.trim();
    if !text.is_empty() {
        let truncated: String = text.chars().take(MAX).collect();
        if truncated.chars().count() < text.chars().count() {
            format!("{truncated}…")
        } else {
            truncated
        }
    } else {
        let n = entry.attachments.len();
        format!("📎 {n}")
    }
}

/// Stable, machine-readable code for a chat send / delivery failure.
///
/// Mirrors openhuman's `chatSendError.ts` taxonomy so analytics and tests
/// can branch on a small fixed set instead of substring-matching messages.
/// New variants only — never rename or repurpose existing ones (wire-stable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSendErrorCode {
    /// WebSocket dropped or never established.
    SocketDisconnected,
    /// Cloud provider rejected the send (HTTP error, rate limit, etc.).
    CloudSendFailed,
    /// Server-side safety pipeline blocked the prompt.
    PromptBlocked,
    /// Server flagged the prompt for review (soft warning).
    PromptReview,
    /// Usage limit / quota reached.
    UsageLimitReached,
    /// Run aborted due to a safety timeout.
    SafetyTimeout,
    /// The composer refused the send before it left the client — the input is
    /// not supported on this surface (e.g. attachments in team group chat).
    /// Distinct from the server-side codes above: nothing was transmitted, and
    /// the user can fix it and retry immediately.
    Unsupported,
    /// Catch-all for unmapped errors. Use the message field for context.
    Unknown,
}

impl ChatSendErrorCode {
    /// CSS modifier class for the inline banner. Lives here so the UI
    /// layer can theme severity by code without a giant match table.
    #[must_use]
    pub const fn severity_class(self) -> &'static str {
        match self {
            // Soft warning — yellow accent
            Self::PromptReview => "warning",
            // Hard block — red accent (default for everything else too)
            _ => "danger",
        }
    }
}

/// Structured chat send error — preferred over the legacy bare
/// `error_message` string. Both are populated in lock-step so existing
/// readers keep working unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSendError {
    pub code: ChatSendErrorCode,
    pub message: String,
}

impl ChatSendError {
    pub fn new(code: ChatSendErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Heuristic classifier — maps an opaque error string to a code so the
    /// existing `ChatApi::send` error path can produce structured errors
    /// without a wire-format change. Order matters (most specific first).
    pub fn classify(msg: impl Into<String>) -> Self {
        let message = msg.into();
        let l = message.to_lowercase();
        let code =
            if l.contains("disconnect") || l.contains("not connected") || l.contains("websocket") {
                ChatSendErrorCode::SocketDisconnected
            } else if l.contains("prompt_blocked") || l.contains("prompt injection") {
                ChatSendErrorCode::PromptBlocked
            } else if l.contains("prompt_review") {
                ChatSendErrorCode::PromptReview
            } else if l.contains("usage limit") || l.contains("quota") || l.contains("rate limit") {
                ChatSendErrorCode::UsageLimitReached
            } else if l.contains("safety timeout")
                || l.contains("turn timeout")
                || l.contains("stalled after")
            {
                // Harness-side watchdogs (TerminateReason::TurnTimeout /
                // StallTimeout humanized text) — the run itself was killed.
                ChatSendErrorCode::SafetyTimeout
            } else if l.contains("timed out")
                || l.contains("cloud")
                || l.contains("http")
                || l.contains("provider")
            {
                // "Request timed out" comes from the provider transport
                // (connect/TTFB/stream-idle), not the harness — an upstream
                // delivery failure, so it belongs with CloudSendFailed.
                ChatSendErrorCode::CloudSendFailed
            } else {
                ChatSendErrorCode::Unknown
            };
        Self { code, message }
    }
}

/// Transient provider-retry status for the active run.
///
/// Set by `stream.run_retrying` (provider chain failed transiently, run-loop
/// is retrying), cleared as soon as the provider responds (first chunk) or
/// the run settles. Ephemeral — never part of [`SessionSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRetryNotice {
    /// Provider that just failed (e.g. "kimi-for-coding").
    pub provider: String,
    /// 1-based dispatch attempt about to run.
    pub attempt: u32,
    /// Total attempts before the run gives up.
    pub max_attempts: u32,
}

/// Context-window occupancy snapshot for the composer gauge. All three figures
/// are computed by core and shipped on the `run_complete` summary — the panel
/// is a pure renderer (R4): `used_tokens` = current occupancy
/// (`prompt_tokens_total` + last output), `window_tokens` = the model's
/// authoritative context window, `total_tokens` = the run's cumulative total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsage {
    /// Input/context tokens occupying the window after the latest turn.
    pub used_tokens: u32,
    /// Resolved context-window size for the run's model (gauge denominator).
    pub window_tokens: u32,
    /// Running total tokens billed for the run (shown in the tooltip).
    pub total_tokens: u64,
    /// True when these figures are a pre-run estimate (no real LLM turn yet),
    /// so the gauge renders `≈N%` instead of `N%`.
    pub is_estimate: bool,
}

/// Model resolution info (mirrors core `ModelInfo`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub is_fallback: bool,
    #[serde(default)]
    pub original_model: Option<String>,
}

/// What a completed run cost, projected from `run_complete`'s summary. Core
/// computes both the money and the token split (R4 — the panel only renders).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunCost {
    /// Estimated spend in USD. `None` when core could not price the run at all.
    #[serde(default)]
    pub usd: Option<f64>,
    /// Core's `cost_status`: "complete" | "partial_missing_price" | "unknown".
    /// Anything other than "complete" must be rendered as an approximation —
    /// presenting a partial estimate as exact is a lie about money.
    #[serde(default)]
    pub status: Option<String>,
    /// Run-cumulative token total.
    #[serde(default)]
    pub total_tokens: u64,
    /// Prompt/completion split from `token_breakdown`, for the hover title.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

impl RunCost {
    /// True when core priced the run in full. Anything else renders with a `≈`.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.status.as_deref() == Some("complete")
    }

    /// The meta-line money label, or `None` when the run carries no price.
    /// Sub-cent runs still get a figure (4 decimals) — "$0.00" reads as free.
    #[must_use]
    pub fn cost_label(&self) -> Option<String> {
        let usd = self.usd?;
        let sigil = if self.is_exact() { "" } else { "≈" };
        if usd >= 0.01 {
            Some(format!("{sigil}${usd:.2}"))
        } else {
            Some(format!("{sigil}${usd:.4}"))
        }
    }

    /// Compact token label ("12.3k tok"). `None` when core reported no tokens
    /// (a cached/aborted turn) — rendering "0 tok" reads as broken.
    #[must_use]
    pub fn tokens_label(&self) -> Option<String> {
        match self.total_tokens {
            0 => None,
            n if n < 1000 => Some(format!("{n} tok")),
            n => Some(format!("{:.1}k tok", n as f64 / 1000.0)),
        }
    }
}

/// A rendered chat message (user or assistant).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,    // "user" | "assistant"
    pub content: String, // final or accumulated text
    #[serde(default)]
    pub tool_calls: Vec<ToolCallEntry>,
    #[serde(default)]
    pub is_streaming: bool, // true while response_chunks arrive
    #[serde(default)]
    pub is_intermediate: bool, // true for intermediate progress messages
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub model_info: Option<ModelInfo>,
    /// Wall-clock creation time in epoch milliseconds. Stamped client-side on
    /// the message-creation path and hydrated from `chat.history` rows.
    /// `serde(default)` keeps older session snapshots (no field) loadable —
    /// they deserialize to `None` and simply render without a clock/separator.
    #[serde(default)]
    pub timestamp: Option<i64>,
    /// Think→Act iteration this bubble belongs to, stamped from
    /// `agent_trace.turn_started`. `None` for user messages, the pre-turn
    /// placeholder, and legacy/hydrated history rows. Drives left-chat
    /// segmentation and the `(run_id, iteration)` cross-highlight key.
    #[serde(default)]
    pub iteration: Option<usize>,
    /// The completed run's authoritative final answer (`run_complete`'s
    /// `summary.final_response`). When set this bubble renders as the
    /// conversational answer even if its terminating turn also issued a tool
    /// call — the tool card stays inline. Without it, a run whose last turn
    /// emitted text *and* a tool call (e.g. a closing `web_fetch`) would trap
    /// the answer in the step strip, since `is_final_answer` otherwise requires
    /// no tool calls. Mirrors the reference agents' typed-part model where text
    /// is always the answer and tools are always steps.
    #[serde(default)]
    pub is_final: bool,
    /// Set once `set_step_text` writes this bubble's authoritative per-turn text
    /// (`agent_trace.text_emitted`). Locks out late-arriving streamed
    /// `response_chunk` previews: the two travel independent async pipelines, so
    /// a chunk landing *after* the authoritative text would otherwise
    /// `push_str` a duplicate copy on top of it.
    #[serde(default)]
    pub text_finalized: bool,
    /// Team chat: the agent this bubble is attributed to. `None` = single-agent
    /// legacy path (zero regression; old snapshots without the field deserialize
    /// to None). `Some(..)` → MessageBubble renders attribution (color + name).
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Sunk archive of a finished/superseded scratchpad plan. `Some` ⇒ this
    /// message renders as a compact "completed task" capsule instead of normal
    /// text. Reconstructed identically by live projection and `replay_run`
    /// (both drive the same archive call sites), so it survives a tab swap (via
    /// `messages` in `SessionSnapshot`) and a full reload (via replay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_archive: Option<super::plan::PlanView>,
}

/// Terminal status for a tool row whose outcome never reached the panel: the
/// run settled while the row was still `running` and the authoritative
/// end-of-run `tool_summaries` did not name it either.
///
/// It exists because the `agent_trace` mirror is explicitly lossy —
/// `AgentTraceEmitSink` pushes through a bounded `mpsc(256)` with `try_send`
/// and drops on overflow. Without a fourth state a dropped
/// `tool_call_completed` left the row pulsing `running` forever (permanent 1s
/// tick subscription, and its `ExploreGroup` stuck on "Exploring…"). Settling
/// to `unknown` is honest: we know the run is over, we do not know how the
/// call ended. See [`ChatState::settle_orphan_tools`].
pub const TOOL_STATUS_UNKNOWN: &str = "unknown";

/// Minimal tool call record for display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub tool_id: String,
    pub tool_name: String,
    /// `"running" | "completed" | "failed" | "unknown"` — see
    /// [`TOOL_STATUS_UNKNOWN`] for the fourth state's rationale.
    pub status: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Epoch-ms when the tool first went "running" — drives the live
    /// elapsed timer on long-running tool rows. Stamped panel-side.
    #[serde(default)]
    pub started_at_ms: Option<i64>,
}

/// A tool row's render-relevant status triple: `(status, duration_ms,
/// started_at_ms)`.
pub type ToolStatusEntry = (String, Option<u64>, Option<i64>);

/// `tool_id → status triple` for every tool row in the transcript.
pub type ToolStatusMap = std::collections::HashMap<String, ToolStatusEntry>;

/// Look one tool row's status up by scanning the transcript. The single
/// implementation behind both [`ToolIndex`] and its no-context fallback.
#[must_use]
pub fn find_tool_status(messages: &[ChatMessage], tool_id: &str) -> Option<ToolStatusEntry> {
    messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|t| t.tool_id == tool_id)
        .map(|t| (t.status.clone(), t.duration_ms, t.started_at_ms))
}

/// Shared, memoized `tool_id → status` index over the foreground transcript.
///
/// Every mounted `ToolCard` (and the right-pane `ToolInspector`) needs one
/// tool's live status. Doing that as a per-card `Memo` that scans
/// `messages × tool_calls` meant a single streamed token — which invalidates
/// `messages` — re-ran an O(transcript) scan **once per mounted card**: a long
/// agentic run with dozens of tool rows paid O(cards × tool_calls) per token,
/// in WASM, at token rate.
///
/// One shared `Memo` collapses that to O(tool_calls) per token, and — because
/// `Memo` only notifies dependents when the computed value actually differs —
/// pure text streaming (the overwhelmingly common case: no tool row changed)
/// recomputes an *equal* map and wakes **no** card at all.
#[derive(Clone, Copy)]
pub struct ToolIndex(pub Memo<ToolStatusMap>);

impl ToolIndex {
    /// Build the index over `chat`'s transcript. Provided once at the app root.
    #[must_use]
    pub fn new(chat: ChatState) -> Self {
        Self(Memo::new(move |_| {
            chat.messages.with(|msgs| {
                msgs.iter()
                    .flat_map(|m| m.tool_calls.iter())
                    .map(|t| {
                        (
                            t.tool_id.clone(),
                            (t.status.clone(), t.duration_ms, t.started_at_ms),
                        )
                    })
                    .collect()
            })
        }))
    }

    /// Reactive lookup of one tool row's status.
    #[must_use]
    pub fn status_of(&self, tool_id: &str) -> Option<ToolStatusEntry> {
        self.0.with(|m| m.get(tool_id).cloned())
    }
}

/// One authoritative tool outcome from `run_complete`'s
/// `summary.tool_summaries[]`, projected onto the transcript by
/// [`ChatState::reconcile_tools`].
///
/// Core builds this from the harness's `tool_timeline` (`build_run_summary` in
/// `gateway/execution_engine/event_drain.rs`), keyed by the same `call.id` the
/// live `tool_call_started` / `tool_call_completed` trace events carry — so a
/// settlement always addresses the row the stream already created, and can
/// safely *create* the row when the `tool_call_started` mirror was the frame
/// that got dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSettlement {
    pub tool_id: String,
    pub tool_name: String,
    pub duration_ms: u64,
    pub success: bool,
}

/// Panel-side team member view (roster rail rendering + attribution coloring).
#[derive(Debug, Clone, PartialEq)]
pub struct TeamMemberView {
    pub agent_id: String,
    pub name: String,
    /// Agent emoji (avatar glyph); `None` falls back to a name monogram.
    pub emoji: Option<String>,
    pub role: String, // backend role, e.g. "leader" | "member" | "researcher"
    pub is_leader: bool,
    pub status: MemberStatus,
}

/// Live execution status of a team member (drives the roster rail status dot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemberStatus {
    #[default]
    Idle,
    Working,
    Done,
    Error,
}

/// Top-level Chat UI phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatPhase {
    #[default]
    Idle,
    Thinking,
    Streaming,
    Error,
}

/// Which sink trigger is archiving the active plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveGate {
    /// New `set_plan` / `clear` — archive only a worked-on plan (`has_activity`).
    Activity,
    /// Next-turn start — archive only an already-complete plan.
    Completed,
}

/// Reactive state container provided via Leptos context.
#[derive(Clone, Copy)]
pub struct ChatState {
    /// All messages in the current session.
    pub messages: RwSignal<Vec<ChatMessage>>,
    /// Current phase of the UI.
    pub phase: RwSignal<ChatPhase>,
    /// Active `run_id` (Some while agent is running).
    pub active_run_id: RwSignal<Option<String>>,
    /// Resolved session key from first chat.send response.
    pub session_key: RwSignal<Option<String>>,
    /// Currently selected agent ID for routing.
    pub agent_id: RwSignal<Option<String>>,
    /// Accumulated reasoning text for the current run.
    pub reasoning_text: RwSignal<String>,
    /// Error message (set when `run_error` arrives).
    ///
    /// Kept as a bare string for backward compatibility with sidebar /
    /// boot-gate readers; new UI code should read `send_error` for the
    /// structured form (preserves error code for severity styling and
    /// analytics).
    pub error_message: RwSignal<Option<String>>,
    /// Structured form of the last send / delivery error, kept in lock-step
    /// with `error_message`. Populated by `fail_run`, `set_send_error`,
    /// and cleared together with `error_message` on `clear*`.
    pub send_error: RwSignal<Option<ChatSendError>>,
    /// Files staged for the next outbound message. Composer paperclip and
    /// chat-surface drop zone both push into this list.
    pub pending_attachments: RwSignal<Vec<PendingAttachment>>,
    /// True while a drag is hovering the chat surface — drives the drop
    /// overlay highlight.
    pub is_dragging_files: RwSignal<bool>,
    /// What the user has typed but not yet sent. Owned here rather than by the
    /// composer so that (a) it is conversation-scoped like the queue and the
    /// attachment tray it belongs with — a tab swap used to keep the text while
    /// swapping the files out from under it — and (b) anything outside the
    /// composer (starter chips, the queued-ghost tap, keyboard recall) can put
    /// a prompt back in front of the user by writing it, with no prefill
    /// channel that could be produced with nothing draining it.
    /// Both composers bind their textarea straight to this signal.
    pub draft: RwSignal<String>,
    /// One-shot: an explicit Stop must suppress exactly one queue auto-drain,
    /// or cancelling a turn would flip busy → idle and immediately re-fire the
    /// queued prompt (the "Stop button does nothing" trap).
    ///
    /// Lives here rather than in the composer because the queue it gates is
    /// per-conversation while the composer is a single foreground component:
    /// pressing Stop in one conversation used to consume the suppression owed
    /// to whichever conversation the user switched to next.
    pub stop_suppresses_next_drain: RwSignal<bool>,
    /// Latest context-window occupancy, set on each `run_complete` and read by
    /// the composer's [`super::context_gauge::ContextGauge`]. `None` until the
    /// first run reports usage. Ephemeral (excluded from [`SessionSnapshot`]);
    /// it simply repopulates on the next completed turn after a tab swap.
    pub context_usage: RwSignal<Option<ContextUsage>>,
    /// Follow-up prompts queued while a run is active. Drained one-at-a-time
    /// when the turn settles naturally (see
    /// [`shared_ui_logic::state::should_auto_drain_on_settle`]). Session-scoped
    /// content, so it rides along in [`SessionSnapshot`] across tab swaps.
    pub prompt_queue: RwSignal<Vec<QueuedPrompt>>,
    /// Pulse signal that asks the composer to retry the last user message:
    /// each bump increments by 1. Used by `MessageBubble`'s retry button so
    /// the composer (which owns the send pipeline) actually fires the send
    /// without prop drilling a callback through every bubble.
    pub retry_pulse: RwSignal<u32>,
    /// One-shot pulse asking the composer to flush the prompt queue into the
    /// live run at a turn boundary (bumped by `events.rs` on
    /// `agent_trace.turn_started`). Ephemeral, like `retry_pulse` — excluded
    /// from [`SessionSnapshot`], so it neither snapshots nor needs clearing.
    pub flush_pulse: RwSignal<u32>,
    /// Pulse signal that asks the composer to run the doctor + LLM-repair
    /// flow (G1, `f` hotkey): each bump increments by 1. Bumped by the global
    /// keydown listener via [`HotkeyState`]; the composer seeds a diagnostic
    /// instruction and routes it through the normal send pipeline. Ephemeral,
    /// like `retry_pulse` — excluded from [`SessionSnapshot`].
    pub repair_pulse: RwSignal<u32>,
    /// Active project workspace root (absolute path). When `Some`, the
    /// chat composer attaches it to `chat.send` as `project_root`, and the
    /// daemon swaps the agent's working directory for the duration of the
    /// run. Switching project clears the session per the "switch opens new
    /// session" convention agreed for the desktop App.
    pub active_project_root: RwSignal<Option<String>>,
    /// Human-friendly display name for the active project. Surfaced in the
    /// composer's "enter project workspace ▾" chip so the user always sees which
    /// folder they're operating against.
    pub active_project_name: RwSignal<Option<String>>,
    /// User-selected per-turn model override (chat-window picker).
    ///
    /// `None` means "use the agent's configured default + fallback chain"
    /// — equivalent to openclaw's empty session model row. When `Some`, the
    /// composer attaches it to `chat.send.model_override`; the daemon
    /// short-circuits the resolver and pins the requested model.
    ///
    /// Selection is **session-sticky on the client**: it persists across
    /// turns within the panel session but resets on page reload (the
    /// server-side `preferred_model` persistence is a follow-up). Picker
    /// component owns reads/writes through this signal.
    pub selected_model: RwSignal<Option<crate::api::providers::ModelOverride>>,
    /// A model override requested from OUTSIDE the chat view (e.g. the MoA
    /// settings page's "Use in chat"). Parked here instead of written straight
    /// to `selected_model` because `restore_from` — which runs when the chat
    /// view activates a session on navigation — would immediately overwrite
    /// `selected_model` from the (empty) snapshot and lose it. Ephemeral: NOT in
    /// `SessionSnapshot`; `restore_from` consumes it right after the snapshot
    /// restore so the externally-requested model wins.
    pub pending_model_override: RwSignal<Option<crate::api::providers::ModelOverride>>,
    /// What each completed run cost, keyed by `run_id`. Projected from
    /// `run_complete`'s summary and read by the assistant bubble's meta line.
    /// Keyed on the run rather than stamped on [`ChatMessage`] because the same
    /// run's cost is looked up from whichever bubble ended up carrying its final
    /// answer. Session-scoped → rides along in [`SessionSnapshot`].
    pub run_costs: RwSignal<std::collections::HashMap<String, RunCost>>,
    /// Per-session execution tier override (`"ask"` | `"auto"` | `"full"`).
    /// `None` = follow the global tier. Mirrors what core persists under
    /// `SessionIdentityMeta.custom["exec_tier"]`; the composer's tier pill owns
    /// reads/writes. Session-scoped → rides along in [`SessionSnapshot`].
    pub session_exec_tier: RwSignal<Option<String>>,
    /// Per-session usage-mode override (`"chat"` | `"work"` | `"code"`).
    /// `None` = follow the global `[policies] mode`. Mirrors what core
    /// persists under `SessionIdentityMeta.custom["session_mode"]`; the
    /// composer's mode pill owns reads/writes. Session-scoped → rides along
    /// in [`SessionSnapshot`].
    pub session_mode: RwSignal<Option<String>>,
    /// The global `[policies] mode` default, mirrored from
    /// `config.get_tool_permissions` by the mode pill's fetch. Lets the
    /// right-rail mode dispatch (`events.rs`) resolve the EFFECTIVE mode for
    /// sessions that follow the global default — not just session-explicit
    /// overrides. Global, not session-scoped: survives `clear_session()` and
    /// stays out of [`SessionSnapshot`]. `None` = not yet fetched (older core
    /// or pre-connect) → dispatch falls back to pre-mode behavior.
    pub global_mode: RwSignal<Option<String>>,
    /// Run IDs whose final assistant reply should be spoken aloud — the
    /// voice-loop turns started from the composer mic button. `events.rs` pops
    /// each on `run_complete` and plays its TTS audio. Ephemeral, like
    /// `is_dragging_files` / `retry_pulse` (excluded from session snapshots).
    pub voice_run_ids: RwSignal<Vec<String>>,
    /// Provider-retry status shown under the thinking indicator while the
    /// run-loop retries a transiently failing provider chain. Ephemeral.
    pub provider_retry: RwSignal<Option<ProviderRetryNotice>>,
    /// Monotonic counter for generating unique user message IDs.
    next_msg_id: RwSignal<u64>,
    /// Team chat mode marker. `Some(team_id)` → render 3-pane team view; composer
    /// routes to teams.chat.send. `None` = single-agent chat (zero regression).
    /// NOTE (MVP): not persisted in SessionSnapshot — team mode is ephemeral and
    /// does not survive a session-tab swap; re-enter via the team compose button.
    pub team_id: RwSignal<Option<String>>,
    /// Team roster + live status (left roster rail data source). Empty = non-team.
    pub team_members: RwSignal<Vec<TeamMemberView>>,
    /// Team chat: coordination tasks for the active team (drives the bottom
    /// task strip + drawer). Empty when not in team mode. Fetched from
    /// `teams.list_tasks` and upserted by `team.<id>.task.<verb>` events.
    pub team_tasks: RwSignal<Vec<CoordTaskDto>>,
    /// Per explore-group expand override, keyed by group key. Absent = use the
    /// default (running groups open, completed collapsed); present = the
    /// user's explicit toggle. Lives here — not as a group-local signal —
    /// because `timeline::row_key` folds in content length, so the group row
    /// remounts on every streamed token and a component-local `open` would
    /// reset each time (re-opening a group the user just collapsed mid-run).
    /// Ephemeral, like `retry_pulse` — excluded from [`SessionSnapshot`].
    pub strip_open: RwSignal<std::collections::HashMap<String, bool>>,
    /// Active single-chat task plan (scratchpad-driven Todo widget). `None`
    /// hides the panel. Projected by `events.rs` via `scratchpad_plan_update`.
    pub plan: RwSignal<Option<PlanView>>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            messages: RwSignal::new(Vec::new()),
            phase: RwSignal::new(ChatPhase::Idle),
            active_run_id: RwSignal::new(None),
            session_key: RwSignal::new(None),
            agent_id: RwSignal::new(None),
            reasoning_text: RwSignal::new(String::new()),
            error_message: RwSignal::new(None),
            send_error: RwSignal::new(None),
            pending_attachments: RwSignal::new(Vec::new()),
            is_dragging_files: RwSignal::new(false),
            draft: RwSignal::new(String::new()),
            stop_suppresses_next_drain: RwSignal::new(false),
            context_usage: RwSignal::new(None),
            prompt_queue: RwSignal::new(Vec::new()),
            retry_pulse: RwSignal::new(0),
            flush_pulse: RwSignal::new(0),
            repair_pulse: RwSignal::new(0),
            active_project_root: RwSignal::new(None),
            active_project_name: RwSignal::new(None),
            selected_model: RwSignal::new(None),
            pending_model_override: RwSignal::new(None),
            run_costs: RwSignal::new(std::collections::HashMap::new()),
            session_exec_tier: RwSignal::new(None),
            session_mode: RwSignal::new(None),
            global_mode: RwSignal::new(None),
            voice_run_ids: RwSignal::new(Vec::new()),
            provider_retry: RwSignal::new(None),
            next_msg_id: RwSignal::new(0),
            team_id: RwSignal::new(None),
            team_members: RwSignal::new(Vec::new()),
            team_tasks: RwSignal::new(Vec::new()),
            strip_open: RwSignal::new(std::collections::HashMap::new()),
            plan: RwSignal::new(None),
        }
    }

    /// Whether explore group `key` is expanded. `default_open` is the state
    /// to use when the user hasn't toggled it (running groups default open,
    /// completed groups default collapsed).
    #[must_use]
    pub fn strip_is_open(&self, key: &str, default_open: bool) -> bool {
        self.strip_open
            .with(|m| m.get(key).copied())
            .unwrap_or(default_open)
    }

    /// Toggle explore group `key`'s expand state, seeding from
    /// `default_open` when the user hasn't toggled it before. The stored
    /// override survives the group row's per-token remount.
    pub fn toggle_strip(&self, key: &str, default_open: bool) {
        let next = !self.strip_is_open(key, default_open);
        self.strip_open.update(|m| {
            m.insert(key.to_string(), next);
        });
    }

    /// Apply a projected plan update to the Todo-panel signal.
    pub fn apply_plan_update(&self, update: PlanUpdate) {
        match update {
            PlanUpdate::Show(v) => self.plan.set(Some(v)),
            PlanUpdate::Hide => self.plan.set(None),
            PlanUpdate::NoChange => {}
        }
    }

    /// Settle the Todo strip at run end — the plan-shaped twin of
    /// [`Self::reconcile_tools`] + [`Self::settle_orphan_tools`].
    ///
    /// Two jobs, in order:
    /// 1. **Reconcile** against `summary.plan`, the authoritative terminal
    ///    snapshot the core latches in its own (unbounded, in-process) event
    ///    drain. The live projection rides the deliberately lossy `agent_trace`
    ///    mirror, so without this a single dropped `complete_item` frame left
    ///    the strip stuck at e.g. 2/5 forever with no repair path.
    /// 2. **Sink a finished plan immediately**, instead of waiting for the next
    ///    run to start. A completed list has nothing left to steer, and leaving
    ///    it mounted kept a pulsing "current step" row on screen across the
    ///    user's next message.
    ///
    /// An unfinished plan deliberately stays mounted: the objective is still
    /// open and the user should keep seeing what is left.
    pub fn settle_plan(&self, summary_plan: Option<&PlanView>) {
        self.apply_plan_update(super::plan::plan_settlement(summary_plan));
        self.archive_active_plan(ArchiveGate::Completed);
    }

    /// Which sink trigger is calling — decides the archive gate.
    pub fn archive_active_plan(&self, gate: ArchiveGate) {
        let Some(p) = self.plan.get_untracked() else {
            return;
        };
        let should = match gate {
            ArchiveGate::Activity => p.has_activity(),
            ArchiveGate::Completed => p.complete,
        };
        if !should {
            return; // leave the slot for the caller to overwrite/hide
        }
        let seq = self.next_msg_id.get_untracked();
        self.next_msg_id.set(seq + 1);
        self.messages.update(|msgs| {
            msgs.push(ChatMessage {
                id: format!("plan-archive-{seq}"),
                role: "assistant".into(),
                content: String::new(),
                tool_calls: vec![],
                is_streaming: false,
                is_intermediate: false,
                error: None,
                model_info: None,
                is_final: false,
                text_finalized: false,
                timestamp: Some(super::timeline::now_millis()),
                iteration: None,
                agent_id: None,
                plan_archive: Some(p),
            });
        });
        self.plan.set(None);
    }

    /// Record a provider-retry status (`stream.run_retrying`).
    pub fn set_provider_retry(&self, notice: ProviderRetryNotice) {
        self.provider_retry.set(Some(notice));
    }

    /// Clear the retry notice once the provider responds or the run settles.
    /// Cheap no-op when nothing is set (called from the per-chunk hot path).
    pub fn clear_provider_retry(&self) {
        if self
            .provider_retry
            .with_untracked(std::option::Option::is_some)
        {
            self.provider_retry.set(None);
        }
    }

    /// Append a follow-up prompt to the tail of the queue.
    pub fn enqueue_prompt(&self, entry: QueuedPrompt) {
        self.prompt_queue.update(|q| q.push(entry));
    }

    /// Remove the queued prompt at `index` (no-op if out of range). Used by
    /// the per-row ✕ in the queue preview bar.
    pub fn remove_queued_prompt(&self, index: usize) {
        self.prompt_queue.update(|q| {
            if index < q.len() {
                q.remove(index);
            }
        });
    }

    /// Take the queued prompt at `index` back out of the queue, if it exists.
    /// The caller is expected to hand it to [`Self::seed_draft`] — dropping the
    /// return value silently destroys a message the user queued.
    #[must_use]
    pub fn take_queued_prompt(&self, index: usize) -> Option<QueuedPrompt> {
        let mut taken = None;
        self.prompt_queue.update(|q| {
            if index < q.len() {
                taken = Some(q.remove(index));
            }
        });
        taken
    }

    /// Take the **newest** queued prompt back out of the queue — the keyboard
    /// recall (`ArrowUp` / `Alt+ArrowUp`) and its pointer equivalent.
    ///
    /// Recall pops from the tail so that repeated recalls, each prepended to
    /// the draft by [`Self::seed_draft`], rebuild the queue's original order in
    /// the composer.
    #[must_use]
    pub fn recall_latest_queued(&self) -> Option<QueuedPrompt> {
        let mut popped = None;
        self.prompt_queue.update(|q| popped = q.pop());
        popped
    }

    /// Put a prompt back in front of the user, **ahead of** whatever they are
    /// already typing.
    ///
    /// The single entry point for "restore this into the composer": starter
    /// chips, the queued-ghost tap, and keyboard recall all call it. It writes
    /// [`Self::draft`] directly — there is no prefill channel that could be
    /// written with nothing draining it — and merges rather than overwrites, so
    /// neither an in-progress draft nor a second recall arriving in the same
    /// frame can be lost.
    pub fn seed_draft(&self, text: String, attachments: Vec<PendingAttachment>) {
        if !text.trim().is_empty() {
            self.draft
                .update(|draft| *draft = merge_recalled_draft(&text, draft));
        }
        if !attachments.is_empty() {
            self.pending_attachments.update(|staged| {
                let mut merged = attachments;
                merged.append(staged);
                *staged = merged;
            });
        }
    }

    /// Remove and return every queued prompt (FIFO order preserved), leaving
    /// the queue empty. Flushes the whole batch in one shot — at a turn
    /// boundary (Steer) or on the busy→idle settle.
    ///
    /// The caller now owns those prompts: a flush that fails part-way must put
    /// the remainder back with [`Self::requeue_front`] rather than drop them.
    #[must_use]
    pub fn drain_all_queued(&self) -> Vec<QueuedPrompt> {
        let mut out = Vec::new();
        self.prompt_queue.update(|q| out = std::mem::take(q));
        out
    }

    /// Put drained prompts back at the head of the queue, ahead of anything
    /// enqueued while the flush was in flight, so their arrival order is
    /// preserved. Used when a send never left the client.
    pub fn requeue_front(&self, prompts: Vec<QueuedPrompt>) {
        if prompts.is_empty() {
            return;
        }
        self.prompt_queue.update(|q| {
            let mut restored = prompts;
            restored.append(q);
            *q = restored;
        });
    }

    /// Register a run whose final assistant reply should be spoken aloud.
    /// Called by the composer mic button after it sends a voice-loop turn.
    pub fn mark_speak_run(&self, run_id: &str) {
        let id = run_id.to_string();
        self.voice_run_ids.update(|ids| {
            if !ids.contains(&id) {
                ids.push(id);
            }
        });
    }

    /// Pop a run from the speak set; returns `true` if it was registered.
    #[must_use]
    pub fn take_speak_run(&self, run_id: &str) -> bool {
        let mut found = false;
        self.voice_run_ids.update(|ids| {
            if let Some(pos) = ids.iter().position(|r| r == run_id) {
                ids.remove(pos);
                found = true;
            }
        });
        found
    }

    /// Final accumulated text of the assistant message for `run_id`, if present.
    #[must_use]
    pub fn assistant_text_for_run(&self, run_id: &str) -> String {
        let target_id = format!("assistant-{run_id}");
        self.messages.with(|msgs| {
            msgs.iter()
                .rev()
                .find(|m| m.id == target_id)
                .map(|m| m.content.clone())
                .unwrap_or_default()
        })
    }

    /// Set the active project and reset the session (1:1 project↔session
    /// binding per the agreed UX model). Passing `None` exits project mode
    /// and the chat reverts to running inside `~/.aleph/workspaces/{agent_id}`.
    pub fn set_active_project(&self, root: Option<String>, name: Option<String>) {
        let switching = self.active_project_root.get_untracked() != root;
        self.active_project_root.set(root);
        self.active_project_name.set(name);
        if switching {
            self.clear_session();
        }
    }

    /// Append a user message and reset error state.
    pub fn push_user_message(&self, text: &str) {
        let seq = self.next_msg_id.get_untracked();
        self.next_msg_id.set(seq + 1);
        let id = format!("user-{seq}");
        self.messages.update(|msgs| {
            msgs.push(ChatMessage {
                id,
                role: "user".into(),
                content: text.to_string(),
                tool_calls: vec![],
                is_streaming: false,
                is_intermediate: false,
                error: None,
                model_info: None,
                is_final: false,
                text_finalized: false,
                timestamp: Some(super::timeline::now_millis()),
                iteration: None,
                agent_id: None,
                plan_archive: None,
            });
        });
        self.error_message.set(None);
        // The banner describes the LAST send attempt, so a new one retires it.
        // Without this a corrective send (remove the attachment, drop the
        // flagged phrasing) left the old red banner pinned above the transcript
        // until the session was cleared, reading as if the fix had failed.
        self.send_error.set(None);
    }

    /// Start a new assistant message placeholder (streaming).
    pub fn start_assistant_message(&self, run_id: &str) {
        // Next-turn sink: a finished plan retires into the conversation flow
        // when the next run begins. Both live (`run_accepted`) and replay
        // (`replay_run`) call this, so the capsule reconstructs identically.
        self.archive_active_plan(ArchiveGate::Completed);
        let id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            msgs.push(ChatMessage {
                id,
                role: "assistant".into(),
                content: String::new(),
                tool_calls: vec![],
                is_streaming: true,
                is_intermediate: false,
                error: None,
                model_info: None,
                is_final: false,
                text_finalized: false,
                timestamp: Some(super::timeline::now_millis()),
                // Stamp the placeholder as step 1 immediately so the timeline
                // folds it into the step strip from the first frame, instead of
                // briefly rendering it as a bare reply bubble before the first
                // `turn_started` arrives.
                iteration: Some(1),
                agent_id: None,
                plan_archive: None,
            });
        });
        self.active_run_id.set(Some(run_id.to_string()));
        self.phase.set(ChatPhase::Thinking);
        self.reasoning_text.set(String::new());
    }

    /// Begin a new agent step (Think→Act iteration) for `run_id`.
    ///
    /// Driven by `agent_trace.turn_started`. If the current
    /// `assistant-{run_id}` bubble already carries text or tool calls and
    /// belongs to a different iteration, it is finalized as a standalone
    /// intermediate step and a fresh streaming bubble tagged with `iteration`
    /// is started. Otherwise the trailing placeholder is reused — its iteration
    /// is simply updated to the incoming one. This covers the first turn (the
    /// placeholder is pre-stamped as step 1 in `start_assistant_message`) and
    /// the race where `response_chunk` preview text lands before `turn_started`
    /// (the two travel independent async pipelines — AgentTraceEmitSink spawns
    /// a drain task), so the preview belongs to THIS step and must not be
    /// orphaned into a duplicate intermediate bubble.
    pub fn begin_step(&self, run_id: &str, iteration: usize) {
        let target_id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            let len = msgs.len();
            if let Some(idx) = msgs.iter().rposition(|m| m.id == target_id) {
                let has_payload = !msgs[idx].content.is_empty() || !msgs[idx].tool_calls.is_empty();
                // Only finalize the bubble as a completed intermediate step when
                // it already belongs to a *different, already-stamped* iteration.
                // When the iteration matches (first turn or a raced preview), the
                // placeholder is reused so the content stays in the step strip.
                let prior_step = msgs[idx].iteration.is_some_and(|prev| prev != iteration);
                if has_payload && prior_step {
                    msgs[idx].is_streaming = false;
                    msgs[idx].is_intermediate = true;
                    msgs[idx].id = format!("intermediate-{run_id}-{len}");
                    msgs.push(ChatMessage {
                        id: target_id,
                        role: "assistant".into(),
                        content: String::new(),
                        tool_calls: vec![],
                        is_streaming: true,
                        is_intermediate: false,
                        error: None,
                        model_info: None,
                        iteration: Some(iteration),
                        timestamp: Some(super::timeline::now_millis()),
                        is_final: false,
                        text_finalized: false,
                        agent_id: None,
                        plan_archive: None,
                    });
                } else {
                    msgs[idx].iteration = Some(iteration);
                }
            }
        });
        self.phase.set(ChatPhase::Thinking);
    }

    /// Set the authoritative text for the bubble of `run_id` at `iteration`,
    /// overwriting any streamed typewriter preview. Targets the bubble
    /// carrying the matching iteration tag — the live `assistant-{run_id}`
    /// bubble or an already-finalized `intermediate-{run_id}-{n}` bubble for
    /// this run — so late `text_emitted` events still land correctly.
    pub fn set_step_text(&self, run_id: &str, iteration: usize, text: &str) {
        let assistant_id = format!("assistant-{run_id}");
        let intermediate_prefix = format!("intermediate-{run_id}-");
        self.messages.update(|msgs| {
            if let Some(m) = msgs.iter_mut().rev().find(|m| {
                m.iteration == Some(iteration)
                    && (m.id == assistant_id || m.id.starts_with(&intermediate_prefix))
            }) {
                m.content = text.to_string();
                // Authoritative per-turn text has landed: lock this bubble so a
                // late streamed `response_chunk` preview can't `push_str` a
                // duplicate copy on top of it (the two arrive on independent
                // async pipelines and can race).
                m.text_finalized = true;
            }
        });
    }

    /// Set model info on the current assistant message for the given run.
    pub fn set_model_info(&self, run_id: &str, info: ModelInfo) {
        let target_id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.model_info = Some(info);
            }
        });
    }

    /// Resolved model id for `run_id`, read from the assistant bubble's
    /// `model_info`. Used by the context gauge to pick a window size.
    #[must_use]
    pub fn model_for_run(&self, run_id: &str) -> Option<String> {
        let target_id = format!("assistant-{run_id}");
        self.messages.with(|msgs| {
            msgs.iter()
                .rev()
                .find(|m| m.id == target_id)
                .and_then(|m| m.model_info.as_ref())
                .map(|info| info.model.clone())
        })
    }

    /// Append a response text chunk to the current assistant message.
    pub fn append_chunk(&self, run_id: &str, content: &str) {
        // Provider produced output — any pending retry notice is stale.
        self.clear_provider_retry();
        let target_id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                // Skip once `set_step_text` has written the authoritative text
                // for this bubble — a late preview chunk would otherwise double
                // the content (see `text_finalized`).
                if !msg.text_finalized {
                    msg.content.push_str(content);
                }
            }
        });
        self.phase.set(ChatPhase::Streaming);
    }

    /// Record a tool call event.
    ///
    /// The update half searches **every bubble of the run** (the trailing
    /// `assistant-{run}` one *and* the already-finalized
    /// `intermediate-{run}-{n}` ones), mirroring what `set_step_text` and
    /// `finalize_answer` already do. Searching only the trailing bubble made a
    /// late `tool_call_completed` — one that lands after the next
    /// `turn_started` renamed its bubble — miss its own row and *append a
    /// phantom duplicate* to the fresh step, leaving the original row pinned
    /// on `running` forever.
    ///
    /// A first sighting still appends to the trailing bubble, so a tool call
    /// always joins the step it was issued from.
    pub fn update_tool(
        &self,
        run_id: &str,
        tool_id: &str,
        tool_name: &str,
        status: &str,
        duration_ms: Option<u64>,
    ) {
        let target_id = format!("assistant-{run_id}");
        let intermediate_prefix = format!("intermediate-{run_id}-");
        self.messages.update(|msgs| {
            let existing = msgs
                .iter_mut()
                .rev()
                .filter(|m| m.id == target_id || m.id.starts_with(&intermediate_prefix))
                .find_map(|m| m.tool_calls.iter_mut().find(|t| t.tool_id == tool_id));
            if let Some(tc) = existing {
                tc.status = status.to_string();
                tc.duration_ms = duration_ms;
                return;
            }
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.tool_calls.push(ToolCallEntry {
                    tool_id: tool_id.to_string(),
                    tool_name: tool_name.to_string(),
                    status: status.to_string(),
                    duration_ms,
                    started_at_ms: (status == "running").then(super::timeline::now_millis),
                });
            }
        });
    }

    /// Project the run's authoritative end-of-run tool outcomes
    /// (`run_complete` → `summary.tool_summaries[]`) onto its rows.
    ///
    /// This is the repair pass for the deliberately lossy `agent_trace` mirror
    /// (see [`TOOL_STATUS_UNKNOWN`]): whichever `tool_call_started` /
    /// `tool_call_completed` frames were dropped, the terminal truth for every
    /// call of the run rides along in the very same `run_complete` frame the
    /// panel already parses for cost and occupancy. Core is authoritative and
    /// the panel is a pure renderer (R4), so a settlement always wins over the
    /// streamed status.
    ///
    /// A settlement whose row does not exist creates it on the trailing bubble
    /// — the case where the *start* frame was the one that got dropped, so the
    /// call was never visible at all.
    pub fn reconcile_tools(&self, run_id: &str, settlements: &[ToolSettlement]) {
        if settlements.is_empty() {
            return;
        }
        let target_id = format!("assistant-{run_id}");
        let intermediate_prefix = format!("intermediate-{run_id}-");
        self.messages.update(|msgs| {
            for s in settlements {
                let status = if s.success { "completed" } else { "failed" };
                let existing = msgs
                    .iter_mut()
                    .rev()
                    .filter(|m| m.id == target_id || m.id.starts_with(&intermediate_prefix))
                    .find_map(|m| m.tool_calls.iter_mut().find(|t| t.tool_id == s.tool_id));
                if let Some(tc) = existing {
                    tc.status = status.to_string();
                    tc.duration_ms = Some(s.duration_ms);
                    if tc.tool_name.is_empty() {
                        tc.tool_name.clone_from(&s.tool_name);
                    }
                } else if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                    msg.tool_calls.push(ToolCallEntry {
                        tool_id: s.tool_id.clone(),
                        tool_name: s.tool_name.clone(),
                        status: status.to_string(),
                        duration_ms: Some(s.duration_ms),
                        started_at_ms: None,
                    });
                }
            }
        });
    }

    /// Settle every row of `run_id` still marked `running` to
    /// [`TOOL_STATUS_UNKNOWN`].
    ///
    /// Runs after [`Self::reconcile_tools`] on `run_complete`, and alone on
    /// `run_error` (whose frame carries no summary at all). Without it a row
    /// whose completion frame was dropped keeps a pulsing dot and an
    /// ever-growing elapsed timer — a permanent 1s-tick subscription — and its
    /// `ExploreGroup` never reaches `completed`, so the block stays expanded
    /// under an "Exploring…" header for the rest of the session.
    pub fn settle_orphan_tools(&self, run_id: &str) {
        let target_id = format!("assistant-{run_id}");
        let intermediate_prefix = format!("intermediate-{run_id}-");
        self.messages.update(|msgs| {
            for m in msgs
                .iter_mut()
                .filter(|m| m.id == target_id || m.id.starts_with(&intermediate_prefix))
            {
                for tc in m.tool_calls.iter_mut().filter(|t| t.status == "running") {
                    tc.status = TOOL_STATUS_UNKNOWN.to_string();
                }
            }
        });
    }

    /// Record what a completed run cost (projected from `run_complete`).
    pub fn set_run_cost(&self, run_id: &str, cost: RunCost) {
        self.run_costs.update(|m| {
            m.insert(run_id.to_string(), cost);
        });
    }

    /// Finalize current run (mark message as not streaming).
    pub fn complete_run(&self, run_id: &str) {
        let target_id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.is_streaming = false;
            }
        });
        self.active_run_id.set(None);
        self.phase.set(ChatPhase::Idle);
        self.clear_provider_retry();
    }

    /// Promote a completed run's authoritative final answer into its trailing
    /// assistant bubble. Driven by `run_complete` from `summary.final_response`
    /// (and mirrored on the `replay_run` history path). Overwrites the trailing
    /// bubble with the authoritative text and flags it `is_final`, so the answer
    /// renders as a conversational bubble even when its terminating turn also
    /// issued a tool call — without this, such an answer stays trapped in the
    /// step strip. No-op for an empty answer or a run with no assistant bubble.
    pub fn finalize_answer(&self, run_id: &str, final_text: &str) {
        if final_text.trim().is_empty() {
            return;
        }
        let assistant_id = format!("assistant-{run_id}");
        let intermediate_prefix = format!("intermediate-{run_id}-");
        self.messages.update(|msgs| {
            if let Some(m) = msgs.iter_mut().rev().find(|m| {
                m.role == "assistant"
                    && (m.id == assistant_id || m.id.starts_with(&intermediate_prefix))
            }) {
                m.content = final_text.to_string();
                m.is_final = true;
                m.is_intermediate = false;
                m.is_streaming = false;
                m.text_finalized = true;
            }
        });
    }

    /// Mark current run as errored.
    pub fn fail_run(&self, run_id: &str, error: &str) {
        let target_id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.is_streaming = false;
                msg.error = Some(error.to_string());
            }
        });
        // Only the run that failed gets torn down. A conversation can have a
        // second run_id outstanding — one the Panel queued and the gateway is
        // still holding in its wait lane — and a `RunError` for *that* one used
        // to clear `active_run_id` and flip the phase to Error while the live
        // run was still streaming: the transcript kept filling in but the
        // composer showed Stop gone, an error banner up, and the queue's settle
        // edge already spent.
        if self
            .active_run_id
            .with_untracked(|live| live.as_deref() == Some(run_id))
        {
            self.active_run_id.set(None);
            self.phase.set(ChatPhase::Error);
            self.clear_provider_retry();
        }
        let structured = ChatSendError::classify(error);
        self.error_message.set(Some(structured.message.clone()));
        self.send_error.set(Some(structured));
    }

    /// Record a structured chat send error from the composer / outbound
    /// path (e.g. `ChatApi::send` rejection, prompt-injection gate). Keeps
    /// the legacy `error_message` field in sync.
    pub fn set_send_error(&self, err: ChatSendError) {
        self.error_message.set(Some(err.message.clone()));
        self.send_error.set(Some(err));
        self.phase.set(ChatPhase::Error);
    }

    /// Ask the composer to retry the last user message. Bumps the pulse so
    /// downstream Effects see a change even when content is identical.
    pub fn request_retry(&self) {
        self.retry_pulse.update(|n| *n = n.wrapping_add(1));
    }

    /// Ask the composer to run the doctor + LLM-repair flow (G1 `f` hotkey).
    /// Bumps the pulse so the composer's Effect seeds the diagnostic prompt
    /// and fires it through the normal send pipeline.
    pub fn request_repair(&self) {
        self.repair_pulse.update(|n| *n = n.wrapping_add(1));
    }

    /// Return the content of the most recent user message, if any. Used by
    /// the retry path to repopulate the composer.
    #[must_use]
    pub fn last_user_text(&self) -> Option<String> {
        self.messages.with(|msgs| {
            msgs.iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.content.clone())
        })
    }

    /// Clear all messages and reset state.
    pub fn clear(&self) {
        self.messages.set(Vec::new());
        self.phase.set(ChatPhase::Idle);
        self.active_run_id.set(None);
        self.session_key.set(None);
        self.agent_id.set(None);
        self.reasoning_text.set(String::new());
        self.error_message.set(None);
        self.send_error.set(None);
        self.prompt_queue.set(Vec::new());
        self.draft.set(String::new());
        self.stop_suppresses_next_drain.set(false);
        self.strip_open.set(std::collections::HashMap::new());
        self.plan.set(None);
        self.context_usage.set(None);
        self.run_costs.set(std::collections::HashMap::new());
        // A fresh conversation carries no session tier — it follows the global
        // one until the user picks otherwise.
        self.session_exec_tier.set(None);
        self.session_mode.set(None);
    }

    /// Reset only the per-session state that [`SessionSnapshot`] does *not*
    /// carry.
    ///
    /// Opening another session in the same tab goes through
    /// `SessionMap::activate` first, which snapshots the outgoing conversation
    /// and restores the incoming one — every signal listed in
    /// `SessionSnapshot` is already correct by then. Calling the full
    /// [`Self::clear_session`] afterwards threw all of it away again: the
    /// draft and the prompt queue vanished (so a recalled or queued message
    /// was lost for good on one sidebar click), and `active_run_id` was
    /// nulled, which is the signal that tells the composer the conversation is
    /// still running — the Stop button disappeared, new input stopped queueing,
    /// and the next Enter started a second concurrent run on the same session.
    ///
    /// Team state is the part no snapshot holds, so it is the part that still
    /// has to be reset by hand — otherwise leaving group A for group B shows
    /// A's roster and tasks under B's name.
    pub fn clear_team_context(&self) {
        self.team_id.set(None);
        self.team_members.set(Vec::new());
        self.team_tasks.set(Vec::new());
        self.strip_open.set(std::collections::HashMap::new());
    }

    /// Clear session state but keep `agent_id` (for new chat within same agent).
    pub fn clear_session(&self) {
        self.messages.set(Vec::new());
        self.phase.set(ChatPhase::Idle);
        self.active_run_id.set(None);
        self.session_key.set(None);
        self.reasoning_text.set(String::new());
        self.error_message.set(None);
        self.send_error.set(None);
        self.prompt_queue.set(Vec::new());
        self.draft.set(String::new());
        self.stop_suppresses_next_drain.set(false);
        self.team_id.set(None);
        self.team_members.set(Vec::new());
        // Clear the task strip too. Leaving it behind meant switching from
        // group A to group B showed A's tasks under B's name until the new
        // team's `teams.list_tasks` came back — and, worse, leaving a group for
        // a single chat kept a phantom strip pinned above the composer.
        self.team_tasks.set(Vec::new());
        self.strip_open.set(std::collections::HashMap::new());
        self.plan.set(None);
        self.context_usage.set(None);
        self.run_costs.set(std::collections::HashMap::new());
        self.session_exec_tier.set(None);
        self.session_mode.set(None);
        // agent_id is intentionally preserved
    }

    /// Capture a serializable, owned copy of all per-session signals.
    /// Used by `SessionMap` to swap tabs without remounting the chat tree:
    /// the outgoing tab's state is snapshotted here, the incoming tab's
    /// snapshot is then restored via [`Self::restore_from`].
    ///
    /// `is_dragging_files` and `retry_pulse` are intentionally excluded —
    /// the former is ephemeral DOM-hover state, the latter a one-shot
    /// pulse that shouldn't replay on restore.
    #[must_use]
    pub fn capture_snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            messages: self.messages.get_untracked(),
            phase: self.phase.get_untracked(),
            active_run_id: self.active_run_id.get_untracked(),
            session_key: self.session_key.get_untracked(),
            agent_id: self.agent_id.get_untracked(),
            reasoning_text: self.reasoning_text.get_untracked(),
            error_message: self.error_message.get_untracked(),
            send_error: self.send_error.get_untracked(),
            pending_attachments: self.pending_attachments.get_untracked(),
            prompt_queue: self.prompt_queue.get_untracked(),
            draft: self.draft.get_untracked(),
            stop_suppresses_next_drain: self.stop_suppresses_next_drain.get_untracked(),
            active_project_root: self.active_project_root.get_untracked(),
            active_project_name: self.active_project_name.get_untracked(),
            selected_model: self.selected_model.get_untracked(),
            next_msg_id: self.next_msg_id.get_untracked(),
            context_usage: self.context_usage.get_untracked(),
            run_costs: self.run_costs.get_untracked(),
            session_exec_tier: self.session_exec_tier.get_untracked(),
            session_mode: self.session_mode.get_untracked(),
            plan: self.plan.get_untracked(),
        }
    }

    /// Restore a previously captured snapshot. Always sets every field so
    /// stale values from the outgoing tab never leak across.
    pub fn restore_from(&self, snap: SessionSnapshot) {
        self.messages.set(snap.messages);
        self.phase.set(snap.phase);
        self.active_run_id.set(snap.active_run_id);
        self.session_key.set(snap.session_key);
        self.agent_id.set(snap.agent_id);
        self.reasoning_text.set(snap.reasoning_text);
        self.error_message.set(snap.error_message);
        self.send_error.set(snap.send_error);
        self.pending_attachments.set(snap.pending_attachments);
        self.prompt_queue.set(snap.prompt_queue);
        self.draft.set(snap.draft);
        self.stop_suppresses_next_drain
            .set(snap.stop_suppresses_next_drain);
        self.active_project_root.set(snap.active_project_root);
        self.active_project_name.set(snap.active_project_name);
        self.selected_model.set(snap.selected_model);
        // A model requested from outside the chat view (MoA "Use in chat")
        // survives this restore: apply it over the snapshot value, then consume.
        if let Some(mo) = self.pending_model_override.get_untracked() {
            self.selected_model.set(Some(mo));
            self.pending_model_override.set(None);
        }
        self.run_costs.set(snap.run_costs);
        self.session_exec_tier.set(snap.session_exec_tier);
        self.session_mode.set(snap.session_mode);
        self.next_msg_id.set(snap.next_msg_id);
        // Carried in the snapshot so the occupancy gauge survives a tab swap
        // (None for a fresh/empty tab, which correctly hides the gauge).
        self.context_usage.set(snap.context_usage);
        // The execution list belongs to the conversation, so it rides the
        // snapshot: it used to be blanked here, which meant switching tabs and
        // back — even mid-run — silently destroyed the Todo strip until the
        // model happened to touch the scratchpad again.
        self.plan.set(snap.plan);
        // Collapse choices stay ephemeral: they are view state about rows that
        // may not even exist in the restored transcript.
        self.strip_open.set(std::collections::HashMap::new());
        // Team mode is ephemeral (not in the snapshot, re-entered via compose) —
        // reset so the outgoing tab's team view/routing never leaks into the
        // restored single-agent session across a tab swap or close.
        self.team_id.set(None);
        self.team_members.set(Vec::new());
        self.team_tasks.set(Vec::new());
    }
}

/// Owned snapshot of every per-session signal in [`ChatState`]. Cheap to
/// stash in a `HashMap` (every field is `Clone`).
#[derive(Debug, Clone, Default)]
pub struct SessionSnapshot {
    pub messages: Vec<ChatMessage>,
    pub phase: ChatPhase,
    pub active_run_id: Option<String>,
    pub session_key: Option<String>,
    pub agent_id: Option<String>,
    pub reasoning_text: String,
    pub error_message: Option<String>,
    pub send_error: Option<ChatSendError>,
    pub pending_attachments: Vec<PendingAttachment>,
    pub prompt_queue: Vec<QueuedPrompt>,
    /// The unsent composer text, so a tab swap carries it with the queue and
    /// the attachment tray instead of stranding it on the wrong conversation.
    pub draft: String,
    /// Whether an explicit Stop is still owed one suppressed auto-drain.
    pub stop_suppresses_next_drain: bool,
    pub active_project_root: Option<String>,
    pub active_project_name: Option<String>,
    pub selected_model: Option<crate::api::providers::ModelOverride>,
    pub next_msg_id: u64,
    /// Last completed turn's context-window occupancy, so the gauge survives a
    /// tab swap instead of blanking until the next turn finishes.
    pub context_usage: Option<ContextUsage>,
    /// Per-run cost, so the meta line survives a tab swap.
    pub run_costs: std::collections::HashMap<String, RunCost>,
    /// This session's execution-tier override (`None` = follow the global tier).
    pub session_exec_tier: Option<String>,
    /// Per-session usage-mode override (mode pill) — same carrier contract as
    /// `session_exec_tier`.
    pub session_mode: Option<String>,
    /// The session's live execution list, so the Todo strip survives a tab
    /// swap the same way the context gauge and per-run costs do.
    pub plan: Option<PlanView>,
}

#[cfg(test)]
mod step_tests {
    use super::*;

    fn assistant_ids(chat: &ChatState) -> Vec<(String, Option<usize>, bool, bool)> {
        chat.messages.with(|m| {
            m.iter()
                .map(|x| (x.id.clone(), x.iteration, x.is_streaming, x.is_intermediate))
                .collect()
        })
    }

    #[test]
    fn begin_step_reuses_empty_placeholder() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);

        let rows = assistant_ids(&chat);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "assistant-r1");
        assert_eq!(rows[0].1, Some(1));
        assert!(rows[0].2, "reused placeholder still streaming");
    }

    #[test]
    fn assistant_placeholder_is_pre_stamped_as_step_one() {
        // The placeholder must fold into the step strip from the first frame,
        // so the UI never renders a bare reply bubble before the first turn
        // starts.
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");

        let rows = assistant_ids(&chat);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "assistant-r1");
        assert_eq!(rows[0].1, Some(1), "placeholder pre-stamped as step 1");
        assert!(rows[0].2, "placeholder streaming");
        assert!(!rows[0].3, "placeholder not intermediate");
    }

    #[test]
    fn begin_step_finalizes_nonempty_and_opens_new() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "step one");
        chat.begin_step("r1", 2);

        let rows = assistant_ids(&chat);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "intermediate-r1-1");
        assert_eq!(rows[0].1, Some(1));
        assert!(!rows[0].2 && rows[0].3, "finalized + intermediate");
        assert_eq!(rows[1].0, "assistant-r1");
        assert_eq!(rows[1].1, Some(2));
        assert!(rows[1].2 && !rows[1].3);
    }

    #[test]
    fn begin_step_reuses_placeholder_with_raced_preview() {
        // Regression (double-render): `response_chunk` deltas and
        // `agent_trace.turn_started` travel independent async pipelines
        // (AgentTraceEmitSink spawns a drain task), so streamed preview text can
        // land in the placeholder bubble BEFORE the first `turn_started`. The
        // late `begin_step` must REUSE the pre-stamped placeholder — its
        // content is THIS step's preview — not orphan it into a duplicate
        // `intermediate-` bubble that `set_step_text` then mirrors.
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.append_chunk("r1", "你好"); // deltas win the cross-pipeline race
        chat.begin_step("r1", 1); // turn_started arrives late
        chat.set_step_text("r1", 1, "你好"); // authoritative text

        let rows = assistant_ids(&chat);
        assert_eq!(rows.len(), 1, "no duplicate intermediate bubble");
        assert_eq!(rows[0].0, "assistant-r1");
        assert_eq!(rows[0].1, Some(1));
        let content = chat.messages.with(|m| m[0].content.clone());
        assert_eq!(content, "你好");
    }

    #[test]
    fn set_step_text_overwrites_streamed_preview() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "par");
        chat.append_chunk("r1", "tial");
        chat.set_step_text("r1", 1, "authoritative");

        let content = chat.messages.with(|m| m[0].content.clone());
        assert_eq!(content, "authoritative");
    }

    #[test]
    fn set_step_text_targets_finalized_step_by_iteration() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "x");
        chat.begin_step("r1", 2); // finalizes step 1 as intermediate
        chat.set_step_text("r1", 1, "late fix");

        let content = chat.messages.with(|m| m[0].content.clone());
        assert_eq!(content, "late fix");
    }

    #[test]
    fn late_chunk_after_finalize_does_not_duplicate() {
        // Symptom B: `text_emitted` (set_step_text) and the streamed
        // `response_chunk` (append_chunk) ride independent async pipelines. When
        // the authoritative text lands first and a preview chunk arrives after,
        // the late chunk must be dropped — not appended on top — or the text
        // shows doubled ("ok…report. ok…report.").
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.set_step_text("r1", 1, "好的，我来继续制作HTML报告。");
        // Late streamed preview for the same turn races in after the
        // authoritative text — must be ignored.
        chat.append_chunk("r1", "好的，我来继续制作HTML报告。");

        let content = chat.messages.with(|m| m[0].content.clone());
        assert_eq!(content, "好的，我来继续制作HTML报告。");
    }

    #[test]
    fn finalize_answer_promotes_trailing_tool_turn() {
        // Symptom A: the run's last turn emitted the answer text *and* a tool
        // call, then ended. `finalize_answer` (driven by run_complete's
        // authoritative summary.final_response) must flag the trailing bubble
        // `is_final` and overwrite its text, so the timeline lifts it out of the
        // step strip — while its tool call is preserved (rendered inline).
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "report draft");
        chat.update_tool("r1", "t1", "web_fetch", "completed", Some(3));
        chat.complete_run("r1");
        chat.finalize_answer("r1", "AUTHORITATIVE REPORT");

        chat.messages.with(|m| {
            let bubble = m
                .iter()
                .find(|b| b.id == "assistant-r1")
                .expect("trailing bubble");
            assert!(bubble.is_final, "promoted to final answer");
            assert!(!bubble.is_intermediate);
            assert!(!bubble.is_streaming);
            assert_eq!(bubble.content, "AUTHORITATIVE REPORT");
            assert!(
                !bubble.tool_calls.is_empty(),
                "tool call preserved for inline render"
            );
        });
    }

    #[test]
    fn provider_retry_notice_cleared_on_first_chunk() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.set_provider_retry(ProviderRetryNotice {
            provider: "kimi-for-coding".into(),
            attempt: 2,
            max_attempts: 3,
        });
        assert!(chat.provider_retry.with_untracked(|n| n.is_some()));

        chat.append_chunk("r1", "hello");
        assert!(
            chat.provider_retry.with_untracked(|n| n.is_none()),
            "provider responded — retry notice must clear"
        );
    }

    #[test]
    fn provider_retry_notice_cleared_when_run_settles() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.set_provider_retry(ProviderRetryNotice {
            provider: "302ai".into(),
            attempt: 3,
            max_attempts: 3,
        });
        chat.fail_run("r1", "provider 302ai transient: Request timed out");
        assert!(
            chat.provider_retry.with_untracked(|n| n.is_none()),
            "run settled — retry notice must clear"
        );
    }

    #[test]
    fn classify_provider_timeout_is_cloud_send_failed() {
        // Real message from a provider-chain network outage (2026-06-10 log).
        // Must NOT be labelled SafetyTimeout — the harness never timed out;
        // the upstream was unreachable.
        let e = ChatSendError::classify(
            "Execution failed: provider failover transient: llm error: Request timed out",
        );
        assert_eq!(e.code, ChatSendErrorCode::CloudSendFailed);
    }

    #[test]
    fn classify_harness_turn_timeout_is_safety_timeout() {
        // Actual humanized TerminateReason::TurnTimeout text from
        // orchestrator::summary_format — the case SafetyTimeout exists for.
        let e = ChatSendError::classify("Turn timeout in think (5m 0s)");
        assert_eq!(e.code, ChatSendErrorCode::SafetyTimeout);
    }

    #[test]
    fn classify_harness_stall_is_safety_timeout() {
        // Humanized TerminateReason::StallTimeout text.
        let e = ChatSendError::classify("Stalled after 3m 0s");
        assert_eq!(e.code, ChatSendErrorCode::SafetyTimeout);
    }

    #[test]
    fn finalize_answer_ignores_empty_final_text() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "kept");
        chat.finalize_answer("r1", "   ");

        chat.messages.with(|m| {
            let bubble = m.iter().find(|b| b.id == "assistant-r1").unwrap();
            assert!(!bubble.is_final, "empty final text is a no-op");
            assert_eq!(bubble.content, "kept");
        });
    }

    #[test]
    fn chat_message_agent_id_defaults_none_for_legacy_json() {
        let legacy = serde_json::json!({ "id": "a", "role": "assistant", "content": "hi" });
        let msg: ChatMessage = serde_json::from_value(legacy).unwrap();
        assert_eq!(msg.agent_id, None);
    }

    #[test]
    fn chat_message_roundtrips_agent_id() {
        let msg: ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "m", "role": "assistant", "content": "x", "agent_id": "alice"
        }))
        .unwrap();
        assert_eq!(msg.agent_id.as_deref(), Some("alice"));
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v.get("agent_id").and_then(|a| a.as_str()), Some("alice"));
    }

    use super::super::plan::{PlanItemStatusView, PlanItemView, PlanView};

    fn plan(items: &[(&str, PlanItemStatusView)], complete: bool) -> PlanView {
        PlanView {
            objective: Some("Obj".into()),
            items: items
                .iter()
                .map(|(t, s)| PlanItemView {
                    text: (*t).into(),
                    status: s.clone(),
                })
                .collect(),
            complete,
        }
    }

    fn archive_count(chat: &ChatState) -> usize {
        chat.messages
            .with(|m| m.iter().filter(|x| x.plan_archive.is_some()).count())
    }

    #[test]
    fn archive_activity_sinks_worked_plan_and_clears_slot() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan
            .set(Some(plan(&[("a", PlanItemStatusView::Completed)], false)));
        chat.archive_active_plan(ArchiveGate::Activity);
        assert_eq!(archive_count(&chat), 1, "worked plan sinks one capsule");
        assert!(
            chat.plan.get_untracked().is_none(),
            "slot cleared after archive"
        );
    }

    #[test]
    fn archive_activity_skips_pristine_plan() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan
            .set(Some(plan(&[("a", PlanItemStatusView::Pending)], false)));
        chat.archive_active_plan(ArchiveGate::Activity);
        assert_eq!(archive_count(&chat), 0, "pristine plan is not archived");
        assert!(
            chat.plan.get_untracked().is_some(),
            "slot left for overwrite"
        );
    }

    #[test]
    fn archive_completed_gate_ignores_incomplete() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan
            .set(Some(plan(&[("a", PlanItemStatusView::InProgress)], false)));
        chat.archive_active_plan(ArchiveGate::Completed);
        assert_eq!(
            archive_count(&chat),
            0,
            "in-progress plan not sunk on Completed gate"
        );
        assert!(chat.plan.get_untracked().is_some());
    }

    #[test]
    fn start_assistant_message_sinks_completed_plan() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan
            .set(Some(plan(&[("a", PlanItemStatusView::Completed)], true)));
        chat.start_assistant_message("r2");
        assert_eq!(
            archive_count(&chat),
            1,
            "completed plan sinks at next run start"
        );
        assert!(chat.plan.get_untracked().is_none());
    }

    #[test]
    fn start_assistant_message_keeps_incomplete_plan() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan
            .set(Some(plan(&[("a", PlanItemStatusView::InProgress)], false)));
        chat.start_assistant_message("r2");
        assert_eq!(
            archive_count(&chat),
            0,
            "in-progress plan stays in the sticky slot"
        );
        assert!(chat.plan.get_untracked().is_some());
    }

    #[test]
    fn context_usage_clears_on_reset_but_survives_tab_swap() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let seed = || {
            chat.context_usage.set(Some(ContextUsage {
                used_tokens: 10_000,
                window_tokens: 200_000,
                total_tokens: 12_000,
                is_estimate: false,
            }))
        };

        seed();
        chat.clear();
        assert!(
            chat.context_usage.get_untracked().is_none(),
            "clear() must reset the context gauge"
        );

        seed();
        chat.clear_session();
        assert!(
            chat.context_usage.get_untracked().is_none(),
            "clear_session() must reset the context gauge"
        );

        // Tab swap = capture outgoing tab → restore incoming tab. The gauge now
        // rides in SessionSnapshot, so a tab that already ran a turn keeps its
        // occupancy across the swap instead of blanking out (the user-visible
        // "switching tabs hides the gauge" complaint).
        seed();
        let snap = chat.capture_snapshot();
        chat.context_usage.set(None);
        chat.restore_from(snap);
        assert_eq!(
            chat.context_usage.get_untracked(),
            Some(ContextUsage {
                used_tokens: 10_000,
                window_tokens: 200_000,
                total_tokens: 12_000,
                is_estimate: false,
            }),
            "restore_from() must rehydrate the captured gauge so it survives tab swaps"
        );

        // A fresh/empty tab (default snapshot) still shows no gauge.
        chat.restore_from(SessionSnapshot::default());
        assert!(
            chat.context_usage.get_untracked().is_none(),
            "restoring an empty snapshot must leave the gauge hidden"
        );
    }

    fn plan_of(
        items: &[(&str, super::super::plan::PlanItemStatusView)],
        complete: bool,
    ) -> PlanView {
        PlanView {
            objective: Some("Ship".into()),
            items: items
                .iter()
                .map(|(t, s)| super::super::plan::PlanItemView {
                    text: (*t).into(),
                    status: *s,
                })
                .collect(),
            complete,
        }
    }

    #[test]
    fn plan_survives_a_tab_swap() {
        use super::super::plan::PlanItemStatusView::{InProgress, Pending};
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.apply_plan_update(PlanUpdate::Show(plan_of(
            &[("a", InProgress), ("b", Pending)],
            false,
        )));

        // Tab swap = capture outgoing → restore incoming.
        let snap = chat.capture_snapshot();
        chat.plan.set(None);
        chat.restore_from(snap);

        let restored = chat.plan.get_untracked().expect("plan rides the snapshot");
        assert_eq!(restored.current_step(), Some("a"));

        // A fresh tab still shows nothing.
        chat.restore_from(SessionSnapshot::default());
        assert!(chat.plan.get_untracked().is_none());
    }

    #[test]
    fn settle_plan_reconciles_a_stale_live_checklist() {
        use super::super::plan::PlanItemStatusView::{Completed, InProgress, Pending};
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        // Live projection lost the last two ticks (dropped agent_trace frames).
        chat.apply_plan_update(PlanUpdate::Show(plan_of(
            &[("a", Completed), ("b", InProgress), ("c", Pending)],
            false,
        )));

        let authoritative = plan_of(
            &[("a", Completed), ("b", Completed), ("c", Completed)],
            true,
        );
        chat.settle_plan(Some(&authoritative));

        // Complete → sunk into the transcript, strip cleared.
        assert!(
            chat.plan.get_untracked().is_none(),
            "a finished plan sinks at run end instead of staying mounted"
        );
        let archived = chat.messages.with_untracked(|m| {
            m.iter()
                .filter_map(|x| x.plan_archive.clone())
                .last()
                .expect("an archive capsule")
        });
        assert_eq!(
            archived.done_count(),
            3,
            "the sunk capsule shows 3/3, not 1/3"
        );
    }

    #[test]
    fn settle_plan_keeps_an_unfinished_list_mounted() {
        use super::super::plan::PlanItemStatusView::{Completed, Pending};
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.apply_plan_update(PlanUpdate::Show(plan_of(
            &[("a", Completed), ("b", Pending)],
            false,
        )));
        chat.settle_plan(Some(&plan_of(&[("a", Completed), ("b", Pending)], false)));
        assert!(
            chat.plan.get_untracked().is_some(),
            "the objective is still open — the user must keep seeing what is left"
        );
    }

    #[test]
    fn settle_plan_without_a_summary_leaves_the_live_plan_alone() {
        use super::super::plan::PlanItemStatusView::{Completed, Pending};
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.apply_plan_update(PlanUpdate::Show(plan_of(
            &[("a", Completed), ("b", Pending)],
            false,
        )));
        chat.settle_plan(None);
        assert_eq!(
            chat.plan.get_untracked().map(|p| p.done_count()),
            Some(1),
            "an older core with no summary.plan must not blank the strip"
        );
    }

    /// Every tool row of `run`, flattened across its step bubbles.
    fn rows(chat: &ChatState) -> Vec<(String, String, Option<u64>)> {
        chat.messages.with_untracked(|msgs| {
            msgs.iter()
                .flat_map(|m| m.tool_calls.iter())
                .map(|t| (t.tool_id.clone(), t.status.clone(), t.duration_ms))
                .collect()
        })
    }

    /// Regression: a `tool_call_completed` that lands *after* the next
    /// `turn_started` renamed its bubble used to miss its own row (the search
    /// covered only the trailing `assistant-{run}` bubble) and append a phantom
    /// duplicate to the fresh step, leaving the original pinned on `running`.
    #[test]
    fn update_tool_reaches_a_row_in_an_earlier_step_bubble() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.update_tool("r1", "t1", "bash", "running", None);
        // Next turn opens: the step-1 bubble is renamed, a fresh
        // `assistant-r1` is pushed. Only then does t1's completion arrive.
        chat.begin_step("r1", 2);
        chat.update_tool("r1", "t1", "bash", "completed", Some(90));

        assert_eq!(
            rows(&chat),
            vec![("t1".to_string(), "completed".to_string(), Some(90))],
            "one row, settled in place — not a stuck row plus a phantom copy"
        );
    }

    #[test]
    fn reconcile_tools_is_authoritative_over_the_streamed_status() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.update_tool("r1", "t1", "bash", "running", None);

        chat.reconcile_tools(
            "r1",
            &[ToolSettlement {
                tool_id: "t1".into(),
                tool_name: "bash".into(),
                duration_ms: 33,
                success: false,
            }],
        );
        assert_eq!(
            rows(&chat),
            vec![("t1".to_string(), "failed".to_string(), Some(33))]
        );
        // Empty settlement list is a no-op, not a wipe.
        chat.reconcile_tools("r1", &[]);
        assert_eq!(rows(&chat).len(), 1);
    }

    #[test]
    fn settle_orphan_tools_touches_only_running_rows_of_that_run() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.update_tool("r1", "done", "bash", "completed", Some(5));
        chat.update_tool("r1", "stuck", "bash", "running", None);
        chat.start_assistant_message("r2");
        chat.update_tool("r2", "other", "bash", "running", None);

        chat.settle_orphan_tools("r1");

        let by_id: std::collections::HashMap<_, _> = rows(&chat)
            .into_iter()
            .map(|(id, status, _)| (id, status))
            .collect();
        assert_eq!(by_id["done"], "completed", "terminal rows are left alone");
        assert_eq!(by_id["stuck"], TOOL_STATUS_UNKNOWN);
        assert_eq!(
            by_id["other"], "running",
            "another run's live tool must keep running"
        );
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn prompt(text: &str, attachments: usize) -> QueuedPrompt {
        QueuedPrompt {
            text: text.to_string(),
            attachments: (0..attachments)
                .map(|i| PendingAttachment {
                    name: format!("f{i}"),
                    mime_type: "text/plain".into(),
                    data_base64: String::new(),
                    size: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn label_uses_trimmed_text() {
        assert_eq!(queue_preview_label(&prompt("  hello  ", 0)), "hello");
    }

    #[test]
    fn label_truncates_on_codepoint_boundary() {
        let long = "a".repeat(100);
        let out = queue_preview_label(&prompt(&long, 0));
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 65); // 64 chars + ellipsis
    }

    #[test]
    fn label_falls_back_to_attachment_count() {
        assert_eq!(queue_preview_label(&prompt("   ", 2)), "📎 2");
    }

    #[test]
    fn drain_all_queued_empties_and_preserves_order() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.enqueue_prompt(prompt("a", 0));
        chat.enqueue_prompt(prompt("b", 0));
        let drained = chat.drain_all_queued();
        let texts: Vec<_> = drained.iter().map(|p| p.text.clone()).collect();
        assert_eq!(texts, vec!["a", "b"]);
        assert!(chat.prompt_queue.get_untracked().is_empty());
    }

    #[test]
    fn a_queued_runs_failure_does_not_tear_down_the_live_run() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("live");

        // A second run id the Panel is holding — queued in the gateway's wait
        // lane — fails (purged by a Stop, rejected by a full lane, timed out).
        chat.fail_run("queued", "queue full");

        assert_eq!(
            chat.active_run_id.get_untracked().as_deref(),
            Some("live"),
            "the live run must survive another run's failure"
        );
        assert!(
            chat.send_error.get_untracked().is_some(),
            "the failure is still reported — it just does not end the live run"
        );

        // The live run's own failure does tear it down.
        chat.fail_run("live", "provider unreachable");
        assert!(chat.active_run_id.get_untracked().is_none());
    }

    #[test]
    fn a_stop_suppression_swaps_with_the_conversation_that_earned_it() {
        // Pressing Stop in one conversation must not spend the suppression owed
        // to whichever conversation the user opens next — the queue it gates is
        // per-conversation, so the flag has to travel with it.
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.enqueue_prompt(prompt("stopped conversation's queue", 0));
        chat.stop_suppresses_next_drain.set(true);
        let stopped = chat.capture_snapshot();

        // Open a different conversation: it owes no suppression.
        chat.restore_from(SessionSnapshot::default());
        assert!(
            !chat.stop_suppresses_next_drain.get_untracked(),
            "a fresh conversation must not inherit another one's Stop"
        );

        // Back to the one that was stopped: its suppression is still armed.
        chat.restore_from(stopped);
        assert!(chat.stop_suppresses_next_drain.get_untracked());
    }

    #[test]
    fn a_failed_flush_gives_the_unsent_prompts_back_ahead_of_newer_ones() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.enqueue_prompt(prompt("a", 0));
        chat.enqueue_prompt(prompt("b", 0));

        // Flush drains the batch; the first send fails, so nothing was sent.
        let batch = chat.drain_all_queued();
        // Meanwhile the user queues another one.
        chat.enqueue_prompt(prompt("typed during the flush", 0));
        chat.requeue_front(batch);

        let texts: Vec<_> = chat
            .prompt_queue
            .get_untracked()
            .into_iter()
            .map(|p| p.text)
            .collect();
        assert_eq!(
            texts,
            vec!["a", "b", "typed during the flush"],
            "prompts that never left the client keep their place in the arrival order"
        );
    }

    #[test]
    fn recall_takes_the_newest_and_shortens_the_queue() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.enqueue_prompt(prompt("a", 0));
        chat.enqueue_prompt(prompt("b", 0));

        assert_eq!(
            chat.recall_latest_queued().map(|p| p.text).as_deref(),
            Some("b")
        );
        assert_eq!(
            chat.prompt_queue.get_untracked().len(),
            1,
            "the recalled prompt must leave the queue, not be copied out of it"
        );
        assert_eq!(
            chat.recall_latest_queued().map(|p| p.text).as_deref(),
            Some("a")
        );
        assert!(
            chat.recall_latest_queued().is_none(),
            "an empty queue recalls nothing"
        );
    }

    #[test]
    fn two_recalls_before_the_composer_drains_keep_both_and_their_order() {
        // The composer's drain runs in an Effect, i.e. after the keystroke.
        // Two fast presses must not let the second seed overwrite the first —
        // that would silently destroy a queued message.
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.enqueue_prompt(prompt("first", 0));
        chat.enqueue_prompt(prompt("second", 0));

        for _ in 0..2 {
            let entry = chat.recall_latest_queued().expect("queued prompt");
            chat.seed_draft(entry.text, entry.attachments);
        }

        assert_eq!(
            chat.draft.get_untracked(),
            "first\n\nsecond",
            "recalling from the tail and prepending rebuilds the queue's order"
        );
        assert!(chat.prompt_queue.get_untracked().is_empty());
    }

    #[test]
    fn recall_joins_an_in_progress_draft_instead_of_replacing_it() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.draft.set("half a thought".into());
        chat.enqueue_prompt(prompt("queued", 0));

        let entry = chat.recall_latest_queued().expect("queued prompt");
        chat.seed_draft(entry.text, entry.attachments);

        assert_eq!(chat.draft.get_untracked(), "queued\n\nhalf a thought");
    }

    #[test]
    fn a_tab_swap_carries_the_draft_with_the_queue_and_the_tray() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.draft.set("conversation A's unsent line".into());
        chat.enqueue_prompt(prompt("A queued", 0));
        let snap = chat.capture_snapshot();

        // Project conversation B over the same signals, then come back to A.
        chat.draft.set("conversation B's line".into());
        chat.prompt_queue.set(Vec::new());
        chat.restore_from(snap);

        assert_eq!(
            chat.draft.get_untracked(),
            "conversation A's unsent line",
            "the draft must swap with the conversation, not linger on the next one"
        );
        assert_eq!(chat.prompt_queue.get_untracked().len(), 1);
    }

    #[test]
    fn seeding_a_prompt_with_files_stages_them_ahead_of_what_is_already_attached() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.pending_attachments.set(prompt("", 1).attachments);

        chat.seed_draft("recalled".into(), prompt("", 2).attachments);

        assert_eq!(
            chat.pending_attachments.get_untracked().len(),
            3,
            "a recalled prompt's files must join the staged ones, not replace them"
        );
    }

    #[test]
    fn taking_a_queued_prompt_by_index_returns_it_and_removes_it_once() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.enqueue_prompt(prompt("a", 0));
        chat.enqueue_prompt(prompt("b", 0));

        assert_eq!(
            chat.take_queued_prompt(0).map(|p| p.text).as_deref(),
            Some("a")
        );
        let left: Vec<_> = chat
            .prompt_queue
            .get_untracked()
            .into_iter()
            .map(|p| p.text)
            .collect();
        assert_eq!(left, vec!["b"]);
        assert!(
            chat.take_queued_prompt(7).is_none(),
            "an out-of-range index must not disturb the queue"
        );
        assert_eq!(chat.prompt_queue.get_untracked().len(), 1);
    }
}

#[cfg(test)]
mod tool_timestamp_tests {
    use super::*;

    #[test]
    fn update_tool_stamps_started_at_on_first_running() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1"); // 建 assistant-r1 容器
        chat.update_tool("r1", "t1", "bash", "running", None);
        let started = chat.messages.with_untracked(|m| {
            m.iter()
                .flat_map(|m| m.tool_calls.iter())
                .find(|t| t.tool_id == "t1")
                .and_then(|t| t.started_at_ms)
        });
        assert!(started.is_some(), "first running must stamp started_at_ms");

        // don't overwrite timestamp on completion
        chat.update_tool("r1", "t1", "bash", "completed", Some(30));
        let after = chat.messages.with_untracked(|m| {
            m.iter()
                .flat_map(|m| m.tool_calls.iter())
                .find(|t| t.tool_id == "t1")
                .map(|t| (t.started_at_ms, t.status.clone()))
        });
        assert_eq!(after.map(|(s, _)| s), Some(started));
    }
}

#[cfg(test)]
mod run_cost_tests {
    use super::*;

    fn cost(usd: Option<f64>, status: &str, total: u64) -> RunCost {
        RunCost {
            usd,
            status: Some(status.to_string()),
            total_tokens: total,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    #[test]
    fn a_fully_priced_run_renders_an_exact_figure() {
        let c = cost(Some(0.1234), "complete", 12_345);
        assert!(c.is_exact());
        assert_eq!(c.cost_label().as_deref(), Some("$0.12"));
        assert_eq!(c.tokens_label().as_deref(), Some("12.3k tok"));
    }

    #[test]
    fn a_partially_priced_run_is_never_passed_off_as_exact() {
        // THE contract: core could not price every model in the run, so the
        // figure must read as an approximation.
        let c = cost(Some(0.5), "partial_missing_price", 900);
        assert!(!c.is_exact());
        assert_eq!(c.cost_label().as_deref(), Some("≈$0.50"));
        assert_eq!(c.tokens_label().as_deref(), Some("900 tok"));
        assert!(!cost(Some(0.5), "unknown", 1).is_exact());
    }

    #[test]
    fn sub_cent_runs_keep_four_decimals() {
        // "$0.00" reads as free; a cheap run is not a free one.
        assert_eq!(
            cost(Some(0.0034), "complete", 10).cost_label().as_deref(),
            Some("$0.0034")
        );
    }

    #[test]
    fn absent_price_or_zero_tokens_render_nothing() {
        // Rendering "$0.00" / "0 tok" for an unpriced or cached turn reads as
        // broken, so both labels stay absent.
        let c = cost(None, "unknown", 0);
        assert!(c.cost_label().is_none());
        assert!(c.tokens_label().is_none());
    }
}
