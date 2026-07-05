// Core application state management for the TUI.
//
// Contains all state types (AppState, ChatMessage, Action, Focus, etc.)
// and the gateway event handler that maps StreamEvent -> state mutations.

mod events;

#[cfg(test)]
mod tests;

use std::time::Duration;

use aleph_protocol::{
    present_agent_trace_event_with_preset, summarize_tool_input, AgentTraceEvent,
    AgentTracePresentation, AgentTracePresentationPreset, AgentTraceReplay, AgentTraceTextKind,
    AgentTraceToolResult, RunSummary,
};
use chrono::{DateTime, Utc};

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

    // -- Dialog --
    /// Select a dialog option by index
    DialogSelect(usize),

    // -- Dialog response --
    /// Respond to an `AskUser` dialog
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

/// State for the `AskUser` confirmation dialog.
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
    /// Stack of namespace names we have browsed into (e.g. `["session"]`)
    pub namespace_stack: Vec<String>,
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Central application state. Owned by the main loop, mutated through
/// methods that enforce invariants (e.g. `auto_scroll` toggling).
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
    pub tool_progress_mode: ToolProgressMode,

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
    /// Create a new `AppState` with a welcome system message.
    pub fn new(session_key: String, model_name: String) -> Self {
        let welcome = format!(
            "Welcome to Aleph CLI. Session: {session_key} | Model: {model_name}. Type /help for commands.",
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
            tool_progress_mode: ToolProgressMode::default(),
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
        // `ensure_assistant_message` guarantees an assistant message exists,
        // so the search from the end always succeeds. Resolve the index with an
        // immutable borrow first to avoid a double mutable borrow of `messages`.
        let idx = self
            .messages
            .iter()
            .rposition(|m| matches!(m, ChatMessage::Assistant { .. }))
            .expect("ensure_assistant_message guarantees an assistant message exists");
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

    /// Close any open overlay (palette or dialog) and return focus to input.
    pub fn close_overlay(&mut self) {
        self.palette = None;
        self.dialog = None;
        self.focus = Focus::Input;
    }

    /// Show an `AskUser` dialog.
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
    pub fn switch_session(&mut self, session_key: &str) {
        self.session_key = session_key.to_string();
        self.messages.clear();
        self.current_run = None;
        self.current_run_uses_agent_trace = false;
        self.current_run_trace_summary_applied = false;
        self.dialog = None;
        self.palette = None;
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
        let bounded = u64::try_from(total_tokens).unwrap_or(u64::MAX);
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
            // TextEmitted carries the model's verbatim output. Feed the raw
            // text into the user-facing message — the debug presentation would
            // prefix it with "[Final text] iter N:" decoration meant only for
            // a trace/debug panel, not the primary chat content.
            AgentTraceEvent::TextEmitted { stream, text, .. } => match stream {
                AgentTraceTextKind::Intermediate => self.append_reasoning_entry(text.clone()),
                AgentTraceTextKind::Final => self.append_assistant_content(text),
            },
            // ToolSummary carries an agent-authored summary sentence — use it
            // verbatim instead of the "Tool summary: " decorated form.
            AgentTraceEvent::ToolSummary { summary, .. } => {
                self.append_reasoning_entry(summary.clone());
            }
            AgentTraceEvent::TurnStarted { .. }
            | AgentTraceEvent::TurnStateEntered { .. }
            | AgentTraceEvent::TurnCompleted { .. }
            | AgentTraceEvent::SessionCompleted { .. }
            // Goal-loop watchdog veto: surface the interception reason (the
            // presentation renders "checklist incomplete — …") so the user
            // sees why the run was forced to continue.
            | AgentTraceEvent::VerifierVeto { .. } => {
                self.append_reasoning_entry(presentation.content.clone());
            }
            // Tool-call lifecycle is rendered by ToolStart/ToolEnd gateway events;
            // observability passthrough variants have no TUI rendering.
            AgentTraceEvent::ToolCallStarted { .. }
            | AgentTraceEvent::ToolCallCompleted { .. }
            | AgentTraceEvent::WorktreeCreated { .. }
            | AgentTraceEvent::WorktreeCleanedUp { .. }
            | AgentTraceEvent::McpScopeAttached { .. }
            | AgentTraceEvent::McpScopeCleaned { .. }
            | AgentTraceEvent::ProviderUsage { .. }
            | AgentTraceEvent::ReactiveCompactionAttempted { .. }
            // MoA fan-out moments — no TUI rendering yet (panel-only, Task 9).
            | AgentTraceEvent::MoaAdvisor { .. }
            | AgentTraceEvent::MoaAggregating { .. }
            | AgentTraceEvent::MoaAdvisorSpend { .. }
            | AgentTraceEvent::MoaTurnTrace { .. } => {}
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

    pub fn load_trace_replay(&mut self, replay: &AgentTraceReplay) {
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
}
