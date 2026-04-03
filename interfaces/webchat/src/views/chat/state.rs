//! Chat reactive state — signals for chat messages, streaming, and UI mode.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChatPhase {
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
    pub error_message: RwSignal<Option<String>>,
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
            next_msg_id: RwSignal::new(0),
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
        self.error_message.set(Some(error.to_string()));
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
    }

    /// Clear session state but keep agent_id (for new chat within same agent).
    pub fn clear_session(&self) {
        self.messages.set(Vec::new());
        self.phase.set(ChatPhase::Idle);
        self.active_run_id.set(None);
        self.session_key.set(None);
        self.reasoning_text.set(String::new());
        self.error_message.set(None);
        // agent_id is intentionally preserved
    }
}
