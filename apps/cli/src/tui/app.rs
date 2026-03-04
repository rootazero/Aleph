// Core application state management for the TUI.
//
// Contains all state types (AppState, ChatMessage, Action, Focus, etc.)
// and the gateway event handler that maps StreamEvent -> state mutations.

use std::time::Duration;

use aleph_protocol::{RunSummary, StreamEvent};
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::slash::{SlashCommand, ThinkingLevel};

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
    /// Execute a slash command
    SlashCommand(SlashCommand),
    /// Cancel a running agent run
    CancelRun(String),

    // -- Scrolling --
    /// Scroll the chat view up by N lines
    ScrollUp(usize),
    /// Scroll the chat view down by N lines
    ScrollDown(usize),
    /// Jump to the bottom of the chat
    ScrollToBottom,
    /// Scroll to bottom only if auto_scroll is enabled
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

    // -- Dialog --
    /// Select a dialog option by index
    DialogSelect(usize),

    // -- Settings --
    /// Toggle verbose output mode
    ToggleVerbose,

    // -- Dialog response --
    /// Respond to an AskUser dialog
    RespondToDialog { run_id: String, choice: String },
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

/// State for the AskUser confirmation dialog.
#[derive(Debug, Clone)]
pub struct DialogState {
    pub run_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub selected: usize,
}

/// State for the command palette overlay.
#[derive(Debug, Clone)]
pub struct PaletteState {
    pub input: String,
    pub filtered: Vec<(&'static str, &'static str)>,
    pub selected: usize,
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Central application state. Owned by the main loop, mutated through
/// methods that enforce invariants (e.g. auto_scroll toggling).
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
    pub model_name: String,
    pub total_tokens: u64,
    pub is_connected: bool,

    // -- Run tracking --
    pub current_run: Option<String>,
    pub last_run_duration: Option<Duration>,

    // -- Settings --
    pub verbose: bool,
    pub thinking_level: ThinkingLevel,

    // -- UI state --
    pub focus: Focus,
    pub dialog: Option<DialogState>,
    pub palette: Option<PaletteState>,

    // -- Control --
    pub ctrl_c_count: u8,
    pub spinner_frame: usize,
    pub should_quit: bool,
}

impl AppState {
    /// Create a new AppState with a welcome system message.
    pub fn new(session_key: String, model_name: String) -> Self {
        let welcome = format!(
            "Welcome to Aleph CLI. Session: {} | Model: {}. Type /help for commands.",
            session_key, model_name,
        );
        Self {
            messages: vec![ChatMessage::System { content: welcome }],
            scroll_offset: 0,
            auto_scroll: true,

            send_history: Vec::new(),
            history_index: None,

            session_key,
            model_name,
            total_tokens: 0,
            is_connected: true,

            current_run: None,
            last_run_duration: None,

            verbose: false,
            thinking_level: ThinkingLevel::Medium,

            focus: Focus::Input,
            dialog: None,
            palette: None,

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
        let needs_new = match self.messages.last() {
            Some(ChatMessage::Assistant { .. }) => false,
            _ => true,
        };
        if needs_new {
            self.messages.push(ChatMessage::Assistant {
                content: String::new(),
                tools: Vec::new(),
                reasoning: None,
                is_streaming: true,
            });
        }
    }

    /// Return a mutable reference to the last assistant message.
    /// Panics if the last message is not an assistant message.
    /// Callers should ensure `ensure_assistant_message()` was called first.
    pub fn current_assistant_mut(&mut self) -> &mut ChatMessage {
        self.messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m, ChatMessage::Assistant { .. }))
            .expect("no assistant message found — call ensure_assistant_message() first")
    }

    /// Find a tool execution by tool_id in the last assistant message.
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

    /// Scroll up by `n` lines. Disables auto_scroll.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.auto_scroll = false;
    }

    /// Scroll down by `n` lines. If offset reaches 0, re-enables auto_scroll.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if self.scroll_offset == 0 {
            self.auto_scroll = true;
        }
    }

    /// Jump to the bottom of the chat. Re-enables auto_scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = true;
    }

    // -- Overlays -------------------------------------------------------

    /// Open the command palette, pre-populated with all commands.
    pub fn open_command_palette(&mut self) {
        self.palette = Some(PaletteState {
            input: String::new(),
            filtered: SlashCommand::all_commands(),
            selected: 0,
        });
        self.focus = Focus::CommandPalette;
    }

    /// Close any open overlay (palette or dialog) and return focus to input.
    pub fn close_overlay(&mut self) {
        self.palette = None;
        self.dialog = None;
        self.focus = Focus::Input;
    }

    /// Show an AskUser dialog.
    pub fn show_dialog(&mut self, run_id: String, question: String, options: Vec<String>) {
        self.dialog = Some(DialogState {
            run_id,
            question,
            options,
            selected: 0,
        });
        self.focus = Focus::Dialog;
    }

    // -- Settings -------------------------------------------------------

    /// Toggle verbose/debug output mode.
    pub fn toggle_verbose(&mut self) {
        self.verbose = !self.verbose;
    }

    /// Set the thinking level.
    pub fn set_thinking(&mut self, level: ThinkingLevel) {
        self.thinking_level = level;
    }

    /// Set the current model name.
    pub fn set_model(&mut self, name: String) {
        self.model_name = name;
    }

    /// Switch to a different session.
    pub fn switch_session(&mut self, key: String) {
        self.session_key = key.clone();
        self.messages.clear();
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.total_tokens = 0;
        self.current_run = None;
        self.add_system_message(format!("Switched to session: {}", key));
    }

    /// Clear the chat screen (keep session state).
    pub fn clear_screen(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.add_system_message("Screen cleared.".to_string());
    }

    /// Update token usage from a RunSummary.
    pub fn update_token_usage(&mut self, summary: &RunSummary) {
        self.total_tokens = self.total_tokens.saturating_add(summary.total_tokens);
    }

    /// Request application quit. Sets should_quit flag.
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    // -- Gateway event handling -----------------------------------------

    /// Handle a StreamEvent from the gateway. Returns an Action if the event
    /// should trigger further side effects (e.g. scrolling to bottom).
    pub fn handle_gateway_event(&mut self, event: StreamEvent) -> Action {
        match event {
            StreamEvent::RunAccepted { run_id, .. } => {
                self.current_run = Some(run_id);
                self.is_connected = true;
                Action::None
            }

            StreamEvent::Reasoning { content, .. } => {
                self.ensure_assistant_message();
                if let ChatMessage::Assistant { reasoning, .. } = self.current_assistant_mut() {
                    match reasoning {
                        Some(existing) => existing.push_str(&content),
                        None => *reasoning = Some(content),
                    }
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolStart {
                tool_name,
                tool_id,
                params,
                ..
            } => {
                self.ensure_assistant_message();
                if let ChatMessage::Assistant { tools, .. } = self.current_assistant_mut() {
                    tools.push(ToolExecution {
                        id: tool_id,
                        name: tool_name,
                        params: format_params_brief(&params),
                        status: ToolStatus::Running,
                        duration: None,
                        progress: None,
                        error: None,
                    });
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolUpdate {
                tool_id, progress, ..
            } => {
                if let Some(tool) = self.find_tool_mut(&tool_id) {
                    tool.progress = Some(progress);
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolEnd {
                tool_id,
                result,
                duration_ms,
                ..
            } => {
                if let Some(tool) = self.find_tool_mut(&tool_id) {
                    tool.status = if result.success {
                        ToolStatus::Success
                    } else {
                        ToolStatus::Failed
                    };
                    tool.duration = Some(Duration::from_millis(duration_ms));
                    if let Some(err) = result.error {
                        tool.error = Some(err);
                    }
                    tool.progress = None;
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ResponseChunk { content, .. } => {
                self.ensure_assistant_message();
                if let ChatMessage::Assistant {
                    content: msg_content,
                    ..
                } = self.current_assistant_mut()
                {
                    msg_content.push_str(&content);
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunComplete {
                summary,
                total_duration_ms,
                ..
            } => {
                self.current_run = None;
                self.last_run_duration = Some(Duration::from_millis(total_duration_ms));
                self.update_token_usage(&summary);

                // Mark the current assistant message as no longer streaming
                if let Some(ChatMessage::Assistant { is_streaming, .. }) =
                    self.messages.iter_mut().rev().find(|m| matches!(m, ChatMessage::Assistant { .. }))
                {
                    *is_streaming = false;
                }

                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunError { error, .. } => {
                self.current_run = None;

                // Mark the current assistant message as no longer streaming
                if let Some(ChatMessage::Assistant { is_streaming, .. }) =
                    self.messages.iter_mut().rev().find(|m| matches!(m, ChatMessage::Assistant { .. }))
                {
                    *is_streaming = false;
                }

                self.add_system_message(format!("Error: {}", error));
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::AskUser {
                run_id,
                question,
                options,
                ..
            } => {
                self.show_dialog(run_id, question, options);
                Action::None
            }

            StreamEvent::ReasoningBlock { content, .. } => {
                // Treated same as Reasoning — append to reasoning buffer
                self.ensure_assistant_message();
                if let ChatMessage::Assistant { reasoning, .. } = self.current_assistant_mut() {
                    match reasoning {
                        Some(existing) => {
                            existing.push('\n');
                            existing.push_str(&content);
                        }
                        None => *reasoning = Some(content),
                    }
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::UncertaintySignal {
                uncertainty,
                suggested_action,
                ..
            } => {
                let msg = format!(
                    "Uncertainty: {} ({})",
                    uncertainty,
                    suggested_action.description()
                );
                self.add_system_message(msg);
                Action::ScrollToBottomIfAutoScroll
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format tool parameters as a brief one-line summary.
///
/// - String values are returned as-is (truncated at 80 chars).
/// - Objects show `key=value` pairs, separated by spaces.
/// - Arrays show `[N items]`.
/// - Other types use their JSON representation.
pub fn format_params_brief(params: &Value) -> String {
    match params {
        Value::String(s) => {
            if s.len() > 80 {
                format!("{}...", &s[..s.char_indices().nth(77).map_or(s.len(), |(i, _)| i)])
            } else {
                s.clone()
            }
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .take(5)
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => {
                            if s.len() > 40 {
                                let end = s.char_indices().nth(37).map_or(s.len(), |(i, _)| i);
                                format!("\"{}...\"", &s[..end])
                            } else {
                                format!("\"{}\"", s)
                            }
                        }
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::Null => "null".to_string(),
                        Value::Array(arr) => format!("[{} items]", arr.len()),
                        Value::Object(_) => "{...}".to_string(),
                    };
                    format!("{}={}", k, val)
                })
                .collect();
            let result = parts.join(" ");
            if map.len() > 5 {
                format!("{} (+{} more)", result, map.len() - 5)
            } else {
                result
            }
        }
        Value::Array(arr) => format!("[{} items]", arr.len()),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_welcome_message() {
        let state = AppState::new("test-session".into(), "claude-3".into());
        assert_eq!(state.messages.len(), 1);
        match &state.messages[0] {
            ChatMessage::System { content } => {
                assert!(content.contains("test-session"));
                assert!(content.contains("claude-3"));
            }
            other => panic!("Expected System message, got: {:?}", other),
        }
        assert!(state.auto_scroll);
        assert_eq!(state.focus, Focus::Input);
        assert!(!state.should_quit);
    }

    #[test]
    fn scroll_up_disables_auto_scroll() {
        let mut state = AppState::new("s".into(), "m".into());
        assert!(state.auto_scroll);

        state.scroll_up(5);
        assert_eq!(state.scroll_offset, 5);
        assert!(!state.auto_scroll);

        // Scrolling up more adds to offset
        state.scroll_up(3);
        assert_eq!(state.scroll_offset, 8);
        assert!(!state.auto_scroll);
    }

    #[test]
    fn scroll_to_bottom_re_enables_auto_scroll() {
        let mut state = AppState::new("s".into(), "m".into());
        state.scroll_up(10);
        assert!(!state.auto_scroll);
        assert_eq!(state.scroll_offset, 10);

        state.scroll_to_bottom();
        assert!(state.auto_scroll);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn scroll_down_to_zero_re_enables_auto_scroll() {
        let mut state = AppState::new("s".into(), "m".into());
        state.scroll_up(3);
        assert!(!state.auto_scroll);

        state.scroll_down(3);
        assert_eq!(state.scroll_offset, 0);
        assert!(state.auto_scroll);
    }

    #[test]
    fn toggle_verbose() {
        let mut state = AppState::new("s".into(), "m".into());
        assert!(!state.verbose);

        state.toggle_verbose();
        assert!(state.verbose);

        state.toggle_verbose();
        assert!(!state.verbose);
    }

    #[test]
    fn format_params_string() {
        let val = Value::String("hello world".into());
        assert_eq!(format_params_brief(&val), "hello world");
    }

    #[test]
    fn format_params_string_truncation() {
        let long = "a".repeat(100);
        let result = format_params_brief(&Value::String(long));
        assert!(result.len() < 100);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn format_params_object() {
        let val = serde_json::json!({
            "command": "ls -la",
            "count": 42,
        });
        let result = format_params_brief(&val);
        assert!(result.contains("command="));
        assert!(result.contains("ls -la"));
        assert!(result.contains("count="));
        assert!(result.contains("42"));
    }

    #[test]
    fn format_params_null() {
        assert_eq!(format_params_brief(&Value::Null), "");
    }

    #[test]
    fn format_params_array() {
        let val = serde_json::json!([1, 2, 3]);
        assert_eq!(format_params_brief(&val), "[3 items]");
    }

    #[test]
    fn ensure_assistant_message_creates_one() {
        let mut state = AppState::new("s".into(), "m".into());
        // Only has system message
        assert_eq!(state.messages.len(), 1);

        state.ensure_assistant_message();
        assert_eq!(state.messages.len(), 2);
        assert!(matches!(
            state.messages[1],
            ChatMessage::Assistant {
                is_streaming: true,
                ..
            }
        ));
    }

    #[test]
    fn ensure_assistant_message_idempotent() {
        let mut state = AppState::new("s".into(), "m".into());
        state.ensure_assistant_message();
        assert_eq!(state.messages.len(), 2);

        // Calling again should not create another
        state.ensure_assistant_message();
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn add_user_message_appended() {
        let mut state = AppState::new("s".into(), "m".into());
        state.add_user_message("hello".into());
        assert_eq!(state.messages.len(), 2);
        match &state.messages[1] {
            ChatMessage::User { content, .. } => assert_eq!(content, "hello"),
            other => panic!("Expected User message, got: {:?}", other),
        }
    }

    #[test]
    fn find_tool_mut_returns_correct_tool() {
        let mut state = AppState::new("s".into(), "m".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { tools, .. } = state.current_assistant_mut() {
            tools.push(ToolExecution {
                id: "tool-1".into(),
                name: "bash".into(),
                params: "ls".into(),
                status: ToolStatus::Running,
                duration: None,
                progress: None,
                error: None,
            });
            tools.push(ToolExecution {
                id: "tool-2".into(),
                name: "read".into(),
                params: "file.txt".into(),
                status: ToolStatus::Running,
                duration: None,
                progress: None,
                error: None,
            });
        }

        let tool = state.find_tool_mut("tool-2");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name, "read");

        let missing = state.find_tool_mut("tool-999");
        assert!(missing.is_none());
    }

    #[test]
    fn open_command_palette_sets_focus() {
        let mut state = AppState::new("s".into(), "m".into());
        state.open_command_palette();
        assert_eq!(state.focus, Focus::CommandPalette);
        assert!(state.palette.is_some());

        let palette = state.palette.as_ref().unwrap();
        assert!(palette.input.is_empty());
        assert!(!palette.filtered.is_empty());
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn close_overlay_resets_focus() {
        let mut state = AppState::new("s".into(), "m".into());
        state.open_command_palette();
        assert_eq!(state.focus, Focus::CommandPalette);

        state.close_overlay();
        assert_eq!(state.focus, Focus::Input);
        assert!(state.palette.is_none());
        assert!(state.dialog.is_none());
    }

    #[test]
    fn show_dialog_sets_focus() {
        let mut state = AppState::new("s".into(), "m".into());
        state.show_dialog(
            "run-1".into(),
            "Approve?".into(),
            vec!["Yes".into(), "No".into()],
        );
        assert_eq!(state.focus, Focus::Dialog);
        let dialog = state.dialog.as_ref().unwrap();
        assert_eq!(dialog.run_id, "run-1");
        assert_eq!(dialog.question, "Approve?");
        assert_eq!(dialog.options.len(), 2);
        assert_eq!(dialog.selected, 0);
    }

    #[test]
    fn switch_session_clears_messages() {
        let mut state = AppState::new("s1".into(), "m".into());
        state.add_user_message("hello".into());
        assert_eq!(state.messages.len(), 2);

        state.switch_session("s2".into());
        assert_eq!(state.session_key, "s2");
        // Should have 1 message: the switch notification
        assert_eq!(state.messages.len(), 1);
        match &state.messages[0] {
            ChatMessage::System { content } => assert!(content.contains("s2")),
            other => panic!("Expected System message, got: {:?}", other),
        }
    }

    #[test]
    fn clear_screen_keeps_session() {
        let mut state = AppState::new("s1".into(), "m".into());
        state.add_user_message("hello".into());
        state.total_tokens = 500;

        state.clear_screen();
        assert_eq!(state.session_key, "s1");
        assert_eq!(state.total_tokens, 500);
        assert_eq!(state.messages.len(), 1);
        match &state.messages[0] {
            ChatMessage::System { content } => assert!(content.contains("cleared")),
            other => panic!("Expected System message, got: {:?}", other),
        }
    }

    #[test]
    fn update_token_usage_accumulates() {
        let mut state = AppState::new("s".into(), "m".into());
        let summary = RunSummary {
            total_tokens: 100,
            tool_calls: 2,
            loops: 1,
            final_response: None,
        };
        state.update_token_usage(&summary);
        assert_eq!(state.total_tokens, 100);

        state.update_token_usage(&summary);
        assert_eq!(state.total_tokens, 200);
    }

    #[test]
    fn request_quit_sets_flag() {
        let mut state = AppState::new("s".into(), "m".into());
        assert!(!state.should_quit);
        state.request_quit();
        assert!(state.should_quit);
    }

    #[test]
    fn handle_run_accepted() {
        let mut state = AppState::new("s".into(), "m".into());
        let event = StreamEvent::RunAccepted {
            run_id: "run-1".into(),
            session_key: "s".into(),
            accepted_at: "2026-03-04T00:00:00Z".into(),
        };
        let action = state.handle_gateway_event(event);
        assert!(matches!(action, Action::None));
        assert_eq!(state.current_run, Some("run-1".into()));
        assert!(state.is_connected);
    }

    #[test]
    fn handle_response_chunk_appends_content() {
        let mut state = AppState::new("s".into(), "m".into());

        let chunk1 = StreamEvent::ResponseChunk {
            run_id: "run-1".into(),
            seq: 1,
            content: "Hello".into(),
            chunk_index: 0,
            is_final: false,
        };
        state.handle_gateway_event(chunk1);

        let chunk2 = StreamEvent::ResponseChunk {
            run_id: "run-1".into(),
            seq: 2,
            content: " World".into(),
            chunk_index: 1,
            is_final: false,
        };
        state.handle_gateway_event(chunk2);

        // Should have: system welcome + assistant message
        assert_eq!(state.messages.len(), 2);
        match &state.messages[1] {
            ChatMessage::Assistant { content, .. } => {
                assert_eq!(content, "Hello World");
            }
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
    }

    #[test]
    fn handle_tool_lifecycle() {
        let mut state = AppState::new("s".into(), "m".into());

        // Tool start
        let start = StreamEvent::ToolStart {
            run_id: "run-1".into(),
            seq: 1,
            tool_name: "bash".into(),
            tool_id: "t1".into(),
            params: serde_json::json!({"command": "ls"}),
        };
        state.handle_gateway_event(start);

        // Tool update
        let update = StreamEvent::ToolUpdate {
            run_id: "run-1".into(),
            seq: 2,
            tool_id: "t1".into(),
            progress: "running...".into(),
        };
        state.handle_gateway_event(update);

        {
            let tool = state.find_tool_mut("t1").unwrap();
            assert_eq!(tool.status, ToolStatus::Running);
            assert_eq!(tool.progress, Some("running...".into()));
        }

        // Tool end
        let end = StreamEvent::ToolEnd {
            run_id: "run-1".into(),
            seq: 3,
            tool_id: "t1".into(),
            result: aleph_protocol::ToolResult::success("output"),
            duration_ms: 150,
        };
        state.handle_gateway_event(end);

        let tool = state.find_tool_mut("t1").unwrap();
        assert_eq!(tool.status, ToolStatus::Success);
        assert_eq!(tool.duration, Some(Duration::from_millis(150)));
        assert!(tool.progress.is_none()); // cleared on end
    }

    #[test]
    fn handle_run_complete_clears_run() {
        let mut state = AppState::new("s".into(), "m".into());
        state.current_run = Some("run-1".into());

        // Create an assistant message that's streaming
        state.ensure_assistant_message();

        let event = StreamEvent::RunComplete {
            run_id: "run-1".into(),
            seq: 10,
            summary: RunSummary {
                total_tokens: 500,
                tool_calls: 3,
                loops: 2,
                final_response: Some("Done".into()),
            },
            total_duration_ms: 5000,
        };
        state.handle_gateway_event(event);

        assert!(state.current_run.is_none());
        assert_eq!(state.total_tokens, 500);
        assert_eq!(state.last_run_duration, Some(Duration::from_millis(5000)));

        // Assistant message should no longer be streaming
        match &state.messages.last().unwrap() {
            ChatMessage::Assistant { is_streaming, .. } => assert!(!is_streaming),
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
    }

    #[test]
    fn handle_run_error_adds_system_message() {
        let mut state = AppState::new("s".into(), "m".into());
        state.current_run = Some("run-1".into());

        let event = StreamEvent::RunError {
            run_id: "run-1".into(),
            seq: 5,
            error: "something went wrong".into(),
            error_code: Some("E001".into()),
        };
        state.handle_gateway_event(event);

        assert!(state.current_run.is_none());
        // Last message should be the error system message
        match state.messages.last().unwrap() {
            ChatMessage::System { content } => {
                assert!(content.contains("something went wrong"));
            }
            other => panic!("Expected System message, got: {:?}", other),
        }
    }

    #[test]
    fn handle_ask_user_shows_dialog() {
        let mut state = AppState::new("s".into(), "m".into());
        let event = StreamEvent::AskUser {
            run_id: "run-1".into(),
            seq: 3,
            question: "Allow file write?".into(),
            options: vec!["Allow".into(), "Deny".into()],
        };
        state.handle_gateway_event(event);

        assert_eq!(state.focus, Focus::Dialog);
        let dialog = state.dialog.as_ref().unwrap();
        assert_eq!(dialog.run_id, "run-1");
        assert_eq!(dialog.question, "Allow file write?");
    }

    #[test]
    fn handle_reasoning_appends() {
        let mut state = AppState::new("s".into(), "m".into());

        let event1 = StreamEvent::Reasoning {
            run_id: "run-1".into(),
            seq: 1,
            content: "Let me think".into(),
            is_complete: false,
        };
        state.handle_gateway_event(event1);

        let event2 = StreamEvent::Reasoning {
            run_id: "run-1".into(),
            seq: 2,
            content: " about this...".into(),
            is_complete: true,
        };
        state.handle_gateway_event(event2);

        match &state.messages[1] {
            ChatMessage::Assistant { reasoning, .. } => {
                assert_eq!(reasoning.as_deref(), Some("Let me think about this..."));
            }
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
    }
}
