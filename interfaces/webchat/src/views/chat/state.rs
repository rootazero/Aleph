//! Chat reactive state — signals for chat messages, streaming, and UI mode.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

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

/// Context-window occupancy snapshot for the composer gauge.
///
/// Populated from each `run_complete` summary (`token_breakdown.input` is the
/// last turn's context tokens, `total_tokens` the run's running total). The
/// window denominator is resolved client-side by
/// [`super::context_gauge::context_window_for`] since the panel — an I/O-only
/// interface (R4) — cannot reach core's model catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsage {
    /// Input/context tokens occupying the window after the latest turn.
    pub used_tokens: u32,
    /// Resolved context-window size for the run's model (gauge denominator).
    pub window_tokens: u32,
    /// Running total tokens billed for the run (shown in the tooltip).
    pub total_tokens: u64,
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
}

/// Minimal tool call record for display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub tool_id: String,
    pub tool_name: String,
    pub status: String, // "running" | "completed" | "failed"
    #[serde(default)]
    pub duration_ms: Option<u64>,
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
    /// One-shot composer prefill request. Empty-state suggestion chips (and any
    /// future "insert prompt" source) drop a starter string here; the composer's
    /// consume-Effect drains it into the textarea and resets it to `None`.
    /// Ephemeral like `is_dragging_files` / `retry_pulse` — intentionally
    /// excluded from [`SessionSnapshot`] so it never replays on a tab swap.
    pub draft_seed: RwSignal<Option<String>>,
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
    /// Pulse signal that asks the composer to run the doctor + LLM-repair
    /// flow (G1, `f` hotkey): each bump increments by 1. Bumped by the global
    /// keydown listener via [`HotkeyState`]; the composer seeds a diagnostic
    /// instruction and routes it through the normal send pipeline. Ephemeral,
    /// like `retry_pulse` — excluded from [`SessionSnapshot`].
    pub repair_pulse: RwSignal<u32>,
    /// Active project workspace root (absolute path). When `Some`, the
    /// chat composer attaches it to `chat.send` as `project_root`, and the
    /// daemon swaps the agent's working directory for the duration of the
    /// run. Switching project clears the session per the "切换即开新
    /// session" convention agreed for the desktop App.
    pub active_project_root: RwSignal<Option<String>>,
    /// Human-friendly display name for the active project. Surfaced in the
    /// composer's "进入项目工作 ▾" chip so the user always sees which
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
    /// does not survive a session-tab swap; re-enter via the 团队 compose button.
    pub team_id: RwSignal<Option<String>>,
    /// Team roster + live status (left roster rail data source). Empty = non-team.
    pub team_members: RwSignal<Vec<TeamMemberView>>,
    /// Per-run step-strip expand/collapse override, keyed by `run_id`.
    /// Absent = use the default (running strips open, completed collapsed);
    /// present = the user's explicit toggle. Lives here — not as a strip-local
    /// signal — because `timeline::row_key` folds in content length, so the
    /// strip row remounts on every streamed token and a component-local `open`
    /// would reset each time (re-opening a strip the user just collapsed mid-run).
    /// Ephemeral, like `retry_pulse` — excluded from [`SessionSnapshot`].
    pub strip_open: RwSignal<std::collections::HashMap<String, bool>>,
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
            draft_seed: RwSignal::new(None),
            context_usage: RwSignal::new(None),
            prompt_queue: RwSignal::new(Vec::new()),
            retry_pulse: RwSignal::new(0),
            repair_pulse: RwSignal::new(0),
            active_project_root: RwSignal::new(None),
            active_project_name: RwSignal::new(None),
            selected_model: RwSignal::new(None),
            voice_run_ids: RwSignal::new(Vec::new()),
            provider_retry: RwSignal::new(None),
            next_msg_id: RwSignal::new(0),
            team_id: RwSignal::new(None),
            team_members: RwSignal::new(Vec::new()),
            strip_open: RwSignal::new(std::collections::HashMap::new()),
        }
    }

    /// Whether run `run_id`'s step strip is expanded. `default_open` is the
    /// state to use when the user hasn't toggled it (running strips default
    /// open, completed strips default collapsed).
    #[must_use]
    pub fn strip_is_open(&self, run_id: &str, default_open: bool) -> bool {
        self.strip_open
            .with(|m| m.get(run_id).copied())
            .unwrap_or(default_open)
    }

    /// Toggle run `run_id`'s step-strip expand state, seeding from
    /// `default_open` when the user hasn't toggled it before. The stored
    /// override survives the strip row's per-token remount.
    pub fn toggle_strip(&self, run_id: &str, default_open: bool) {
        let next = !self.strip_is_open(run_id, default_open);
        self.strip_open.update(|m| {
            m.insert(run_id.to_string(), next);
        });
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

    /// Pop the head of the queue, if any. Returns the prompt to replay.
    #[must_use]
    pub fn dequeue_prompt_front(&self) -> Option<QueuedPrompt> {
        let mut popped = None;
        self.prompt_queue.update(|q| {
            if !q.is_empty() {
                popped = Some(q.remove(0));
            }
        });
        popped
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
            });
        });
        self.error_message.set(None);
    }

    /// Start a new assistant message placeholder (streaming).
    pub fn start_assistant_message(&self, run_id: &str) {
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
                iteration: None,
                agent_id: None,
            });
        });
        self.active_run_id.set(Some(run_id.to_string()));
        self.phase.set(ChatPhase::Thinking);
        self.reasoning_text.set(String::new());
    }

    /// Begin a new agent step (Think→Act iteration) for `run_id`.
    ///
    /// Driven by `agent_trace.turn_started`. If the current
    /// `assistant-{run_id}` bubble already carries text or tool calls, it is
    /// finalized as a standalone intermediate step and a fresh streaming
    /// bubble tagged with `iteration` is started. An empty placeholder (the
    /// one created on `run_accepted` before the first turn) is reused — just
    /// stamped with the iteration — to avoid an empty step bubble.
    pub fn begin_step(&self, run_id: &str, iteration: usize) {
        let target_id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            let len = msgs.len();
            if let Some(idx) = msgs.iter().rposition(|m| m.id == target_id) {
                let has_payload = !msgs[idx].content.is_empty() || !msgs[idx].tool_calls.is_empty();
                // Only finalize the bubble as a completed intermediate step when
                // it already belongs to a *different, already-stamped* iteration.
                // A bubble still tagged `iteration == None` is the un-stamped
                // placeholder for the step we're just now starting: streamed
                // `response_chunk` preview text may have raced ahead of this
                // `turn_started` (the two travel independent async pipelines —
                // AgentTraceEmitSink spawns a drain task), so its content belongs
                // to THIS step. Reuse it instead of orphaning the preview into a
                // duplicate intermediate bubble that `set_step_text` then mirrors.
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
    pub fn update_tool(
        &self,
        run_id: &str,
        tool_id: &str,
        tool_name: &str,
        status: &str,
        duration_ms: Option<u64>,
    ) {
        let target_id = format!("assistant-{run_id}");
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                if let Some(tc) = msg.tool_calls.iter_mut().find(|t| t.tool_id == tool_id) {
                    tc.status = status.to_string();
                    tc.duration_ms = duration_ms;
                } else {
                    msg.tool_calls.push(ToolCallEntry {
                        tool_id: tool_id.to_string(),
                        tool_name: tool_name.to_string(),
                        status: status.to_string(),
                        duration_ms,
                    });
                }
            }
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
        self.active_run_id.set(None);
        self.phase.set(ChatPhase::Error);
        self.clear_provider_retry();
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
        self.team_id.set(None);
        self.team_members.set(Vec::new());
        self.strip_open.set(std::collections::HashMap::new());
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
            active_project_root: self.active_project_root.get_untracked(),
            active_project_name: self.active_project_name.get_untracked(),
            selected_model: self.selected_model.get_untracked(),
            next_msg_id: self.next_msg_id.get_untracked(),
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
        self.active_project_root.set(snap.active_project_root);
        self.active_project_name.set(snap.active_project_name);
        self.selected_model.set(snap.selected_model);
        self.next_msg_id.set(snap.next_msg_id);
        // Ephemeral (not in the snapshot): reset so the outgoing tab's
        // collapse choices don't leak into the restored session.
        self.strip_open.set(std::collections::HashMap::new());
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
    pub active_project_root: Option<String>,
    pub active_project_name: Option<String>,
    pub selected_model: Option<crate::api::providers::ModelOverride>,
    pub next_msg_id: u64,
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
    fn begin_step_reuses_unstamped_placeholder_with_raced_preview() {
        // Regression (double-render): `response_chunk` deltas and
        // `agent_trace.turn_started` travel independent async pipelines
        // (AgentTraceEmitSink spawns a drain task), so streamed preview text can
        // land in the placeholder bubble BEFORE the first `turn_started`. The
        // late `begin_step` must REUSE that un-stamped (iteration == None)
        // placeholder — its content is THIS step's preview — not orphan it into
        // a duplicate `intermediate-` bubble that `set_step_text` then mirrors.
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
        // shows doubled ("好的…报告。好的…报告。").
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
}
