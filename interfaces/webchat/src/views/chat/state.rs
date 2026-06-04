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
            } else if l.contains("safety timeout") || l.contains("timed out") {
                ChatSendErrorCode::SafetyTimeout
            } else if l.contains("cloud") || l.contains("http") || l.contains("provider") {
                ChatSendErrorCode::CloudSendFailed
            } else {
                ChatSendErrorCode::Unknown
            };
        Self { code, message }
    }
}

/// Model resolution info (mirrors core ModelInfo).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
}

/// Minimal tool call record for display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallEntry {
    pub tool_id: String,
    pub tool_name: String,
    pub status: String, // "running" | "completed" | "failed"
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// Top-level Chat UI phase.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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
    /// Active run_id (Some while agent is running).
    pub active_run_id: RwSignal<Option<String>>,
    /// Resolved session key from first chat.send response.
    pub session_key: RwSignal<Option<String>>,
    /// Currently selected agent ID for routing.
    pub agent_id: RwSignal<Option<String>>,
    /// Accumulated reasoning text for the current run.
    pub reasoning_text: RwSignal<String>,
    /// Error message (set when run_error arrives).
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
    /// Follow-up prompts queued while a run is active. Drained one-at-a-time
    /// when the turn settles naturally (see
    /// [`shared_ui_logic::state::should_auto_drain_on_settle`]). Session-scoped
    /// content, so it rides along in [`SessionSnapshot`] across tab swaps.
    pub prompt_queue: RwSignal<Vec<QueuedPrompt>>,
    /// Pulse signal that asks the composer to retry the last user message:
    /// each bump increments by 1. Used by MessageBubble's retry button so
    /// the composer (which owns the send pipeline) actually fires the send
    /// without prop drilling a callback through every bubble.
    pub retry_pulse: RwSignal<u32>,
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
    /// Monotonic counter for generating unique user message IDs.
    next_msg_id: RwSignal<u64>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatState {
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
            prompt_queue: RwSignal::new(Vec::new()),
            retry_pulse: RwSignal::new(0),
            active_project_root: RwSignal::new(None),
            active_project_name: RwSignal::new(None),
            selected_model: RwSignal::new(None),
            voice_run_ids: RwSignal::new(Vec::new()),
            next_msg_id: RwSignal::new(0),
        }
    }

    /// Append a follow-up prompt to the tail of the queue.
    pub fn enqueue_prompt(&self, entry: QueuedPrompt) {
        self.prompt_queue.update(|q| q.push(entry));
    }

    /// Pop the head of the queue, if any. Returns the prompt to replay.
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
    pub fn assistant_text_for_run(&self, run_id: &str) -> String {
        let target_id = format!("assistant-{}", run_id);
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
        let id = format!("user-{}", seq);
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
                timestamp: Some(super::timeline::now_millis()),
            });
        });
        self.error_message.set(None);
    }

    /// Start a new assistant message placeholder (streaming).
    pub fn start_assistant_message(&self, run_id: &str) {
        let id = format!("assistant-{}", run_id);
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
                timestamp: Some(super::timeline::now_millis()),
            });
        });
        self.active_run_id.set(Some(run_id.to_string()));
        self.phase.set(ChatPhase::Thinking);
        self.reasoning_text.set(String::new());
    }

    /// Finalize the current assistant message as intermediate and start a new one.
    ///
    /// Called when the DeltaSink signals an intermediate boundary (text + tool_calls).
    /// The current message becomes a standalone intermediate message, and a new
    /// placeholder is created for the next iteration's text.
    pub fn finalize_intermediate(&self, run_id: &str) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            let len = msgs.len();
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                // Only finalize if there's content; skip empty intermediate boundaries
                if msg.content.is_empty() {
                    return;
                }
                msg.is_streaming = false;
                msg.is_intermediate = true;
                // Rename ID so the next message can reuse the run_id-based ID
                msg.id = format!("intermediate-{}-{}", run_id, len);
            }
            // Start a new placeholder for the next iteration
            msgs.push(ChatMessage {
                id: format!("assistant-{}", run_id),
                role: "assistant".into(),
                content: String::new(),
                tool_calls: vec![],
                is_streaming: true,
                is_intermediate: false,
                error: None,
                model_info: None,
                timestamp: Some(super::timeline::now_millis()),
            });
        });
        self.phase.set(ChatPhase::Thinking);
    }

    /// Set model info on the current assistant message for the given run.
    pub fn set_model_info(&self, run_id: &str, info: ModelInfo) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.model_info = Some(info);
            }
        });
    }

    /// Append a response text chunk to the current assistant message.
    pub fn append_chunk(&self, run_id: &str, content: &str) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.content.push_str(content);
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
        let target_id = format!("assistant-{}", run_id);
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
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.is_streaming = false;
            }
        });
        self.active_run_id.set(None);
        self.phase.set(ChatPhase::Idle);
    }

    /// Mark current run as errored.
    pub fn fail_run(&self, run_id: &str, error: &str) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.is_streaming = false;
                msg.error = Some(error.to_string());
            }
        });
        self.active_run_id.set(None);
        self.phase.set(ChatPhase::Error);
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

    /// Return the content of the most recent user message, if any. Used by
    /// the retry path to repopulate the composer.
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
    }

    /// Clear session state but keep agent_id (for new chat within same agent).
    pub fn clear_session(&self) {
        self.messages.set(Vec::new());
        self.phase.set(ChatPhase::Idle);
        self.active_run_id.set(None);
        self.session_key.set(None);
        self.reasoning_text.set(String::new());
        self.error_message.set(None);
        self.send_error.set(None);
        self.prompt_queue.set(Vec::new());
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
