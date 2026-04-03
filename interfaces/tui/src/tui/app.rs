// Core application state management for the TUI.
//
// Contains all state types (AppState, ChatMessage, Action, Focus, etc.)
// and the gateway event handler that maps StreamEvent -> state mutations.

use std::time::Duration;

use aleph_protocol::{
    present_agent_trace_event_with_preset, summarize_tool_input, AgentTraceEvent,
    AgentTracePresentation, AgentTracePresentationPreset, AgentTraceReplay, AgentTraceTextKind,
    AgentTraceToolResult, RunSummary, StreamEvent,
};
use chrono::{DateTime, Utc};

use super::command_tree::{CommandEntry, DisplayEntry};
use super::slash::LocalCommand;

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
    pub filtered: Vec<DisplayEntry>,
    pub selected: usize,
    /// Stack of namespace names we have browsed into (e.g. ["session"])
    pub namespace_stack: Vec<String>,
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
    pub current_run_uses_agent_trace: bool,
    pub current_run_trace_summary_applied: bool,

    // -- Settings --
    pub verbose: bool,

    // -- Gateway commands (fetched at startup, tree-structured) --
    pub gateway_commands: Vec<CommandEntry>,

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
            current_run_uses_agent_trace: false,
            current_run_trace_summary_applied: false,

            verbose: false,
            gateway_commands: Vec::new(),

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
        self.messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m, ChatMessage::Assistant { .. }))
            .expect("ensure_assistant_message guarantees this exists")
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
                    full_command: format!("{} ", n),
                })
                .collect();
            entries.extend(CommandEntry::root_display_entries(&self.gateway_commands));
            entries
        } else {
            // Inside a namespace: drill down through the stack
            let mut current_entries = &self.gateway_commands;
            let mut found_ns: Option<&CommandEntry> = None;

            for ns_name in namespace_stack {
                found_ns = current_entries
                    .iter()
                    .find(|e| e.is_namespace && e.name.eq_ignore_ascii_case(ns_name));
                if let Some(ns) = found_ns {
                    current_entries = &ns.children;
                } else {
                    return Vec::new();
                }
            }

            if let Some(ns) = found_ns {
                let path = namespace_stack.join(" ");
                CommandEntry::namespace_display_entries(ns, &path)
            } else {
                Vec::new()
            }
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
        all.into_iter()
            .filter(|e| {
                e.label.to_lowercase().contains(&filter_lower)
                    || e.hint.to_lowercase().contains(&filter_lower)
            })
            .collect()
    }

    /// Open the command palette, pre-populated with root-level commands.
    pub fn open_command_palette(&mut self) {
        let all = self.palette_display_entries(&[]);
        self.palette = Some(PaletteState {
            input: String::new(),
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
            let palette = match &self.palette {
                Some(p) => p,
                None => return,
            };
            let mut stack = palette.namespace_stack.clone();
            stack.push(ns_name.to_string());
            stack
        };
        let entries = self.palette_display_entries(&new_stack);
        if let Some(palette) = &mut self.palette {
            palette.namespace_stack = new_stack;
            palette.input.clear();
            palette.selected = 0;
            palette.filtered = entries;
        }
    }

    /// Go back one level in the palette namespace stack.
    /// Returns true if we went back, false if already at root.
    pub fn palette_go_back(&mut self) -> bool {
        // Build the new stack, then compute entries without holding a mutable borrow
        let new_stack = {
            let palette = match &self.palette {
                Some(p) => p,
                None => return false,
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

    /// Switch to a different session and reset transient chat/run UI state.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn switch_session(&mut self, session_key: String) {
        self.session_key = session_key.clone();
        self.messages.clear();
        self.current_run = None;
        self.current_run_uses_agent_trace = false;
        self.current_run_trace_summary_applied = false;
        self.dialog = None;
        self.palette = None;
        self.focus = Focus::Input;
        self.scroll_to_bottom();
        self.add_system_message(format!("Switched to session {}", session_key));
    }

    // -- Settings -------------------------------------------------------

    /// Toggle verbose/debug output mode.
    pub fn toggle_verbose(&mut self) {
        self.verbose = !self.verbose;
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

    fn append_reasoning_entry(&mut self, content: String) {
        self.ensure_assistant_message();
        if let ChatMessage::Assistant { reasoning, .. } = self.current_assistant_mut() {
            match reasoning {
                Some(existing) if !existing.is_empty() => {
                    existing.push('\n');
                    existing.push_str(&content);
                }
                Some(existing) => existing.push_str(&content),
                None => *reasoning = Some(content),
            }
        }
    }

    fn append_assistant_content(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }

        self.ensure_assistant_message();
        if let ChatMessage::Assistant {
            content: msg_content,
            ..
        } = self.current_assistant_mut()
        {
            msg_content.push_str(content);
        }
    }

    fn start_tool_execution(&mut self, tool_id: String, tool_name: String, params: String) {
        self.ensure_assistant_message();
        if let Some(tool) = self.find_tool_mut(&tool_id) {
            tool.name = tool_name;
            tool.params = params;
            tool.status = ToolStatus::Running;
            tool.duration = None;
            tool.progress = None;
            tool.error = None;
            return;
        }

        if let ChatMessage::Assistant { tools, .. } = self.current_assistant_mut() {
            tools.push(ToolExecution {
                id: tool_id,
                name: tool_name,
                params,
                status: ToolStatus::Running,
                duration: None,
                progress: None,
                error: None,
            });
        }
    }

    fn finish_tool_execution(
        &mut self,
        tool_id: &str,
        result: &AgentTraceToolResult,
        duration_ms: u64,
    ) {
        if let Some(tool) = self.find_tool_mut(tool_id) {
            tool.status = if result.is_success() {
                ToolStatus::Success
            } else {
                ToolStatus::Failed
            };
            tool.duration = Some(Duration::from_millis(duration_ms));
            tool.error = result.error_text().map(ToOwned::to_owned);
            tool.progress = None;
        }
    }

    fn mark_current_assistant_complete(&mut self) {
        if let Some(ChatMessage::Assistant { is_streaming, .. }) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m, ChatMessage::Assistant { .. }))
        {
            *is_streaming = false;
        }
    }

    fn update_total_tokens_from_trace(&mut self, total_tokens: usize) {
        let bounded = total_tokens.min(u64::MAX as usize) as u64;
        self.total_tokens = self.total_tokens.saturating_add(bounded);
    }

    fn default_trace_presentation(event: &AgentTraceEvent) -> Option<AgentTracePresentation> {
        present_agent_trace_event_with_preset(event, AgentTracePresentationPreset::TuiDebug)
    }

    fn append_trace_debug_entry(
        &mut self,
        event: &AgentTraceEvent,
        presentation: &AgentTracePresentation,
    ) {
        match event {
            AgentTraceEvent::TextEmitted { stream, .. } => match stream {
                AgentTraceTextKind::Intermediate => {
                    self.append_reasoning_entry(presentation.content.clone())
                }
                AgentTraceTextKind::Final => self.append_assistant_content(&presentation.content),
            },
            AgentTraceEvent::ToolSummary { .. }
            | AgentTraceEvent::TurnStarted { .. }
            | AgentTraceEvent::TurnStateEntered { .. }
            | AgentTraceEvent::TurnCompleted { .. }
            | AgentTraceEvent::SessionCompleted { .. } => {
                self.append_reasoning_entry(presentation.content.clone())
            }
            AgentTraceEvent::ToolCallStarted { .. } | AgentTraceEvent::ToolCallCompleted { .. } => {
            }
        }
    }

    fn apply_agent_trace_event(&mut self, event: &AgentTraceEvent) -> Action {
        let presentation = Self::default_trace_presentation(event);
        if let Some(presentation) = &presentation {
            self.append_trace_debug_entry(event, presentation);
        }

        match event {
            AgentTraceEvent::TextEmitted { .. } => Action::ScrollToBottomIfAutoScroll,
            AgentTraceEvent::ToolCallStarted { call, .. } => {
                self.start_tool_execution(
                    call.tool_id.clone(),
                    call.tool_name.clone(),
                    summarize_tool_input(
                        &call.input,
                        AgentTracePresentationPreset::TuiDebug.options(),
                    ),
                );
                Action::ScrollToBottomIfAutoScroll
            }
            AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                self.finish_tool_execution(&call.tool_id, result, call.duration_ms);
                Action::ScrollToBottomIfAutoScroll
            }
            AgentTraceEvent::ToolSummary { summary, .. } => {
                let _ = summary;
                Action::ScrollToBottomIfAutoScroll
            }
            AgentTraceEvent::SessionCompleted {
                total_tokens,
                final_text,
                ..
            } => {
                if let Some(text) = final_text {
                    let needs_final_text = !matches!(
                        self.messages.iter().rev().find_map(|msg| match msg {
                            ChatMessage::Assistant { content, .. } => Some(!content.is_empty()),
                            _ => None,
                        }),
                        Some(true)
                    );

                    if needs_final_text {
                        self.append_assistant_content(text);
                    }
                }

                if !self.current_run_trace_summary_applied {
                    self.update_total_tokens_from_trace(*total_tokens);
                    self.current_run_trace_summary_applied = true;
                }
                self.current_run = None;
                self.current_run_uses_agent_trace = false;
                self.mark_current_assistant_complete();
                Action::ScrollToBottomIfAutoScroll
            }
            _ => Action::None,
        }
    }

    pub fn load_trace_replay(&mut self, replay: AgentTraceReplay) {
        let summary = format!(
            "Loaded replay {} from session {} [{}] via {}.",
            replay.task.task_id, replay.task.session_id, replay.task.status, replay.task.agent_id
        );

        self.messages.clear();
        self.current_run = Some(replay.task.task_id.clone());
        self.current_run_uses_agent_trace = true;
        self.dialog = None;
        self.palette = None;
        self.focus = Focus::Input;
        self.scroll_to_bottom();
        self.add_system_message(summary);

        for trace in &replay.traces {
            let _ = self.apply_agent_trace_event(&trace.event);
        }

        if replay.traces.is_empty() {
            self.add_system_message("Replay has no structured trace events.".to_string());
        }

        self.current_run = None;
        self.current_run_uses_agent_trace = false;
        self.current_run_trace_summary_applied = false;
        self.mark_current_assistant_complete();
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
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.is_connected = true;
                Action::None
            }

            StreamEvent::AgentTrace { event, .. } => {
                self.current_run_uses_agent_trace = true;
                self.apply_agent_trace_event(&event)
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
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                self.start_tool_execution(
                    tool_id,
                    tool_name,
                    summarize_tool_input(&params, AgentTracePresentationPreset::TuiDebug.options()),
                );
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
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                let result = if result.success {
                    AgentTraceToolResult::Success {
                        output: serde_json::json!(result.output),
                    }
                } else {
                    AgentTraceToolResult::Error {
                        error: result.error.unwrap_or_default(),
                        retryable: false,
                    }
                };
                self.finish_tool_execution(&tool_id, &result, duration_ms);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ResponseChunk { content, .. } => {
                self.append_assistant_content(&content);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunComplete {
                summary,
                total_duration_ms,
                ..
            } => {
                self.current_run = None;
                self.last_run_duration = Some(Duration::from_millis(total_duration_ms));
                if !self.current_run_trace_summary_applied {
                    self.update_token_usage(&summary);
                }
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.mark_current_assistant_complete();

                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunError { error, .. } => {
                self.current_run = None;
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.mark_current_assistant_complete();

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
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                // Treated same as Reasoning — append to reasoning buffer
                self.append_reasoning_entry(content);
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::{AgentTraceSessionOutcome, AgentTraceTextKind};
    use serde_json::Value;

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
        assert_eq!(
            aleph_protocol::summarize_tool_input(
                &val,
                aleph_protocol::AgentTracePresentationPreset::TuiDebug.options()
            ),
            "hello world"
        );
    }

    #[test]
    fn format_params_string_truncation() {
        let long = "a".repeat(100);
        let result = aleph_protocol::summarize_tool_input(
            &Value::String(long),
            aleph_protocol::AgentTracePresentationPreset::TuiDebug.options(),
        );
        assert!(result.len() < 100);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn format_params_object() {
        let val = serde_json::json!({
            "command": "ls -la",
            "count": 42,
        });
        let result = aleph_protocol::summarize_tool_input(
            &val,
            aleph_protocol::AgentTracePresentationPreset::TuiDebug.options(),
        );
        assert!(result.contains("command="));
        assert!(result.contains("ls -la"));
        assert!(result.contains("count="));
        assert!(result.contains("42"));
    }

    #[test]
    fn format_params_null() {
        assert_eq!(
            aleph_protocol::summarize_tool_input(
                &Value::Null,
                aleph_protocol::AgentTracePresentationPreset::TuiDebug.options()
            ),
            ""
        );
    }

    #[test]
    fn format_params_array() {
        let val = serde_json::json!([1, 2, 3]);
        assert_eq!(
            aleph_protocol::summarize_tool_input(
                &val,
                aleph_protocol::AgentTracePresentationPreset::TuiDebug.options()
            ),
            "[3 items]"
        );
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
            is_intermediate: false,
        };
        state.handle_gateway_event(chunk1);

        let chunk2 = StreamEvent::ResponseChunk {
            run_id: "run-1".into(),
            seq: 2,
            content: " World".into(),
            chunk_index: 1,
            is_final: false,
            is_intermediate: false,
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
    fn handle_agent_trace_text_events_populate_assistant_content_and_reasoning() {
        let mut state = AppState::new("s".into(), "m".into());

        state.handle_gateway_event(StreamEvent::RunAccepted {
            run_id: "run-1".into(),
            session_key: "s".into(),
            accepted_at: "2026-03-04T00:00:00Z".into(),
        });

        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 1,
            event: AgentTraceEvent::TextEmitted {
                iteration: 1,
                stream: AgentTraceTextKind::Intermediate,
                text: "Inspecting replay trace".into(),
            },
        });
        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 2,
            event: AgentTraceEvent::TextEmitted {
                iteration: 1,
                stream: AgentTraceTextKind::Final,
                text: "Replay loaded".into(),
            },
        });

        match &state.messages[1] {
            ChatMessage::Assistant {
                content, reasoning, ..
            } => {
                assert_eq!(content, "Replay loaded");
                assert_eq!(reasoning.as_deref(), Some("Inspecting replay trace"));
            }
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
    }

    #[test]
    fn handle_agent_trace_session_completed_updates_totals_and_closes_stream() {
        let mut state = AppState::new("s".into(), "m".into());

        state.handle_gateway_event(StreamEvent::RunAccepted {
            run_id: "run-1".into(),
            session_key: "s".into(),
            accepted_at: "2026-03-04T00:00:00Z".into(),
        });
        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 1,
            event: AgentTraceEvent::TextEmitted {
                iteration: 1,
                stream: AgentTraceTextKind::Final,
                text: "Replay loaded".into(),
            },
        });

        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 2,
            event: AgentTraceEvent::SessionCompleted {
                outcome: AgentTraceSessionOutcome::Completed,
                iterations: 1,
                tool_calls_made: 0,
                total_tokens: 321,
                hit_limit: false,
                final_text: Some("Replay loaded".into()),
            },
        });

        assert_eq!(state.total_tokens, 321);
        assert!(state.current_run.is_none());
        assert!(!state.current_run_uses_agent_trace);
        match &state.messages[1] {
            ChatMessage::Assistant { is_streaming, .. } => assert!(!is_streaming),
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
    }

    #[test]
    fn handle_agent_trace_decision_events_append_shared_projection_reasoning() {
        let mut state = AppState::new("s".into(), "m".into());

        state.handle_gateway_event(StreamEvent::RunAccepted {
            run_id: "run-1".into(),
            session_key: "s".into(),
            accepted_at: "2026-03-04T00:00:00Z".into(),
        });

        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 1,
            event: AgentTraceEvent::TurnStarted { iteration: 1 },
        });
        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 2,
            event: AgentTraceEvent::TurnStateEntered {
                iteration: 1,
                state: aleph_protocol::AgentTraceState::Think,
            },
        });
        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 3,
            event: AgentTraceEvent::TurnCompleted {
                iteration: 1,
                outcome: aleph_protocol::AgentTraceTurnOutcome::Continue,
                metrics: aleph_protocol::AgentTraceTurnMetrics {
                    requested_tool_calls: 1,
                    executed_tool_calls: 1,
                    productive: true,
                    consecutive_errors: 0,
                    total_tokens: 64,
                },
            },
        });

        match &state.messages[1] {
            ChatMessage::Assistant { reasoning, .. } => {
                assert_eq!(
                    reasoning.as_deref(),
                    Some(
                        "Turn started #1\nState #1: think\nTurn completed #1 (continue, 1 requested, 1 executed, 64 tokens)"
                    )
                );
            }
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
    }

    #[test]
    fn load_trace_replay_records_session_summary_in_reasoning() {
        let mut state = AppState::new("s".into(), "m".into());

        state.load_trace_replay(AgentTraceReplay {
            task: aleph_protocol::AgentTraceTaskSummary {
                task_id: "task-1".into(),
                session_id: "session-1".into(),
                agent_id: "agent-1".into(),
                status: "completed".into(),
                prompt_preview: "Inspect replay".into(),
                created_at: 10,
                updated_at: 20,
                started_at: Some(11),
                completed_at: Some(19),
                trace_count: 2,
                last_event_kind: Some("session_completed".into()),
            },
            traces: vec![
                aleph_protocol::AgentTraceRecord {
                    step_index: 0,
                    timestamp: 11,
                    event: AgentTraceEvent::TurnStarted { iteration: 1 },
                },
                aleph_protocol::AgentTraceRecord {
                    step_index: 1,
                    timestamp: 19,
                    event: AgentTraceEvent::SessionCompleted {
                        outcome: AgentTraceSessionOutcome::Completed,
                        iterations: 1,
                        tool_calls_made: 0,
                        total_tokens: 33,
                        hit_limit: false,
                        final_text: Some("done".into()),
                    },
                },
            ],
        });

        match &state.messages[1] {
            ChatMessage::Assistant {
                content, reasoning, ..
            } => {
                assert_eq!(content, "done");
                assert_eq!(
                    reasoning.as_deref(),
                    Some("Turn started #1\nSession completed (completed, 1 iterations, 0 tool calls, 33 tokens)")
                );
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
    fn handle_agent_trace_tool_lifecycle_takes_precedence() {
        let mut state = AppState::new("s".into(), "m".into());

        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 1,
            event: aleph_protocol::AgentTraceEvent::ToolCallStarted {
                iteration: 1,
                call: aleph_protocol::AgentTraceToolCallStart {
                    tool_id: "t1".into(),
                    tool_name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                },
            },
        });

        state.handle_gateway_event(StreamEvent::ToolStart {
            run_id: "run-1".into(),
            seq: 2,
            tool_name: "bash".into(),
            tool_id: "t1".into(),
            params: serde_json::json!({"command": "ls"}),
        });

        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 3,
            event: aleph_protocol::AgentTraceEvent::ToolSummary {
                iteration: 1,
                summary: "Listed the current directory".into(),
            },
        });

        state.handle_gateway_event(StreamEvent::ReasoningBlock {
            run_id: "run-1".into(),
            seq: 4,
            step_type: aleph_protocol::ReasoningStepType::Observation,
            label: "Tool Summary".into(),
            content: "legacy summary".into(),
            confidence: None,
            is_final: false,
        });

        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 5,
            event: aleph_protocol::AgentTraceEvent::ToolCallCompleted {
                iteration: 1,
                call: aleph_protocol::AgentTraceToolCallEnd {
                    tool_id: "t1".into(),
                    tool_name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                    duration_ms: 120,
                },
                result: aleph_protocol::AgentTraceToolResult::Success {
                    output: serde_json::json!({"ok": true}),
                },
            },
        });

        match &state.messages[1] {
            ChatMessage::Assistant {
                tools, reasoning, ..
            } => {
                assert_eq!(tools.len(), 1);
                assert_eq!(tools[0].id, "t1");
                assert_eq!(tools[0].status, ToolStatus::Success);
                assert_eq!(tools[0].duration, Some(Duration::from_millis(120)));
                assert_eq!(reasoning.as_deref(), Some("Listed the current directory"));
            }
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
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
    fn run_complete_does_not_double_count_after_agent_trace_session_completed() {
        let mut state = AppState::new("s".into(), "m".into());

        state.handle_gateway_event(StreamEvent::RunAccepted {
            run_id: "run-1".into(),
            session_key: "s".into(),
            accepted_at: "2026-03-04T00:00:00Z".into(),
        });
        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: 1,
            event: AgentTraceEvent::SessionCompleted {
                outcome: AgentTraceSessionOutcome::Completed,
                iterations: 1,
                tool_calls_made: 0,
                total_tokens: 321,
                hit_limit: false,
                final_text: Some("done".into()),
            },
        });
        state.handle_gateway_event(StreamEvent::RunComplete {
            run_id: "run-1".into(),
            seq: 2,
            summary: RunSummary {
                total_tokens: 321,
                tool_calls: 0,
                loops: 1,
                final_response: Some("done".into()),
            },
            total_duration_ms: 1500,
        });

        assert_eq!(state.total_tokens, 321);
        assert!(!state.current_run_trace_summary_applied);
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
