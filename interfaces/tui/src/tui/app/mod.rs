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

use aleph_protocol::RunSummary;
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
    /// Answer an `AskUser` dialog. Routed to `clarification.resolve` keyed by
    /// `session_key` (the reply routes by session, not by run).
    RespondToDialog { session_key: String, reply: String },

    // -- Tool approval (Ask exec tier) --
    /// Resolve the pending tool-approval overlay by option index into
    /// `APPROVAL_DECISIONS` (0 = allow once, 1 = allow session, 2 = deny).
    ResolveApproval { index: usize },

    // -- Session picker --
    /// Move session-picker selection up
    SessionPickerUp,
    /// Move session-picker selection down
    SessionPickerDown,
    /// Confirm the selected session and switch to it
    SessionPickerConfirm,
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
    Approval,
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
    /// Clarification registry key — the answer is posted back to
    /// `clarification.resolve` against this, NOT the run id (replies route by
    /// session). Carried on the `AskUser` frame.
    pub session_key: String,
    pub question: String,
    pub options: Vec<String>,
    pub selected: usize,
}

/// Decision options for the tool-approval overlay, in display order. Each is
/// `(label, wire_decision)`; the wire value is exactly what
/// `exec.approval.resolve` expects (kebab-case, server-validated).
pub const APPROVAL_DECISIONS: [(&str, &str); 3] = [
    ("Allow once", "allow-once"),
    ("Allow for session", "allow-session"),
    ("Deny", "deny"),
];

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
    /// Highlighted decision index into [`APPROVAL_DECISIONS`].
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
    /// Live context-window occupancy `(used_tokens, window_tokens)` from the
    /// latest `ContextGauge` event. `None` until the session's first gauge
    /// arrives. The pair always travels together (one event), so a single
    /// `Option` keeps the half-known state unrepresentable; the denominator is
    /// server-authoritative per model.
    pub context_gauge: Option<(u32, u32)>,
    /// Last-call prompt-cache efficiency `(cache_read, denominator)` from the
    /// latest `ProviderUsage` trace event that reported cache activity, where
    /// `denominator = input + cache_creation + cache_read`. `None` until a
    /// call reports cache tokens — providers without prompt caching never
    /// surface a misleading 0%. Last-call (not cumulative) on purpose: a
    /// sudden drop is what tells you a prefix bust just happened.
    pub cache_stat: Option<(u64, u64)>,
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

    // -- Run tracking --
    pub current_run: Option<String>,
    /// Wall-clock start of the active run (set on `RunAccepted`, cleared on any
    /// run-end). Drives the status-bar working indicator's elapsed timer.
    pub run_started_at: Option<Instant>,
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
    pub session_picker: Option<SessionPickerState>,
    /// Pending tool-approval overlay (Ask exec tier), if one is being shown.
    /// Surfaced by the `exec.approvals.pending` poll, resolved via
    /// `exec.approval.resolve`.
    pub approval: Option<ApprovalState>,

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
            context_gauge: None,
            cache_stat: None,
            cache_stat_agent: None,
            cache_root_agent: None,
            is_connected: true,

            current_run: None,
            run_started_at: None,
            last_run_duration: None,
            current_run_uses_agent_trace: false,
            current_run_trace_summary_applied: false,

            verbose: false,
            tool_progress_mode: ToolProgressMode::default(),
            gateway_commands: Vec::new(),

            focus: Focus::Input,
            dialog: None,
            palette: None,
            session_picker: None,
            approval: None,

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

    /// Close any open overlay (palette, dialog, or session picker) and return
    /// focus to input.
    pub fn close_overlay(&mut self) {
        self.palette = None;
        self.dialog = None;
        self.session_picker = None;
        self.approval = None;
        self.focus = Focus::Input;
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

    /// Show an `AskUser` dialog. `session_key` is the clarification key the
    /// answer resolves against (`clarification.resolve`).
    pub fn show_dialog(&mut self, session_key: String, question: String, options: Vec<String>) {
        self.dialog = Some(DialogState {
            session_key,
            question,
            options,
            selected: 0,
        });
        self.focus = Focus::Dialog;
    }

    /// Surface the tool-approval overlay for a pending, session-owned approval
    /// and steal focus so the parked run gets a decision. The caller
    /// (`commands::poll_approvals`) has already confirmed the `id` belongs to
    /// this session.
    pub fn open_approval(&mut self, id: String, command: String, reason: Option<String>) {
        self.approval = Some(ApprovalState {
            id,
            command,
            reason,
            selected: 0,
        });
        self.focus = Focus::Approval;
    }

    /// Retract the approval overlay (resolved here, resolved elsewhere, or the
    /// server-side approval expired) and return focus to input.
    pub fn close_approval(&mut self) {
        self.approval = None;
        self.focus = Focus::Input;
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

    /// Switch to a different session and reset transient chat/run UI state.
    /// The caller then appends the fetched `chat.history` transcript.
    pub fn switch_session(&mut self, session_key: &str) {
        self.session_key = session_key.to_string();
        self.messages.clear();
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
