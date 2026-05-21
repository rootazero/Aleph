# CLI TUI Upgrade Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Upgrade Aleph CLI from basic stdin/stdout to a full ratatui split-screen TUI with chat area, multi-line input, status bar, slash commands, streaming markdown, and tool execution display.

**Architecture:** Event → Action → State → Render loop. Terminal events (crossterm) and Gateway events (WebSocket StreamEvent) merge into a single Action dispatcher. AppState drives immediate-mode ratatui rendering. CLI remains a pure protocol client (0 dependency on core).

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28, tui-textarea 0.7, tokio, aleph-protocol (StreamEvent, ToolResult, RunSummary)

**Design Doc:** `docs/plans/2026-03-04-cli-tui-upgrade-design.md`

---

## Existing Code Reference

| File | Lines | Role |
|------|-------|------|
| `apps/cli/Cargo.toml` | 65 | Dependencies — ratatui/crossterm already present |
| `apps/cli/src/main.rs` | 190 | Entry point, clap routing. Line 165: dispatches to `chat::run()` |
| `apps/cli/src/client.rs` | 340 | `AlephClient` — WebSocket JSON-RPC. Line 52: `connect()`, Line 229: `call()` |
| `apps/cli/src/config.rs` | 130 | `CliConfig` — TOML config at `~/.config/aleph-cli/config.toml` |
| `apps/cli/src/error.rs` | 50 | `CliError` enum, `CliResult<T>` type alias |
| `apps/cli/src/commands/chat.rs` | 160 | Current REPL loop — **will be replaced by TUI launch** |
| `apps/cli/src/ui/mod.rs` | 7 | Placeholder — **will be deleted** |
| `shared/protocol/src/events.rs` | ~270 | `StreamEvent` (12 variants), `ToolResult`, `RunSummary`, `ConfidenceLevel` |

---

## Task 1: Add Dependencies & Create Module Skeleton

**Files:**
- Modify: `apps/cli/Cargo.toml`
- Create: `apps/cli/src/tui/mod.rs`
- Create: `apps/cli/src/tui/theme.rs`
- Delete: `apps/cli/src/ui/mod.rs`
- Modify: `apps/cli/src/main.rs` (replace `mod ui` with `mod tui`)

**Step 1: Add tui-textarea, unicode-width, textwrap to Cargo.toml**

Add after the existing `ratatui = "0.29"` line:

```toml
tui-textarea = "0.7"
unicode-width = "0.2"
textwrap = "0.16"
```

**Step 2: Delete ui/mod.rs, create tui/ module skeleton**

Delete `apps/cli/src/ui/mod.rs`.

Create `apps/cli/src/tui/mod.rs`:
```rust
mod app;
mod event;
mod markdown;
mod render;
mod slash;
mod theme;
mod widgets;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use tokio::sync::mpsc;
use aleph_protocol::StreamEvent;

/// Entry point: run the TUI application
pub async fn run(
    client: AlephClient,
    events: mpsc::Receiver<StreamEvent>,
    config: &CliConfig,
    session_key: String,
) -> CliResult<()> {
    todo!("Task 3 implements this")
}
```

Create `apps/cli/src/tui/theme.rs`:
```rust
use ratatui::style::Color;

pub struct Theme {
    pub user: Color,
    pub assistant: Color,
    pub system: Color,
    pub tool_running: Color,
    pub tool_success: Color,
    pub tool_failed: Color,
    pub tool_name: Color,
    pub tool_param: Color,
    pub tool_duration: Color,
    pub code_bg: Color,
    pub code_block_border: Color,
    pub heading: Color,
    pub link: Color,
    pub quote: Color,
    pub border: Color,
    pub border_focused: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub connected: Color,
    pub disconnected: Color,
    pub primary: Color,
    pub muted: Color,
    pub reasoning: Color,
    pub error: Color,
    pub warning: Color,
}

pub const DEFAULT_THEME: Theme = Theme {
    user: Color::Blue,
    assistant: Color::Green,
    system: Color::Yellow,
    tool_running: Color::Yellow,
    tool_success: Color::Green,
    tool_failed: Color::Red,
    tool_name: Color::Cyan,
    tool_param: Color::DarkGray,
    tool_duration: Color::DarkGray,
    code_bg: Color::DarkGray,
    code_block_border: Color::Gray,
    heading: Color::White,
    link: Color::Blue,
    quote: Color::DarkGray,
    border: Color::Gray,
    border_focused: Color::White,
    status_bg: Color::DarkGray,
    status_fg: Color::White,
    connected: Color::Green,
    disconnected: Color::Red,
    primary: Color::White,
    muted: Color::DarkGray,
    reasoning: Color::DarkGray,
    error: Color::Red,
    warning: Color::Yellow,
};
```

**Step 3: Update main.rs module declaration**

In `apps/cli/src/main.rs`, replace `mod ui;` with `mod tui;`.

**Step 4: Verify it compiles**

Run: `cargo check -p aleph-cli`
Expected: Compiles with warnings about unused code and `todo!()` macros.

**Step 5: Commit**

```bash
git add apps/cli/
git commit -m "cli: add TUI module skeleton with theme and new dependencies"
```

---

## Task 2: Slash Command Parser

**Files:**
- Create: `apps/cli/src/tui/slash.rs`

**Step 1: Write tests for slash command parsing**

At the bottom of `apps/cli/src/tui/slash.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_without_name() {
        let cmd = SlashCommand::parse("/new").unwrap().unwrap();
        assert!(matches!(cmd, SlashCommand::New { name: None }));
    }

    #[test]
    fn parse_new_with_name() {
        let cmd = SlashCommand::parse("/new my-session").unwrap().unwrap();
        assert!(matches!(cmd, SlashCommand::New { name: Some(n) } if n == "my-session"));
    }

    #[test]
    fn parse_session_missing_arg() {
        let result = SlashCommand::parse("/session").unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn parse_model_with_name() {
        let cmd = SlashCommand::parse("/model claude-opus").unwrap().unwrap();
        assert!(matches!(cmd, SlashCommand::Model { name } if name == "claude-opus"));
    }

    #[test]
    fn parse_think_levels() {
        for (input, expected) in [
            ("/think off", ThinkingLevel::Off),
            ("/think low", ThinkingLevel::Low),
            ("/think medium", ThinkingLevel::Medium),
            ("/think high", ThinkingLevel::High),
        ] {
            let cmd = SlashCommand::parse(input).unwrap().unwrap();
            assert!(matches!(cmd, SlashCommand::Think { level } if level == expected));
        }
    }

    #[test]
    fn parse_think_invalid_level() {
        let result = SlashCommand::parse("/think extreme").unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn parse_not_a_slash_command() {
        assert!(SlashCommand::parse("hello world").is_none());
    }

    #[test]
    fn parse_unknown_command() {
        let result = SlashCommand::parse("/foobar").unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn parse_no_arg_commands() {
        for input in ["/sessions", "/models", "/usage", "/status", "/verbose",
                      "/health", "/clear", "/help", "/quit", "/compact"] {
            let cmd = SlashCommand::parse(input).unwrap();
            assert!(cmd.is_ok(), "failed for {input}");
        }
    }

    #[test]
    fn parse_tools_with_filter() {
        let cmd = SlashCommand::parse("/tools web").unwrap().unwrap();
        assert!(matches!(cmd, SlashCommand::Tools { filter: Some(f) } if f == "web"));
    }

    #[test]
    fn parse_memory_requires_query() {
        assert!(SlashCommand::parse("/memory").unwrap().is_err());
        let cmd = SlashCommand::parse("/memory rust generics").unwrap().unwrap();
        assert!(matches!(cmd, SlashCommand::Memory { query } if query == "rust generics"));
    }

    #[test]
    fn all_commands_returns_complete_list() {
        let cmds = SlashCommand::all_commands();
        assert!(cmds.len() >= 17);
        assert!(cmds.iter().any(|(name, _)| *name == "new"));
        assert!(cmds.iter().any(|(name, _)| *name == "quit"));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aleph-cli --lib tui::slash`
Expected: FAIL — module doesn't have the types yet.

**Step 3: Implement the parser**

Write `apps/cli/src/tui/slash.rs`:
```rust
/// Thinking level for AI reasoning depth
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" | "none" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// All slash commands supported by the TUI
#[derive(Debug, Clone)]
pub enum SlashCommand {
    // Session management
    New { name: Option<String> },
    Session { key: String },
    Sessions,
    Delete { key: String },

    // Model & thinking
    Model { name: String },
    Models,
    Think { level: ThinkingLevel },
    Usage,

    // Debug & status
    Status,
    Verbose,
    Health,
    Clear,

    // Tools & memory
    Tools { filter: Option<String> },
    Memory { query: String },
    Compact,

    // General
    Help,
    Quit,
}

impl SlashCommand {
    /// Parse user input into a slash command.
    /// Returns None if input doesn't start with '/'.
    /// Returns Some(Err(msg)) if command is unknown or args are invalid.
    pub fn parse(input: &str) -> Option<Result<Self, String>> {
        let input = input.trim();
        if !input.starts_with('/') {
            return None;
        }

        let mut parts = input[1..].splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().map(|s| s.trim().to_string());

        Some(match cmd.to_lowercase().as_str() {
            "new" | "reset" => Ok(Self::New { name: arg }),
            "session" => arg
                .map(|k| Self::Session { key: k })
                .ok_or_else(|| "Usage: /session <key>".to_string()),
            "sessions" => Ok(Self::Sessions),
            "delete" => arg
                .map(|k| Self::Delete { key: k })
                .ok_or_else(|| "Usage: /delete <key>".to_string()),
            "model" => arg
                .map(|n| Self::Model { name: n })
                .ok_or_else(|| "Usage: /model <name>".to_string()),
            "models" => Ok(Self::Models),
            "think" => arg
                .as_deref()
                .and_then(ThinkingLevel::parse)
                .map(|l| Self::Think { level: l })
                .ok_or_else(|| "Usage: /think off|low|medium|high".to_string()),
            "usage" => Ok(Self::Usage),
            "status" => Ok(Self::Status),
            "verbose" => Ok(Self::Verbose),
            "health" => Ok(Self::Health),
            "clear" => Ok(Self::Clear),
            "tools" => Ok(Self::Tools { filter: arg }),
            "memory" => arg
                .map(|q| Self::Memory { query: q })
                .ok_or_else(|| "Usage: /memory <query>".to_string()),
            "compact" => Ok(Self::Compact),
            "help" => Ok(Self::Help),
            "quit" | "exit" | "q" => Ok(Self::Quit),
            other => Err(format!("Unknown command: /{other}. Type /help for available commands.")),
        })
    }

    /// Returns all command names with descriptions (for command palette)
    pub fn all_commands() -> Vec<(&'static str, &'static str)> {
        vec![
            ("new", "Create new session"),
            ("session", "Switch session"),
            ("sessions", "List all sessions"),
            ("delete", "Delete session"),
            ("model", "Switch AI model"),
            ("models", "List available models"),
            ("think", "Set thinking level"),
            ("usage", "Show token usage"),
            ("status", "Show system status"),
            ("verbose", "Toggle verbose mode"),
            ("health", "Server health check"),
            ("clear", "Clear screen"),
            ("tools", "List available tools"),
            ("memory", "Search memory"),
            ("compact", "Compress context"),
            ("help", "Show help"),
            ("quit", "Exit TUI"),
        ]
    }

    /// Fuzzy-match command names against a prefix
    pub fn filter_commands(prefix: &str) -> Vec<(&'static str, &'static str)> {
        let prefix = prefix.to_lowercase();
        Self::all_commands()
            .into_iter()
            .filter(|(name, _)| name.starts_with(&prefix))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    // ... tests from Step 1 above
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p aleph-cli --lib tui::slash`
Expected: All 12 tests PASS.

**Step 5: Commit**

```bash
git add apps/cli/src/tui/slash.rs
git commit -m "cli: add slash command parser with 17 commands and fuzzy filter"
```

---

## Task 3: AppState, Action, and Event Types

**Files:**
- Create: `apps/cli/src/tui/app.rs`
- Create: `apps/cli/src/tui/event.rs`

**Step 1: Create app.rs with core types and state**

Write `apps/cli/src/tui/app.rs`:
```rust
use std::time::Duration;
use chrono::{DateTime, Utc};
use aleph_protocol::{StreamEvent, ToolResult as ProtoToolResult, RunSummary};
use super::slash::{SlashCommand, ThinkingLevel};

/// All possible actions from user/system events
#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    Tick,
    SendMessage(String),
    SlashCommand(SlashCommand),
    CancelRun(String),
    ScrollUp(usize),
    ScrollDown(usize),
    ScrollToBottom,
    ScrollToBottomIfAutoScroll,
    FocusInput,
    FocusChat,
    OpenCommandPalette,
    CloseOverlay,
    PaletteUp,
    PaletteDown,
    PaletteConfirm,
    DialogSelect(usize),
    ToggleVerbose,
    RespondToDialog { run_id: String, choice: String },
}

/// Focus state for keyboard routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Chat,
    CommandPalette,
    Dialog,
}

/// Tool execution status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Failed,
}

/// A single tool execution record
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

/// A chat message in the conversation
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

/// Dialog state for AskUser events
#[derive(Debug, Clone)]
pub struct DialogState {
    pub run_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub selected: usize,
}

/// Command palette state
#[derive(Debug, Clone)]
pub struct PaletteState {
    pub input: String,
    pub filtered: Vec<(&'static str, &'static str)>,
    pub selected: usize,
}

/// Main application state
pub struct AppState {
    // Chat
    pub messages: Vec<ChatMessage>,
    pub scroll_offset: usize,
    pub auto_scroll: bool,

    // Input
    pub send_history: Vec<String>,
    pub history_index: Option<usize>,

    // Session
    pub session_key: String,
    pub model_name: String,
    pub total_tokens: u64,
    pub is_connected: bool,

    // Run state
    pub current_run: Option<String>,
    pub last_run_duration: Option<Duration>,

    // Settings
    pub verbose: bool,
    pub thinking_level: ThinkingLevel,

    // Focus & overlays
    pub focus: Focus,
    pub dialog: Option<DialogState>,
    pub palette: Option<PaletteState>,

    // Ctrl+C tracking
    pub ctrl_c_count: u8,

    // Spinner
    pub spinner_frame: usize,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(session_key: String, model_name: String) -> Self {
        Self {
            messages: vec![ChatMessage::System {
                content: format!("Welcome to Aleph TUI. Session: {session_key}. Type /help for commands."),
            }],
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
            thinking_level: ThinkingLevel::Off,
            focus: Focus::Input,
            dialog: None,
            palette: None,
            ctrl_c_count: 0,
            spinner_frame: 0,
            should_quit: false,
        }
    }

    // --- Message management ---

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(ChatMessage::User {
            content,
            timestamp: Utc::now(),
        });
        self.auto_scroll = true;
    }

    pub fn add_system_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage::System {
            content: content.into(),
        });
        self.auto_scroll = true;
    }

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

    pub fn current_assistant_mut(&mut self) -> &mut ChatMessage {
        self.ensure_assistant_message();
        self.messages.last_mut().unwrap()
    }

    pub fn find_tool_mut(&mut self, tool_id: &str) -> Option<&mut ToolExecution> {
        if let Some(ChatMessage::Assistant { tools, .. }) = self.messages.last_mut() {
            tools.iter_mut().find(|t| t.id == tool_id)
        } else {
            None
        }
    }

    // --- Scroll ---

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.auto_scroll = false;
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if self.scroll_offset == 0 {
            self.auto_scroll = true;
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = true;
    }

    // --- Overlays ---

    pub fn open_command_palette(&mut self) {
        let filtered = SlashCommand::all_commands();
        self.palette = Some(PaletteState {
            input: String::new(),
            filtered,
            selected: 0,
        });
        self.focus = Focus::CommandPalette;
    }

    pub fn close_overlay(&mut self) {
        self.palette = None;
        if self.dialog.is_none() {
            self.focus = Focus::Input;
        }
    }

    pub fn show_dialog(&mut self, run_id: String, question: String, options: Vec<String>) {
        self.dialog = Some(DialogState {
            run_id,
            question,
            options,
            selected: 0,
        });
        self.focus = Focus::Dialog;
    }

    // --- Settings ---

    pub fn toggle_verbose(&mut self) {
        self.verbose = !self.verbose;
        let state = if self.verbose { "ON" } else { "OFF" };
        self.add_system_message(format!("Verbose mode: {state}"));
    }

    pub fn set_thinking(&mut self, level: ThinkingLevel) {
        self.thinking_level = level;
        self.add_system_message(format!("Thinking level: {}", level.as_str()));
    }

    pub fn set_model(&mut self, name: &str) {
        self.model_name = name.to_string();
    }

    pub fn switch_session(&mut self, key: String) {
        self.session_key = key.clone();
        self.messages.clear();
        self.add_system_message(format!("Switched to session: {key}"));
    }

    pub fn clear_screen(&mut self) {
        self.messages.clear();
        self.add_system_message("Screen cleared.");
    }

    pub fn update_token_usage(&mut self, summary: &RunSummary) {
        self.total_tokens = self.total_tokens.saturating_add(summary.total_tokens);
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    // --- Gateway events ---

    pub fn handle_gateway_event(&mut self, event: StreamEvent) -> Action {
        match event {
            StreamEvent::RunAccepted { run_id, .. } => {
                self.current_run = Some(run_id);
                self.ensure_assistant_message();
                Action::None
            }
            StreamEvent::Reasoning { content, .. } => {
                if self.verbose {
                    if let Some(ChatMessage::Assistant { reasoning, .. }) = self.messages.last_mut() {
                        let r = reasoning.get_or_insert_with(String::new);
                        r.push_str(&content);
                    }
                }
                Action::None
            }
            StreamEvent::ResponseChunk { content, is_final, .. } => {
                if let Some(ChatMessage::Assistant { content: c, is_streaming, .. }) = self.messages.last_mut() {
                    c.push_str(&content);
                    if is_final {
                        *is_streaming = false;
                    }
                }
                Action::ScrollToBottomIfAutoScroll
            }
            StreamEvent::ToolStart { tool_name, tool_id, params, .. } => {
                let params_brief = format_params_brief(&params);
                if let Some(ChatMessage::Assistant { tools, .. }) = self.messages.last_mut() {
                    tools.push(ToolExecution {
                        id: tool_id,
                        name: tool_name,
                        params: params_brief,
                        status: ToolStatus::Running,
                        duration: None,
                        progress: None,
                        error: None,
                    });
                }
                Action::ScrollToBottomIfAutoScroll
            }
            StreamEvent::ToolUpdate { tool_id, progress, .. } => {
                if let Some(tool) = self.find_tool_mut(&tool_id) {
                    tool.progress = Some(progress);
                }
                Action::None
            }
            StreamEvent::ToolEnd { tool_id, result, duration_ms, .. } => {
                if let Some(tool) = self.find_tool_mut(&tool_id) {
                    tool.status = if result.success { ToolStatus::Success } else { ToolStatus::Failed };
                    tool.duration = Some(Duration::from_millis(duration_ms));
                    tool.error = result.error;
                }
                Action::None
            }
            StreamEvent::RunComplete { summary, total_duration_ms, .. } => {
                if let Some(ChatMessage::Assistant { is_streaming, .. }) = self.messages.last_mut() {
                    *is_streaming = false;
                }
                self.current_run = None;
                self.last_run_duration = Some(Duration::from_millis(total_duration_ms));
                self.update_token_usage(&summary);
                Action::None
            }
            StreamEvent::RunError { error, .. } => {
                if let Some(ChatMessage::Assistant { is_streaming, .. }) = self.messages.last_mut() {
                    *is_streaming = false;
                }
                self.current_run = None;
                self.add_system_message(format!("Error: {error}"));
                Action::None
            }
            StreamEvent::AskUser { run_id, question, options, .. } => {
                self.show_dialog(run_id, question, options);
                Action::None
            }
            _ => Action::None,
        }
    }
}

/// Format tool params as a brief one-line summary
fn format_params_brief(params: &serde_json::Value) -> String {
    match params {
        serde_json::Value::String(s) => {
            if s.len() > 60 {
                format!("\"{}...\"", &s[..57])
            } else {
                format!("\"{s}\"")
            }
        }
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map.iter().take(3).map(|(k, v)| {
                let v_str = match v {
                    serde_json::Value::String(s) if s.len() > 30 => format!("\"{}...\"", &s[..27]),
                    serde_json::Value::String(s) => format!("\"{s}\""),
                    other => {
                        let s = other.to_string();
                        if s.len() > 30 { format!("{}...", &s[..27]) } else { s }
                    }
                };
                format!("{k}: {v_str}")
            }).collect();
            if map.len() > 3 {
                format!("{} (+{})", parts.join(", "), map.len() - 3)
            } else {
                parts.join(", ")
            }
        }
        other => {
            let s = other.to_string();
            if s.len() > 60 { format!("{}...", &s[..57]) } else { s }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_state_has_welcome_message() {
        let state = AppState::new("test-session".into(), "claude".into());
        assert_eq!(state.messages.len(), 1);
        assert!(matches!(&state.messages[0], ChatMessage::System { .. }));
    }

    #[test]
    fn scroll_up_disables_auto_scroll() {
        let mut state = AppState::new("s".into(), "m".into());
        assert!(state.auto_scroll);
        state.scroll_up(5);
        assert!(!state.auto_scroll);
        assert_eq!(state.scroll_offset, 5);
    }

    #[test]
    fn scroll_to_bottom_re_enables_auto_scroll() {
        let mut state = AppState::new("s".into(), "m".into());
        state.scroll_up(10);
        state.scroll_to_bottom();
        assert!(state.auto_scroll);
        assert_eq!(state.scroll_offset, 0);
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
        let v = json!("hello world");
        assert_eq!(format_params_brief(&v), "\"hello world\"");
    }

    #[test]
    fn format_params_object() {
        let v = json!({"query": "rust", "limit": 10});
        let s = format_params_brief(&v);
        assert!(s.contains("query"));
        assert!(s.contains("limit"));
    }

    #[test]
    fn ensure_assistant_message_creates_one() {
        let mut state = AppState::new("s".into(), "m".into());
        state.ensure_assistant_message();
        assert_eq!(state.messages.len(), 2);
        assert!(matches!(&state.messages[1], ChatMessage::Assistant { is_streaming: true, .. }));
    }

    #[test]
    fn ensure_assistant_message_idempotent() {
        let mut state = AppState::new("s".into(), "m".into());
        state.ensure_assistant_message();
        state.ensure_assistant_message();
        assert_eq!(state.messages.len(), 2); // still 2, not 3
    }
}
```

**Step 2: Create event.rs for terminal event collection**

Write `apps/cli/src/tui/event.rs`:
```rust
use crossterm::event::{self, Event as CrosstermEvent, KeyEvent};
use std::time::Duration;
use tokio::sync::mpsc;

/// Collected terminal event
#[derive(Debug)]
pub enum TermEvent {
    Key(KeyEvent),
    Resize(u16, u16),
}

/// Spawns a background task that polls crossterm events and sends them to a channel.
/// Returns the receiver. The task runs until the sender is dropped.
pub fn spawn_event_collector() -> mpsc::Receiver<TermEvent> {
    let (tx, rx) = mpsc::channel(64);

    // Use a blocking thread for crossterm polling (it blocks)
    tokio::task::spawn_blocking(move || {
        loop {
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => {
                    if let Ok(ev) = event::read() {
                        let term_event = match ev {
                            CrosstermEvent::Key(key) => Some(TermEvent::Key(key)),
                            CrosstermEvent::Resize(w, h) => Some(TermEvent::Resize(w, h)),
                            _ => None,
                        };
                        if let Some(te) = term_event {
                            if tx.blocking_send(te).is_err() {
                                break; // Receiver dropped
                            }
                        }
                    }
                }
                Ok(false) => {} // No event, continue polling
                Err(_) => break, // Terminal error
            }
        }
    });

    rx
}
```

**Step 3: Run tests**

Run: `cargo test -p aleph-cli --lib tui::app`
Expected: All tests PASS.

**Step 4: Verify full compile**

Run: `cargo check -p aleph-cli`
Expected: Compiles (with warnings about unused items — that's fine).

**Step 5: Commit**

```bash
git add apps/cli/src/tui/app.rs apps/cli/src/tui/event.rs
git commit -m "cli: add AppState, Action types, event collector, and gateway event handling"
```

---

## Task 4: Markdown Renderer

**Files:**
- Create: `apps/cli/src/tui/markdown.rs`

**Step 1: Write tests for markdown rendering**

At bottom of `apps/cli/src/tui/markdown.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn plain_text() {
        let lines = markdown_to_lines("Hello world", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_to_plain_text(&lines[0]), "Hello world");
    }

    #[test]
    fn bold_text() {
        let lines = markdown_to_lines("Hello **world**", 80);
        assert_eq!(lines.len(), 1);
        // Should contain a bold span
        let spans = &lines[0].spans;
        assert!(spans.iter().any(|s| s.content.contains("world")
            && s.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn inline_code() {
        let lines = markdown_to_lines("Use `cargo build`", 80);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert!(spans.iter().any(|s| s.content.contains("cargo build")));
    }

    #[test]
    fn code_block() {
        let input = "```rust\nfn main() {}\n```";
        let lines = markdown_to_lines(input, 80);
        assert!(lines.len() >= 3); // border + code + border
    }

    #[test]
    fn heading() {
        let lines = markdown_to_lines("# Title", 80);
        assert!(!lines.is_empty());
        let spans = &lines[0].spans;
        assert!(spans.iter().any(|s| s.content.contains("Title")
            && s.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn list_item() {
        let lines = markdown_to_lines("- item one\n- item two", 80);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn blockquote() {
        let lines = markdown_to_lines("> quoted text", 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn wraps_long_lines() {
        let long = "a ".repeat(50); // 100 chars
        let lines = markdown_to_lines(&long, 40);
        assert!(lines.len() > 1);
    }

    fn line_to_plain_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect::<String>()
    }
}
```

**Step 2: Implement the markdown renderer**

Write `apps/cli/src/tui/markdown.rs`:
```rust
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use super::theme::DEFAULT_THEME;

/// Convert markdown text to styled ratatui Lines
pub fn markdown_to_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_lines: Vec<String> = Vec::new();

    for raw_line in text.lines() {
        // Code block toggle
        if raw_line.trim_start().starts_with("```") {
            if in_code_block {
                // End code block — render accumulated code
                render_code_block(&mut lines, &code_lang, &code_lines, width);
                code_lines.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                // Start code block
                in_code_block = true;
                code_lang = raw_line.trim_start().trim_start_matches('`').to_string();
            }
            continue;
        }

        if in_code_block {
            code_lines.push(raw_line.to_string());
            continue;
        }

        // Heading
        if let Some(heading) = raw_line.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(
                heading.to_string(),
                Style::default().fg(DEFAULT_THEME.heading).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            continue;
        }
        if let Some(heading) = raw_line.strip_prefix("## ") {
            lines.push(Line::from(Span::styled(
                heading.to_string(),
                Style::default().fg(DEFAULT_THEME.heading).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(heading) = raw_line.strip_prefix("### ") {
            lines.push(Line::from(Span::styled(
                heading.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            continue;
        }

        // Blockquote
        if let Some(quoted) = raw_line.strip_prefix("> ") {
            let mut spans = vec![Span::styled("┊ ", Style::default().fg(DEFAULT_THEME.quote))];
            spans.extend(parse_inline(quoted, DEFAULT_THEME.quote));
            lines.push(Line::from(spans));
            continue;
        }

        // List item
        if let Some(item) = raw_line.strip_prefix("- ").or_else(|| raw_line.strip_prefix("* ")) {
            let mut spans = vec![Span::raw("  • ")];
            spans.extend(parse_inline(item, DEFAULT_THEME.primary));
            let wrapped = wrap_line_spans(spans, width);
            lines.extend(wrapped);
            continue;
        }

        // Empty line
        if raw_line.trim().is_empty() {
            lines.push(Line::default());
            continue;
        }

        // Normal paragraph with inline formatting
        let spans = parse_inline(raw_line, DEFAULT_THEME.primary);
        let wrapped = wrap_line_spans(spans, width);
        lines.extend(wrapped);
    }

    // Handle unterminated code block
    if in_code_block && !code_lines.is_empty() {
        render_code_block(&mut lines, &code_lang, &code_lines, width);
    }

    lines
}

/// Parse inline markdown: **bold**, *italic*, `code`, [link](url)
fn parse_inline(text: &str, base_color: Color) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut current = String::new();
    let base_style = Style::default().fg(base_color);

    while let Some((i, ch)) = chars.next() {
        match ch {
            '`' => {
                // Inline code
                if !current.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut current), base_style));
                }
                let mut code = String::new();
                for (_, c) in chars.by_ref() {
                    if c == '`' { break; }
                    code.push(c);
                }
                spans.push(Span::styled(
                    format!(" {code} "),
                    Style::default().fg(DEFAULT_THEME.primary).bg(DEFAULT_THEME.code_bg),
                ));
            }
            '*' => {
                // Bold or italic
                let is_bold = chars.peek().map(|(_, c)| *c == '*').unwrap_or(false);
                if is_bold {
                    chars.next(); // consume second *
                    if !current.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut current), base_style));
                    }
                    let mut bold_text = String::new();
                    while let Some((_, c)) = chars.next() {
                        if c == '*' {
                            if chars.peek().map(|(_, c)| *c == '*').unwrap_or(false) {
                                chars.next(); // consume closing **
                                break;
                            }
                        }
                        bold_text.push(c);
                    }
                    spans.push(Span::styled(
                        bold_text,
                        Style::default().fg(base_color).add_modifier(Modifier::BOLD),
                    ));
                } else {
                    // Italic
                    if !current.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut current), base_style));
                    }
                    let mut italic_text = String::new();
                    for (_, c) in chars.by_ref() {
                        if c == '*' { break; }
                        italic_text.push(c);
                    }
                    spans.push(Span::styled(
                        italic_text,
                        Style::default().fg(base_color).add_modifier(Modifier::ITALIC),
                    ));
                }
            }
            '[' => {
                // Link: [text](url)
                if !current.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut current), base_style));
                }
                let mut link_text = String::new();
                let mut found_close = false;
                for (_, c) in chars.by_ref() {
                    if c == ']' { found_close = true; break; }
                    link_text.push(c);
                }
                if found_close && chars.peek().map(|(_, c)| *c == '(').unwrap_or(false) {
                    chars.next(); // consume (
                    // Skip URL content
                    for (_, c) in chars.by_ref() {
                        if c == ')' { break; }
                    }
                    spans.push(Span::styled(
                        link_text,
                        Style::default().fg(DEFAULT_THEME.link).add_modifier(Modifier::UNDERLINED),
                    ));
                } else {
                    // Not a valid link, output as-is
                    current.push('[');
                    current.push_str(&link_text);
                    if found_close { current.push(']'); }
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        spans.push(Span::styled(current, base_style));
    }

    spans
}

/// Render a code block with border
fn render_code_block(lines: &mut Vec<Line<'static>>, lang: &str, code: &[String], width: usize) {
    let border_style = Style::default().fg(DEFAULT_THEME.code_block_border);
    let code_style = Style::default().fg(DEFAULT_THEME.primary);
    let lang_display = if lang.is_empty() { "code".to_string() } else { lang.clone() };

    // Top border
    let top = format!("┌─ {lang_display} {}", "─".repeat(width.saturating_sub(lang_display.len() + 4)));
    lines.push(Line::from(Span::styled(top, border_style)));

    // Code lines
    for line in code {
        let padded = format!("│ {line}");
        lines.push(Line::from(Span::styled(padded, code_style)));
    }

    // Bottom border
    let bottom = format!("└{}", "─".repeat(width.saturating_sub(1)));
    lines.push(Line::from(Span::styled(bottom, border_style)));
}

/// Wrap spans across multiple lines respecting terminal width.
/// Simple approach: join spans into one string, wrap, re-apply base style.
fn wrap_line_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 { return vec![Line::from(spans)]; }

    // Calculate total visual width
    let total: usize = spans.iter().map(|s| unicode_display_width(&s.content)).sum();
    if total <= width {
        return vec![Line::from(spans)];
    }

    // For wrapped lines, flatten to plain text and re-wrap
    // (This loses inline formatting on wrapped lines — acceptable for v1)
    let plain: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let options = textwrap::Options::new(width);
    textwrap::wrap(&plain, options)
        .into_iter()
        .map(|cow| Line::from(Span::raw(cow.into_owned())))
        .collect()
}

/// Calculate display width accounting for CJK and emoji
fn unicode_display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    s.width()
}
```

**Step 3: Run tests**

Run: `cargo test -p aleph-cli --lib tui::markdown`
Expected: All tests PASS.

**Step 4: Commit**

```bash
git add apps/cli/src/tui/markdown.rs
git commit -m "cli: add markdown-to-ratatui renderer with inline formatting and code blocks"
```

---

## Task 5: Widget Implementations

**Files:**
- Create: `apps/cli/src/tui/widgets/mod.rs`
- Create: `apps/cli/src/tui/widgets/status_bar.rs`
- Create: `apps/cli/src/tui/widgets/tool_block.rs`
- Create: `apps/cli/src/tui/widgets/chat_area.rs`
- Create: `apps/cli/src/tui/widgets/input_area.rs`
- Create: `apps/cli/src/tui/widgets/command_palette.rs`
- Create: `apps/cli/src/tui/widgets/dialog.rs`

This is a large task. Implement each widget file in order: status_bar (simplest) → tool_block → chat_area → input_area → command_palette → dialog.

**Step 1: Create widgets/mod.rs**

```rust
pub mod chat_area;
pub mod command_palette;
pub mod dialog;
pub mod input_area;
pub mod status_bar;
pub mod tool_block;
```

**Step 2: Implement status_bar.rs**

```rust
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use ratatui::Frame;
use crate::tui::theme::DEFAULT_THEME;

pub struct StatusBar<'a> {
    pub model: &'a str,
    pub session: &'a str,
    pub tokens: u64,
    pub is_connected: bool,
}

impl<'a> StatusBar<'a> {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let dot = if self.is_connected { "●" } else { "○" };
        let dot_color = if self.is_connected { DEFAULT_THEME.connected } else { DEFAULT_THEME.disconnected };
        let token_str = format_tokens(self.tokens);

        let line = Line::from(vec![
            Span::styled(format!(" {dot} "), Style::default().fg(dot_color).bg(DEFAULT_THEME.status_bg)),
            Span::styled(self.model, Style::default().fg(DEFAULT_THEME.status_fg).bg(DEFAULT_THEME.status_bg)),
            Span::styled(" │ ", Style::default().fg(DEFAULT_THEME.muted).bg(DEFAULT_THEME.status_bg)),
            Span::styled(self.session, Style::default().fg(DEFAULT_THEME.status_fg).bg(DEFAULT_THEME.status_bg)),
            Span::styled(" │ ", Style::default().fg(DEFAULT_THEME.muted).bg(DEFAULT_THEME.status_bg)),
            Span::styled(token_str, Style::default().fg(DEFAULT_THEME.status_fg).bg(DEFAULT_THEME.status_bg)),
            Span::styled(" │ /help for commands ", Style::default().fg(DEFAULT_THEME.muted).bg(DEFAULT_THEME.status_bg)),
        ]);

        // Fill remaining width with background
        let paragraph = ratatui::widgets::Paragraph::new(line)
            .style(Style::default().bg(DEFAULT_THEME.status_bg));
        frame.render_widget(paragraph, area);
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tok", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k tok", n as f64 / 1_000.0)
    } else {
        format!("{n} tok")
    }
}
```

**Step 3: Implement tool_block.rs**

```rust
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use crate::tui::app::{ToolExecution, ToolStatus};
use crate::tui::theme::DEFAULT_THEME;

const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn render_tool_block(tool: &ToolExecution, spinner_frame: usize, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let w = width as usize;

    // Status indicator
    let (status_char, status_style) = match tool.status {
        ToolStatus::Running => {
            let ch = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
            (ch.to_string(), Style::default().fg(DEFAULT_THEME.tool_running))
        }
        ToolStatus::Success => {
            let dur = tool.duration.map(|d| format!(" {:.1}s", d.as_secs_f64())).unwrap_or_default();
            (format!("{dur} ✓"), Style::default().fg(DEFAULT_THEME.tool_success))
        }
        ToolStatus::Failed => {
            let dur = tool.duration.map(|d| format!(" {:.1}s", d.as_secs_f64())).unwrap_or_default();
            (format!("{dur} ✗"), Style::default().fg(DEFAULT_THEME.tool_failed))
        }
    };

    // Top border: ┌ tool_name ──── status ┐
    let name_len = tool.name.len();
    let status_len = status_char.len();
    let fill = w.saturating_sub(name_len + status_len + 6); // 6 = "┌ " + " " + " ┐"
    let top = Line::from(vec![
        Span::styled("  ┌ ", Style::default().fg(DEFAULT_THEME.border)),
        Span::styled(tool.name.clone(), Style::default().fg(DEFAULT_THEME.tool_name)),
        Span::styled(" ".to_string() + &"─".repeat(fill) + " ", Style::default().fg(DEFAULT_THEME.border)),
        Span::styled(status_char, status_style),
        Span::styled(" ┐", Style::default().fg(DEFAULT_THEME.border)),
    ]);
    lines.push(top);

    // Params line
    if !tool.params.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(DEFAULT_THEME.border)),
            Span::styled(tool.params.clone(), Style::default().fg(DEFAULT_THEME.tool_param)),
        ]));
    }

    // Error line (if failed)
    if let Some(err) = &tool.error {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(DEFAULT_THEME.border)),
            Span::styled(format!("Error: {err}"), Style::default().fg(DEFAULT_THEME.error)),
        ]));
    }

    // Bottom border
    let bottom_fill = w.saturating_sub(4);
    lines.push(Line::from(Span::styled(
        format!("  └{}┘", "─".repeat(bottom_fill)),
        Style::default().fg(DEFAULT_THEME.border),
    )));

    lines
}
```

**Step 4: Implement chat_area.rs**

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use crate::tui::app::{AppState, ChatMessage};
use crate::tui::markdown::markdown_to_lines;
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::tool_block::render_tool_block;

pub fn render_chat_area(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(
            if state.focus == super::super::app::Focus::Chat {
                DEFAULT_THEME.border_focused
            } else {
                DEFAULT_THEME.border
            }
        ))
        .title(" Chat ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build all rendered lines from messages
    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let content_width = inner.width.saturating_sub(2); // padding

    for msg in &state.messages {
        match msg {
            ChatMessage::User { content, timestamp } => {
                let time = timestamp.format("%H:%M:%S").to_string();
                all_lines.push(Line::from(vec![
                    Span::styled("┃ ", Style::default().fg(DEFAULT_THEME.user)),
                    Span::styled("You", Style::default().fg(DEFAULT_THEME.user).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  {time}"), Style::default().fg(DEFAULT_THEME.muted)),
                ]));
                let md_lines = markdown_to_lines(content, content_width);
                for line in md_lines {
                    let mut prefixed = vec![Span::styled("│ ", Style::default().fg(DEFAULT_THEME.user))];
                    prefixed.extend(line.spans);
                    all_lines.push(Line::from(prefixed));
                }
                all_lines.push(Line::default()); // spacing
            }
            ChatMessage::Assistant { content, tools, reasoning, is_streaming } => {
                all_lines.push(Line::from(vec![
                    Span::styled("┃ ", Style::default().fg(DEFAULT_THEME.assistant)),
                    Span::styled("Aleph", Style::default().fg(DEFAULT_THEME.assistant).add_modifier(Modifier::BOLD)),
                ]));

                // Reasoning (if verbose)
                if let Some(reason) = reasoning {
                    if !reason.is_empty() {
                        for rline in reason.lines() {
                            all_lines.push(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(DEFAULT_THEME.assistant)),
                                Span::styled("┊ ", Style::default().fg(DEFAULT_THEME.reasoning)),
                                Span::styled(rline.to_string(), Style::default().fg(DEFAULT_THEME.reasoning)),
                            ]));
                        }
                        all_lines.push(Line::default());
                    }
                }

                // Tool executions
                for tool in tools {
                    let tool_lines = render_tool_block(tool, state.spinner_frame, content_width);
                    for tl in tool_lines {
                        let mut prefixed = vec![Span::styled("│ ", Style::default().fg(DEFAULT_THEME.assistant))];
                        prefixed.extend(tl.spans);
                        all_lines.push(Line::from(prefixed));
                    }
                    all_lines.push(Line::default());
                }

                // Content
                if !content.is_empty() {
                    let md_lines = markdown_to_lines(content, content_width);
                    for line in md_lines {
                        let mut prefixed = vec![Span::styled("│ ", Style::default().fg(DEFAULT_THEME.assistant))];
                        prefixed.extend(line.spans);
                        all_lines.push(Line::from(prefixed));
                    }
                }

                // Streaming cursor
                if *is_streaming {
                    all_lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(DEFAULT_THEME.assistant)),
                        Span::styled("▍", Style::default().fg(DEFAULT_THEME.assistant)),
                    ]));
                }

                all_lines.push(Line::default()); // spacing
            }
            ChatMessage::System { content } => {
                all_lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(content.clone(), Style::default().fg(DEFAULT_THEME.system)),
                ]));
                all_lines.push(Line::default());
            }
        }
    }

    // Apply scrolling
    let visible_height = inner.height as usize;
    let total = all_lines.len();
    let start = if state.auto_scroll {
        total.saturating_sub(visible_height)
    } else {
        total.saturating_sub(visible_height).saturating_sub(state.scroll_offset)
    };
    let visible: Vec<Line> = all_lines.into_iter().skip(start).take(visible_height).collect();

    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, inner);
}
```

**Step 5: Implement input_area.rs**

```rust
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use tui_textarea::TextArea;
use crate::tui::app::Focus;
use crate::tui::theme::DEFAULT_THEME;

pub struct InputWidget<'a> {
    pub textarea: &'a TextArea<'a>,
    pub focused: bool,
}

impl<'a> InputWidget<'a> {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let border_color = if self.focused {
            DEFAULT_THEME.border_focused
        } else {
            DEFAULT_THEME.border
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(" Input (Enter=send, Shift+Enter=newline) ");

        // tui-textarea handles its own rendering with the block
        let mut ta = self.textarea.clone();
        ta.set_block(block);
        frame.render_widget(&ta, area);
    }
}

/// Calculate dynamic height for input area based on content
pub fn input_height(textarea: &TextArea, min: u16, max: u16) -> u16 {
    let lines = textarea.lines().len() as u16;
    // +2 for borders
    (lines + 2).clamp(min, max)
}
```

**Step 6: Implement command_palette.rs**

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;
use crate::tui::app::PaletteState;
use crate::tui::theme::DEFAULT_THEME;

pub fn render_command_palette(frame: &mut Frame, palette: &PaletteState, area: Rect) {
    // Calculate overlay position: above the input area, centered
    let height = (palette.filtered.len() as u16 + 2).min(12); // +2 for border
    let width = 42.min(area.width);
    let x = area.x + 1;
    let y = area.y.saturating_sub(height);

    let overlay = Rect::new(x, y, width, height);

    // Clear area behind overlay
    frame.render_widget(Clear, overlay);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(" Commands ");

    let items: Vec<ListItem> = palette.filtered.iter().enumerate().map(|(i, (name, desc))| {
        let indicator = if i == palette.selected { ">" } else { " " };
        let style = if i == palette.selected {
            Style::default().fg(DEFAULT_THEME.primary).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DEFAULT_THEME.muted)
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{indicator} /"), style),
            Span::styled(format!("{name:<12}"), style),
            Span::styled(*desc, Style::default().fg(DEFAULT_THEME.muted)),
        ]))
    }).collect();

    let list = List::new(items).block(block);
    let mut list_state = ListState::default().with_selected(Some(palette.selected));
    frame.render_stateful_widget(list, overlay, &mut list_state);
}
```

**Step 7: Implement dialog.rs**

```rust
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use crate::tui::app::DialogState;
use crate::tui::theme::DEFAULT_THEME;

pub fn render_dialog(frame: &mut Frame, dialog: &DialogState, area: Rect) {
    let height = 4 + 1; // question + options + border
    let width = 50.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height.saturating_sub(height + 6);

    let overlay = Rect::new(x, y, width, height);
    frame.render_widget(Clear, overlay);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.warning))
        .title(" Agent needs your input ");

    let mut lines = vec![
        Line::from(Span::styled(&dialog.question, Style::default().fg(DEFAULT_THEME.primary))),
        Line::default(),
    ];

    let options_line: Vec<Span> = dialog.options.iter().enumerate().flat_map(|(i, opt)| {
        let style = if i == dialog.selected {
            Style::default().fg(DEFAULT_THEME.primary).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(DEFAULT_THEME.muted)
        };
        vec![
            Span::styled(format!(" [{}] ", i + 1), Style::default().fg(DEFAULT_THEME.warning)),
            Span::styled(opt.clone(), style),
            Span::raw("  "),
        ]
    }).collect();
    lines.push(Line::from(options_line));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, overlay);
}
```

**Step 8: Verify compilation**

Run: `cargo check -p aleph-cli`
Expected: Compiles with warnings about unused items.

**Step 9: Commit**

```bash
git add apps/cli/src/tui/widgets/
git commit -m "cli: add TUI widgets — chat area, input, status bar, tool block, palette, dialog"
```

---

## Task 6: Render Function & Main Loop

**Files:**
- Create: `apps/cli/src/tui/render.rs`
- Modify: `apps/cli/src/tui/mod.rs` (implement `run()`)

**Step 1: Implement render.rs**

```rust
use ratatui::layout::{Constraint, Layout};
use ratatui::Frame;
use crate::tui::app::AppState;
use crate::tui::widgets::{
    chat_area::render_chat_area,
    command_palette::render_command_palette,
    dialog::render_dialog,
    input_area::{InputWidget, input_height},
    status_bar::StatusBar,
};
use tui_textarea::TextArea;

pub fn render(frame: &mut Frame, state: &AppState, textarea: &TextArea) {
    let input_h = input_height(textarea, 3, 8);

    let chunks = Layout::vertical([
        Constraint::Min(5),              // Chat area
        Constraint::Length(input_h),     // Input area
        Constraint::Length(1),           // Status bar
    ]).split(frame.area());

    // Chat area
    render_chat_area(frame, state, chunks[0]);

    // Input area
    let input_widget = InputWidget {
        textarea,
        focused: state.focus == crate::tui::app::Focus::Input,
    };
    input_widget.render(frame, chunks[1]);

    // Status bar
    let status = StatusBar {
        model: &state.model_name,
        session: &state.session_key,
        tokens: state.total_tokens,
        is_connected: state.is_connected,
    };
    status.render(frame, chunks[2]);

    // Overlays (rendered last, on top)
    if let Some(palette) = &state.palette {
        render_command_palette(frame, palette, chunks[1]);
    }
    if let Some(dialog) = &state.dialog {
        render_dialog(frame, dialog, frame.area());
    }
}
```

**Step 2: Implement the full TUI main loop in mod.rs**

Replace the `todo!()` in `apps/cli/src/tui/mod.rs` with:

```rust
mod app;
mod event;
mod markdown;
mod render;
mod slash;
mod theme;
mod widgets;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tui_textarea::{Input, Key, TextArea};
use aleph_protocol::StreamEvent;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use app::{Action, AppState, Focus};
use slash::SlashCommand;

/// Entry point: run the TUI application
pub async fn run(
    client: AlephClient,
    mut gateway_events: mpsc::Receiver<StreamEvent>,
    config: &CliConfig,
    session_key: String,
) -> CliResult<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let model_name = "claude".to_string(); // TODO: get from config or server
    let mut state = AppState::new(session_key.clone(), model_name);
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("Type your message...");

    let mut term_events = event::spawn_event_collector();
    let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Render
        terminal.draw(|frame| {
            render::render(frame, &state, &textarea);
        })?;

        // Collect events
        let action = tokio::select! {
            Some(term_event) = term_events.recv() => {
                handle_terminal_event(&mut state, &mut textarea, term_event)
            }
            Some(gw_event) = gateway_events.recv() => {
                state.handle_gateway_event(gw_event)
            }
            _ = tick_interval.tick() => {
                state.spinner_frame = state.spinner_frame.wrapping_add(1);
                state.ctrl_c_count = 0; // Reset on tick
                Action::Tick
            }
        };

        // Execute action
        match action {
            Action::Quit => break,
            Action::SendMessage(msg) => {
                state.add_user_message(msg.clone());
                state.send_history.push(msg.clone());
                state.history_index = None;
                // Send via RPC
                let params = serde_json::json!({
                    "session_key": state.session_key,
                    "message": msg,
                });
                if let Err(e) = client.call::<_, serde_json::Value>("agent.run", Some(params)).await {
                    state.add_system_message(format!("Failed to send: {e}"));
                }
            }
            Action::SlashCommand(cmd) => {
                execute_slash_command(&mut state, &client, cmd).await;
            }
            Action::CancelRun(run_id) => {
                let _ = client.call::<_, serde_json::Value>(
                    "agent.cancel", Some(serde_json::json!({"run_id": run_id}))
                ).await;
                state.add_system_message("Run cancelled.");
                state.current_run = None;
            }
            Action::ScrollUp(n) => state.scroll_up(n),
            Action::ScrollDown(n) => state.scroll_down(n),
            Action::ScrollToBottom => state.scroll_to_bottom(),
            Action::ScrollToBottomIfAutoScroll => {
                if state.auto_scroll { state.scroll_to_bottom(); }
            }
            Action::FocusInput => { state.focus = Focus::Input; }
            Action::FocusChat => { state.focus = Focus::Chat; }
            Action::OpenCommandPalette => state.open_command_palette(),
            Action::CloseOverlay => state.close_overlay(),
            Action::ToggleVerbose => state.toggle_verbose(),
            Action::RespondToDialog { run_id, choice } => {
                let _ = client.call::<_, serde_json::Value>(
                    "agent.respond", Some(serde_json::json!({"run_id": run_id, "choice": choice}))
                ).await;
                state.dialog = None;
                state.focus = Focus::Input;
            }
            _ => {} // PaletteUp/Down/Confirm, DialogSelect handled inline
        }

        if state.should_quit { break; }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn handle_terminal_event(
    state: &mut AppState,
    textarea: &mut TextArea,
    event: event::TermEvent,
) -> Action {
    let event::TermEvent::Key(key) = event else {
        return Action::None; // Ignore resize for now
    };

    // Global: Ctrl+C
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        if let Some(run_id) = state.current_run.clone() {
            return Action::CancelRun(run_id);
        } else if !textarea.lines().join("").is_empty() {
            textarea.select_all();
            textarea.delete_char();
            return Action::None;
        } else {
            state.ctrl_c_count += 1;
            if state.ctrl_c_count >= 2 {
                return Action::Quit;
            }
            state.add_system_message("Press Ctrl+C again to quit");
            return Action::None;
        }
    }

    // Global: Ctrl+D
    if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Action::Quit;
    }

    // Global: Esc
    if key.code == KeyCode::Esc {
        if state.palette.is_some() || state.dialog.is_some() {
            return Action::CloseOverlay;
        }
    }

    // Global: F1
    if key.code == KeyCode::F(1) {
        return Action::SlashCommand(SlashCommand::Help);
    }

    // Route by focus
    match state.focus {
        Focus::Input => handle_input_key(state, textarea, key),
        Focus::Chat => handle_chat_key(state, key),
        Focus::CommandPalette => handle_palette_key(state, textarea, key),
        Focus::Dialog => handle_dialog_key(state, key),
    }
}

fn handle_input_key(state: &mut AppState, textarea: &mut TextArea, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            let text: String = textarea.lines().join("\n").trim().to_string();
            if text.is_empty() { return Action::None; }

            // Clear textarea
            textarea.select_all();
            textarea.delete_char();

            // Check for slash command
            if let Some(result) = SlashCommand::parse(&text) {
                match result {
                    Ok(cmd) => return Action::SlashCommand(cmd),
                    Err(msg) => {
                        state.add_system_message(msg);
                        return Action::None;
                    }
                }
            }

            Action::SendMessage(text)
        }
        KeyCode::Char('/') if textarea.lines().join("").is_empty() => {
            textarea.input(Input { key: Key::Char('/'), ..Default::default() });
            state.open_command_palette();
            Action::None
        }
        KeyCode::Up if textarea.lines().join("").is_empty() => {
            // Browse history
            if let Some(idx) = state.history_index {
                if idx + 1 < state.send_history.len() {
                    state.history_index = Some(idx + 1);
                } else {
                    return Action::FocusChat; // At top of history, switch to chat
                }
            } else if !state.send_history.is_empty() {
                state.history_index = Some(0);
            } else {
                return Action::FocusChat;
            }
            if let Some(idx) = state.history_index {
                let hist = state.send_history[state.send_history.len() - 1 - idx].clone();
                textarea.select_all();
                textarea.delete_char();
                textarea.insert_str(&hist);
            }
            Action::None
        }
        KeyCode::Down if state.history_index.is_some() => {
            if let Some(idx) = state.history_index {
                if idx == 0 {
                    state.history_index = None;
                    textarea.select_all();
                    textarea.delete_char();
                } else {
                    state.history_index = Some(idx - 1);
                    let hist = state.send_history[state.send_history.len() - idx].clone();
                    textarea.select_all();
                    textarea.delete_char();
                    textarea.insert_str(&hist);
                }
            }
            Action::None
        }
        _ => {
            // Pass to tui-textarea
            textarea.input(Input::from(key));
            state.history_index = None;
            Action::None
        }
    }
}

fn handle_chat_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Action::ScrollUp(1),
        KeyCode::Down | KeyCode::Char('j') => Action::ScrollDown(1),
        KeyCode::PageUp => Action::ScrollUp(20),
        KeyCode::PageDown => Action::ScrollDown(20),
        KeyCode::Home | KeyCode::Char('g') => {
            state.scroll_offset = usize::MAX; // Will be clamped
            state.auto_scroll = false;
            Action::None
        }
        KeyCode::End | KeyCode::Char('G') => Action::ScrollToBottom,
        KeyCode::Tab => Action::FocusInput,
        _ => {
            // Any character typed → switch to input focus
            if let KeyCode::Char(_) = key.code {
                return Action::FocusInput;
            }
            Action::None
        }
    }
}

fn handle_palette_key(state: &mut AppState, textarea: &mut TextArea, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Up => {
            if let Some(p) = &mut state.palette {
                p.selected = p.selected.saturating_sub(1);
            }
            Action::None
        }
        KeyCode::Down => {
            if let Some(p) = &mut state.palette {
                if p.selected + 1 < p.filtered.len() {
                    p.selected += 1;
                }
            }
            Action::None
        }
        KeyCode::Tab | KeyCode::Enter => {
            if let Some(p) = state.palette.take() {
                if let Some((name, _)) = p.filtered.get(p.selected) {
                    textarea.select_all();
                    textarea.delete_char();
                    textarea.insert_str(&format!("/{name} "));
                }
            }
            state.focus = Focus::Input;
            Action::None
        }
        KeyCode::Char(c) => {
            textarea.input(Input { key: Key::Char(c), ..Default::default() });
            // Update filter
            let text: String = textarea.lines().join("");
            let prefix = text.strip_prefix('/').unwrap_or(&text);
            if let Some(p) = &mut state.palette {
                p.filtered = SlashCommand::filter_commands(prefix);
                p.selected = 0;
            }
            Action::None
        }
        KeyCode::Backspace => {
            textarea.input(Input { key: Key::Backspace, ..Default::default() });
            let text: String = textarea.lines().join("");
            if text.is_empty() || !text.starts_with('/') {
                state.close_overlay();
            } else {
                let prefix = text.strip_prefix('/').unwrap_or(&text);
                if let Some(p) = &mut state.palette {
                    p.filtered = SlashCommand::filter_commands(prefix);
                    p.selected = 0;
                }
            }
            Action::None
        }
        _ => Action::None,
    }
}

fn handle_dialog_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let idx = c.to_digit(10).unwrap_or(0) as usize;
            if idx > 0 {
                if let Some(dialog) = &state.dialog {
                    if idx <= dialog.options.len() {
                        let choice = dialog.options[idx - 1].clone();
                        let run_id = dialog.run_id.clone();
                        return Action::RespondToDialog { run_id, choice };
                    }
                }
            }
            Action::None
        }
        KeyCode::Up => {
            if let Some(d) = &mut state.dialog {
                d.selected = d.selected.saturating_sub(1);
            }
            Action::None
        }
        KeyCode::Down => {
            if let Some(d) = &mut state.dialog {
                if d.selected + 1 < d.options.len() {
                    d.selected += 1;
                }
            }
            Action::None
        }
        KeyCode::Enter => {
            if let Some(dialog) = &state.dialog {
                let choice = dialog.options[dialog.selected].clone();
                let run_id = dialog.run_id.clone();
                return Action::RespondToDialog { run_id, choice };
            }
            Action::None
        }
        _ => Action::None,
    }
}

async fn execute_slash_command(state: &mut AppState, client: &AlephClient, cmd: SlashCommand) {
    match cmd {
        // Pure local
        SlashCommand::Clear => state.clear_screen(),
        SlashCommand::Verbose => state.toggle_verbose(),
        SlashCommand::Think { level } => state.set_thinking(level),
        SlashCommand::Quit => state.request_quit(),
        SlashCommand::Help => {
            let help = SlashCommand::all_commands()
                .iter()
                .map(|(name, desc)| format!("  /{name:<12} {desc}"))
                .collect::<Vec<_>>()
                .join("\n");
            state.add_system_message(format!("Available commands:\n{help}"));
        }

        // RPC commands
        SlashCommand::New { name } => {
            match client.call::<_, serde_json::Value>("sessions.create", name.map(|n| serde_json::json!({"name": n}))).await {
                Ok(resp) => {
                    if let Some(key) = resp.get("session_key").and_then(|v| v.as_str()) {
                        state.switch_session(key.to_string());
                    }
                }
                Err(e) => state.add_system_message(format!("Error: {e}")),
            }
        }
        SlashCommand::Session { key } => state.switch_session(key),
        SlashCommand::Sessions => {
            match client.call::<_, serde_json::Value>("sessions.list", None::<()>).await {
                Ok(resp) => {
                    let msg = format!("Sessions: {}", serde_json::to_string_pretty(&resp).unwrap_or_default());
                    state.add_system_message(msg);
                }
                Err(e) => state.add_system_message(format!("Error: {e}")),
            }
        }
        SlashCommand::Delete { key } => {
            match client.call::<_, serde_json::Value>("sessions.delete", Some(serde_json::json!({"session_key": key}))).await {
                Ok(_) => state.add_system_message("Session deleted."),
                Err(e) => state.add_system_message(format!("Error: {e}")),
            }
        }
        SlashCommand::Model { name } => {
            // Attempt to set model via config RPC (graceful if not supported)
            match client.call::<_, serde_json::Value>("config.set", Some(serde_json::json!({"key": "model", "value": &name}))).await {
                Ok(_) => {
                    state.set_model(&name);
                    state.add_system_message(format!("Model: {name}"));
                }
                Err(e) => state.add_system_message(format!("Error: {e}")),
            }
        }
        SlashCommand::Models => {
            match client.call::<_, serde_json::Value>("providers.list", None::<()>).await {
                Ok(resp) => {
                    state.add_system_message(format!("Models: {}", serde_json::to_string_pretty(&resp).unwrap_or_default()));
                }
                Err(e) => state.add_system_message(format!("Error: {e}")),
            }
        }
        SlashCommand::Usage => {
            match client.call::<_, serde_json::Value>("usage.current", Some(serde_json::json!({"session_key": &state.session_key}))).await {
                Ok(resp) => state.add_system_message(format!("Usage: {}", serde_json::to_string_pretty(&resp).unwrap_or_default())),
                Err(_) => state.add_system_message(format!("Token usage: {} total", state.total_tokens)),
            }
        }
        SlashCommand::Status => {
            let mut info = format!("Session: {}\nModel: {}\nTokens: {}\nVerbose: {}\nThinking: {}",
                state.session_key, state.model_name, state.total_tokens, state.verbose, state.thinking_level.as_str());
            match client.call::<_, serde_json::Value>("health", None::<()>).await {
                Ok(resp) => {
                    if let Some(status) = resp.get("status").and_then(|v| v.as_str()) {
                        info.push_str(&format!("\nServer: {status}"));
                    }
                }
                Err(_) => info.push_str("\nServer: disconnected"),
            }
            state.add_system_message(info);
        }
        SlashCommand::Health => {
            match client.call::<_, serde_json::Value>("health", None::<()>).await {
                Ok(resp) => state.add_system_message(format!("Health: {}", serde_json::to_string_pretty(&resp).unwrap_or_default())),
                Err(e) => state.add_system_message(format!("Error: {e}")),
            }
        }
        SlashCommand::Tools { filter } => {
            match client.call::<_, serde_json::Value>("commands.list", None::<()>).await {
                Ok(resp) => state.add_system_message(format!("Tools: {}", serde_json::to_string_pretty(&resp).unwrap_or_default())),
                Err(e) => state.add_system_message(format!("Error: {e}")),
            }
        }
        SlashCommand::Memory { query } => {
            match client.call::<_, serde_json::Value>("memory.search", Some(serde_json::json!({"query": query}))).await {
                Ok(resp) => state.add_system_message(format!("Memory results: {}", serde_json::to_string_pretty(&resp).unwrap_or_default())),
                Err(_) => state.add_system_message("Memory search not available yet."),
            }
        }
        SlashCommand::Compact => {
            match client.call::<_, serde_json::Value>("sessions.compact", Some(serde_json::json!({"session_key": &state.session_key}))).await {
                Ok(_) => state.add_system_message("Session context compacted."),
                Err(_) => state.add_system_message("Compact not available yet."),
            }
        }
    }
}
```

**Step 3: Verify compilation**

Run: `cargo check -p aleph-cli`
Expected: Compiles. May have warnings about unused imports — fix them.

**Step 4: Commit**

```bash
git add apps/cli/src/tui/
git commit -m "cli: implement TUI main loop with render, event handling, and slash command execution"
```

---

## Task 7: Wire chat.rs to Launch TUI

**Files:**
- Modify: `apps/cli/src/commands/chat.rs`
- Modify: `apps/cli/src/main.rs`

**Step 1: Rewrite chat.rs to launch TUI**

Replace the entire `run()` function body in `apps/cli/src/commands/chat.rs`:

```rust
use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

pub async fn run(
    server_url: &str,
    session: Option<&str>,
    config: &CliConfig,
) -> CliResult<()> {
    // Connect to gateway
    let (client, events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    // Determine session key
    let session_key = match session {
        Some(s) => s.to_string(),
        None => config.default_session.clone().unwrap_or_else(|| {
            format!("chat-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("0000"))
        }),
    };

    // Launch TUI
    let result = crate::tui::run(client, events, config, session_key).await;

    result
}
```

**Step 2: Update main.rs module declaration**

In `apps/cli/src/main.rs`, replace `mod ui;` with `mod tui;` (if not done in Task 1).

Verify the Chat command dispatch (around line 165) still calls `commands::chat::run()` — it should, since we only changed the function body.

**Step 3: Build the full binary**

Run: `cargo build -p aleph-cli`
Expected: Compiles successfully.

**Step 4: Smoke test (if gateway is running)**

Run: `cargo run -p aleph-cli -- chat`
Expected: TUI launches with split-screen layout. Can type text, see status bar. Ctrl+D exits.

If gateway is not running, the connection error should display cleanly.

**Step 5: Commit**

```bash
git add apps/cli/src/commands/chat.rs apps/cli/src/main.rs
git commit -m "cli: wire chat command to launch ratatui TUI instead of stdin REPL"
```

---

## Task 8: Polish & Integration Testing

**Files:**
- Various files in `apps/cli/src/tui/`

**Step 1: Fix compilation warnings**

Run: `cargo clippy -p aleph-cli -- -W warnings`
Fix all warnings (unused imports, unreachable patterns, etc.).

**Step 2: Run all existing tests**

Run: `cargo test -p aleph-cli --lib`
Expected: All tests pass (slash parser + app state + markdown + existing guest tests).

**Step 3: Test terminal restore on panic**

Ensure that if the TUI panics, the terminal is restored. Add a panic hook in `tui::run()`:

```rust
let original_hook = std::panic::take_hook();
std::panic::set_hook(Box::new(move |panic_info| {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    original_hook(panic_info);
}));
```

**Step 4: Test ask command still works**

Run: `cargo run -p aleph-cli -- ask "hello"` (with or without gateway)
Expected: One-shot mode unchanged, no TUI launched.

**Step 5: Final commit**

```bash
git add apps/cli/
git commit -m "cli: polish TUI — clippy fixes, panic recovery, terminal restore"
```

---

## Summary

| Task | Files | Estimated Lines | Description |
|------|-------|-----------------|-------------|
| 1 | Cargo.toml, tui/mod.rs, theme.rs | ~100 | Dependencies + skeleton |
| 2 | tui/slash.rs | ~150 | Slash command parser |
| 3 | tui/app.rs, tui/event.rs | ~380 | AppState, Action, events |
| 4 | tui/markdown.rs | ~250 | Markdown renderer |
| 5 | tui/widgets/*.rs (6 files) | ~550 | All widgets |
| 6 | tui/render.rs, tui/mod.rs | ~430 | Main loop + render |
| 7 | commands/chat.rs, main.rs | ~30 | Wire to TUI |
| 8 | Various | ~20 | Polish |
| **Total** | **~14 files** | **~1,910** | |

Task dependency chain: `1 → 2 → 3 → 4 → 5 → 6 → 7 → 8` (sequential, each builds on previous).
