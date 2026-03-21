// TUI module: full-screen terminal UI for interactive chat.
//
// Provides the main event loop that integrates terminal events (keyboard, resize),
// gateway events (streaming responses, tool updates), and a 50ms tick for spinner
// animation. All rendering is delegated to render.rs, which splits the layout
// into chat area, input area, status bar, and overlays.

mod app;
mod command_tree;
mod event;
mod markdown;
mod render;
mod slash;
mod theme;
mod widgets;

use std::io;
use std::time::Duration;

use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tui_textarea::{Input, TextArea};

use aleph_protocol::StreamEvent;

use aleph_client::{AlephClient, CliConfig, CliResult};

use app::{Action, AppState, Focus};
use slash::{LocalCommand, ParsedInput};

/// Entry point: run the TUI application.
///
/// Sets up the terminal, spawns the event collector, and runs the main loop
/// until the user quits. Terminal is always restored on exit (including panics).
pub async fn run(
    client: AlephClient,
    mut gateway_events: mpsc::Receiver<StreamEvent>,
    _config: &CliConfig,
    session_key: String,
) -> CliResult<()> {
    // 1. Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 2. Set panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = execute!(io::stdout(), crossterm::cursor::Show);
        original_hook(info);
    }));

    // 3. Fetch model name from gateway, then create AppState and TextArea
    let model_name = match client.call::<_, Value>("models.list", None::<()>).await {
        Ok(result) => {
            // Try array format: [{"name": "..."}, ...] or ["model-name", ...]
            result
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|first| {
                    first
                        .as_str()
                        .map(|s| s.to_string())
                        .or_else(|| {
                            first
                                .get("name")
                                .or_else(|| first.get("id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                })
                // Also try object format: {"models": [...]}
                .or_else(|| {
                    result
                        .get("models")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|first| {
                            first
                                .as_str()
                                .map(|s| s.to_string())
                                .or_else(|| {
                                    first
                                        .get("name")
                                        .or_else(|| first.get("id"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string())
                                })
                        })
                })
                .unwrap_or_else(|| "unknown".to_string())
        }
        Err(_) => "unknown".to_string(),
    };

    // 3b. Fetch gateway commands for command palette
    let gateway_commands = fetch_gateway_commands(&client).await;

    let mut state = AppState::new(session_key, model_name);
    state.gateway_commands = gateway_commands;
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("Type a message... (/ for commands)");

    // 4. Spawn event collector
    let mut term_events = event::spawn_event_collector();

    // 5. Main loop
    let mut tick_interval = tokio::time::interval(Duration::from_millis(50));

    let result = main_loop(
        &mut terminal,
        &mut state,
        &mut textarea,
        &client,
        &mut gateway_events,
        &mut term_events,
        &mut tick_interval,
    )
    .await;

    // 6. Restore terminal (always, even on error)
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// The main event loop. Separated from `run()` so terminal restoration
/// happens even if this function returns an error.
async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    textarea: &mut TextArea<'_>,
    client: &AlephClient,
    gateway_events: &mut mpsc::Receiver<StreamEvent>,
    term_events: &mut mpsc::Receiver<event::TermEvent>,
    tick_interval: &mut tokio::time::Interval,
) -> CliResult<()> {
    loop {
        // Draw
        terminal.draw(|f| render::render(f, state, textarea))?;

        // Wait for next event
        let action = tokio::select! {
            Some(te) = term_events.recv() => {
                handle_terminal_event(state, textarea, te)
            }
            Some(ge) = gateway_events.recv() => {
                state.handle_gateway_event(ge)
            }
            _ = tick_interval.tick() => {
                Action::Tick
            }
        };

        // Execute action
        match action {
            Action::None => {}
            Action::Quit => {
                break;
            }
            Action::Tick => {
                state.spinner_frame = state.spinner_frame.wrapping_add(1);
            }

            // -- Chat --
            Action::SendMessage(msg) => {
                state.add_user_message(msg.clone());
                state.ctrl_c_count = 0;

                // Save to input history
                if !msg.is_empty() {
                    state.send_history.push(msg.clone());
                    state.history_index = None;
                }

                // Send via RPC
                let params = json!({
                    "session_key": state.session_key,
                    "message": msg,
                });
                match client.call::<_, Value>("agent.run", Some(params)).await {
                    Ok(_) => {}
                    Err(e) => {
                        state.add_system_message(format!("Send error: {}", e));
                    }
                }
            }
            Action::LocalCommand(cmd) => {
                execute_local_command(state, textarea, cmd);
            }
            Action::GatewayCommand(text) => {
                // Send slash command to Gateway as a regular message
                state.add_user_message(text.clone());
                state.ctrl_c_count = 0;

                if !text.is_empty() {
                    state.send_history.push(text.clone());
                    state.history_index = None;
                }

                let params = json!({
                    "session_key": state.session_key,
                    "message": text,
                });
                match client.call::<_, Value>("agent.run", Some(params)).await {
                    Ok(_) => {}
                    Err(e) => {
                        state.add_system_message(format!("Command error: {}", e));
                    }
                }
            }
            Action::CancelRun(run_id) => {
                let params = json!({ "run_id": run_id });
                match client.call::<_, Value>("agent.cancel", Some(params)).await {
                    Ok(_) => {
                        state.add_system_message("Run cancelled.".to_string());
                        state.current_run = None;
                    }
                    Err(e) => {
                        state.add_system_message(format!("Cancel error: {}", e));
                    }
                }
            }

            // -- Scrolling --
            Action::ScrollUp(n) => state.scroll_up(n),
            Action::ScrollDown(n) => state.scroll_down(n),
            Action::ScrollToBottom => state.scroll_to_bottom(),
            Action::ScrollToBottomIfAutoScroll => {
                if state.auto_scroll {
                    state.scroll_to_bottom();
                }
            }

            // -- Focus --
            Action::FocusInput => {
                state.focus = Focus::Input;
            }
            Action::FocusChat => {
                state.focus = Focus::Chat;
            }

            // -- Overlays --
            Action::OpenCommandPalette => {
                state.open_command_palette();
            }
            Action::CloseOverlay => {
                state.close_overlay();
            }
            Action::PaletteUp => {
                if let Some(palette) = &mut state.palette {
                    if palette.selected > 0 {
                        palette.selected -= 1;
                    }
                }
            }
            Action::PaletteDown => {
                if let Some(palette) = &mut state.palette {
                    if palette.selected + 1 < palette.filtered.len() {
                        palette.selected += 1;
                    }
                }
            }
            Action::PaletteConfirm => {
                if let Some(palette) = state.palette.take() {
                    if let Some(entry) = palette.filtered.get(palette.selected) {
                        let cmd_str = entry.full_command.trim().to_string();
                        state.close_overlay();
                        // Parse through our unified parser
                        match slash::parse_input(&cmd_str) {
                            ParsedInput::Local(cmd) => {
                                execute_local_command(state, textarea, cmd);
                            }
                            ParsedInput::Gateway(text) => {
                                // Send gateway command as chat message
                                state.add_user_message(text.clone());
                                let params = json!({
                                    "session_key": state.session_key,
                                    "message": text,
                                });
                                match client.call::<_, Value>("agent.run", Some(params)).await {
                                    Ok(_) => {}
                                    Err(e) => {
                                        state.add_system_message(format!("Command error: {}", e));
                                    }
                                }
                            }
                            ParsedInput::NotSlashCommand => {
                                // Shouldn't happen from palette, but handle gracefully
                            }
                        }
                    } else {
                        state.close_overlay();
                    }
                }
            }

            // -- Dialog --
            Action::DialogSelect(idx) => {
                if let Some(dialog) = &mut state.dialog {
                    if idx < dialog.options.len() {
                        dialog.selected = idx;
                    }
                }
            }
            Action::RespondToDialog { run_id, choice } => {
                let params = json!({
                    "run_id": run_id,
                    "response": choice,
                });
                match client
                    .call::<_, Value>("agent.respondToInput", Some(params))
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        state.add_system_message(format!("Dialog response error: {}", e));
                    }
                }
                state.close_overlay();
            }

        }

        // Check quit flag
        if state.should_quit {
            break;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Terminal event handling
// ---------------------------------------------------------------------------

/// Route a terminal event to an Action based on current focus.
fn handle_terminal_event(
    state: &mut AppState,
    textarea: &mut TextArea,
    event: event::TermEvent,
) -> Action {
    match event {
        event::TermEvent::Key(key) => handle_key_event(state, textarea, key),
        event::TermEvent::Resize => {
            // Terminal resize is handled automatically by ratatui
            Action::None
        }
    }
}

/// Route a key event to an Action based on current focus.
fn handle_key_event(state: &mut AppState, textarea: &mut TextArea, key: KeyEvent) -> Action {
    // Global keys (work in all focus modes)
    if let Some(action) = handle_global_key(state, textarea, &key) {
        return action;
    }

    // Focus-specific handling
    match state.focus {
        Focus::Input => handle_input_key(state, textarea, key),
        Focus::Chat => handle_chat_key(state, key),
        Focus::CommandPalette => handle_palette_key(state, key),
        Focus::Dialog => handle_dialog_key(state, key),
    }
}

/// Handle global key bindings that work regardless of focus.
/// Returns Some(Action) if the key was handled, None to delegate to focus handler.
fn handle_global_key(
    state: &mut AppState,
    textarea: &mut TextArea,
    key: &KeyEvent,
) -> Option<Action> {
    // Ctrl+C: smart cascade
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        // If there's an active run, cancel it
        if let Some(run_id) = state.current_run.clone() {
            state.ctrl_c_count = 0;
            return Some(Action::CancelRun(run_id));
        }

        // If input has content, clear it
        let has_content = textarea.lines().iter().any(|line| !line.is_empty());
        if has_content {
            textarea.select_all();
            textarea.delete_char();
            state.ctrl_c_count = 0;
            return Some(Action::None);
        }

        // Otherwise, increment counter and maybe quit
        state.ctrl_c_count += 1;
        if state.ctrl_c_count >= 2 {
            return Some(Action::Quit);
        }
        state.add_system_message("Press Ctrl+C again to quit.".to_string());
        return Some(Action::None);
    }

    // Reset Ctrl+C counter on any other key
    if key.code != KeyCode::Char('c') || !key.modifiers.contains(KeyModifiers::CONTROL) {
        state.ctrl_c_count = 0;
    }

    // Ctrl+D: quit immediately
    if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Action::Quit);
    }

    // Esc: close overlay if any is open
    if key.code == KeyCode::Esc {
        if state.palette.is_some() || state.dialog.is_some() {
            return Some(Action::CloseOverlay);
        }
        // If in chat focus, return to input
        if state.focus == Focus::Chat {
            return Some(Action::FocusInput);
        }
    }

    // F1: help
    if key.code == KeyCode::F(1) {
        return Some(Action::LocalCommand(LocalCommand::Help));
    }

    None
}

/// Handle key events when the input area is focused.
fn handle_input_key(state: &mut AppState, textarea: &mut TextArea, key: KeyEvent) -> Action {
    match key.code {
        // Enter: send message (unless Shift is held for newline)
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                // Shift+Enter: insert newline
                textarea.input(Input::from(crossterm::event::Event::Key(key)));
                Action::None
            } else {
                // Enter: collect text and send
                let text: String = textarea.lines().join("\n");
                let text = text.trim().to_string();

                if text.is_empty() {
                    return Action::None;
                }

                // Clear the textarea
                textarea.select_all();
                textarea.delete_char();

                // Check if it's a slash command
                match slash::parse_input(&text) {
                    ParsedInput::Local(cmd) => Action::LocalCommand(cmd),
                    ParsedInput::Gateway(cmd_text) => Action::GatewayCommand(cmd_text),
                    ParsedInput::NotSlashCommand => Action::SendMessage(text),
                }
            }
        }

        // Up arrow: browse history or focus chat
        KeyCode::Up => {
            // If the textarea has only one line and it's empty (or matches history),
            // browse input history
            let lines = textarea.lines();
            if lines.len() <= 1 {
                let current_text = lines.first().map(|s| s.as_str()).unwrap_or("");

                if state.send_history.is_empty() {
                    return Action::FocusChat;
                }

                let next_index = match state.history_index {
                    None => {
                        if current_text.is_empty() {
                            Some(state.send_history.len() - 1)
                        } else {
                            return Action::FocusChat;
                        }
                    }
                    Some(idx) => {
                        if idx > 0 {
                            Some(idx - 1)
                        } else {
                            Some(0)
                        }
                    }
                };

                if let Some(idx) = next_index {
                    state.history_index = Some(idx);
                    let history_text = state.send_history[idx].clone();
                    textarea.select_all();
                    textarea.delete_char();
                    textarea.insert_str(&history_text);
                }

                Action::None
            } else {
                // Multi-line: let textarea handle cursor movement
                textarea.input(Input::from(crossterm::event::Event::Key(key)));
                Action::None
            }
        }

        // Down arrow: browse history forward
        KeyCode::Down => {
            let lines = textarea.lines();
            if lines.len() <= 1 {
                if let Some(idx) = state.history_index {
                    if idx + 1 < state.send_history.len() {
                        state.history_index = Some(idx + 1);
                        let history_text = state.send_history[idx + 1].clone();
                        textarea.select_all();
                        textarea.delete_char();
                        textarea.insert_str(&history_text);
                    } else {
                        // Past the end of history, clear
                        state.history_index = None;
                        textarea.select_all();
                        textarea.delete_char();
                    }
                }
                Action::None
            } else {
                textarea.input(Input::from(crossterm::event::Event::Key(key)));
                Action::None
            }
        }

        // Tab: cycle focus
        KeyCode::Tab => Action::FocusChat,

        // '/' at beginning of empty line: open command palette
        KeyCode::Char('/') => {
            let is_empty = textarea.lines().iter().all(|line| line.is_empty());
            if is_empty {
                Action::OpenCommandPalette
            } else {
                textarea.input(Input::from(crossterm::event::Event::Key(key)));
                Action::None
            }
        }

        // All other keys: forward to textarea
        _ => {
            textarea.input(Input::from(crossterm::event::Event::Key(key)));
            Action::None
        }
    }
}

/// Handle key events when the chat panel is focused.
fn handle_chat_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        // Scroll up
        KeyCode::Up | KeyCode::Char('k') => Action::ScrollUp(1),
        // Scroll down
        KeyCode::Down | KeyCode::Char('j') => Action::ScrollDown(1),
        // Page up
        KeyCode::PageUp => Action::ScrollUp(20),
        // Page down
        KeyCode::PageDown => Action::ScrollDown(20),
        // Home: scroll to top (large offset)
        KeyCode::Home => Action::ScrollUp(usize::MAX / 2),
        // End: jump to bottom
        KeyCode::End => Action::ScrollToBottom,
        // Tab: return to input
        KeyCode::Tab => Action::FocusInput,
        // Any printable char: switch to input and let user type
        KeyCode::Char(c) => {
            // Don't steal j/k which we handle above
            if c != 'j' && c != 'k' {
                state.focus = Focus::Input;
                Action::FocusInput
            } else {
                Action::None
            }
        }
        _ => Action::None,
    }
}

/// Handle key events when the command palette is focused.
fn handle_palette_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Up => Action::PaletteUp,
        KeyCode::Down => Action::PaletteDown,
        KeyCode::Tab | KeyCode::Enter => {
            // Check if the selected item is a namespace — if so, enter it
            let selected_entry = state
                .palette
                .as_ref()
                .and_then(|p| p.filtered.get(p.selected).cloned());

            if let Some(entry) = selected_entry {
                if entry.is_namespace {
                    // Extract the namespace name from the full_command (e.g. "/session" -> "session")
                    let ns_name = entry
                        .full_command
                        .trim_start_matches('/')
                        .trim()
                        .to_string();
                    state.palette_enter_namespace(&ns_name);
                    return Action::None;
                }
            }
            // Not a namespace — confirm selection
            Action::PaletteConfirm
        }
        KeyCode::Backspace => {
            let is_empty = state
                .palette
                .as_ref()
                .map(|p| p.input.is_empty())
                .unwrap_or(true);
            if is_empty {
                // If inside a namespace, go back one level
                if state.palette_go_back() {
                    return Action::None;
                }
                // At root with empty input — close palette
                Action::CloseOverlay
            } else {
                if let Some(palette) = &mut state.palette {
                    palette.input.pop();
                }
                // Recompute filtered list
                recompute_palette_filter(state);
                Action::None
            }
        }
        KeyCode::Char(' ') => {
            // Space after a namespace name at root level enters it
            if let Some(palette) = &state.palette {
                if palette.namespace_stack.is_empty() {
                    let input = palette.input.clone();
                    if let Some(ns) = command_tree::CommandEntry::find_namespace(
                        &state.gateway_commands,
                        &input,
                    ) {
                        let ns_name = ns.name.clone();
                        state.palette_enter_namespace(&ns_name);
                        return Action::None;
                    }
                }
            }
            // Normal space character in filter
            if let Some(palette) = &mut state.palette {
                palette.input.push(' ');
            }
            recompute_palette_filter(state);
            Action::None
        }
        KeyCode::Char(c) => {
            if let Some(palette) = &mut state.palette {
                palette.input.push(c);
            }
            recompute_palette_filter(state);
            Action::None
        }
        _ => Action::None,
    }
}

/// Recompute the filtered display entries based on current palette input and namespace stack.
fn recompute_palette_filter(state: &mut AppState) {
    let (stack, filter) = match &state.palette {
        Some(p) => (p.namespace_stack.clone(), p.input.clone()),
        None => return,
    };
    let filtered = state.filter_display_entries(&stack, &filter);
    if let Some(palette) = &mut state.palette {
        palette.filtered = filtered;
        palette.selected = 0;
    }
}

/// Handle key events when the dialog is focused.
fn handle_dialog_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        // Number keys for quick select (1-9)
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let idx = (c as usize) - ('1' as usize);
            if let Some(dialog) = &state.dialog {
                if idx < dialog.options.len() {
                    let run_id = dialog.run_id.clone();
                    let choice = dialog.options[idx].clone();
                    return Action::RespondToDialog { run_id, choice };
                }
            }
            Action::DialogSelect(idx)
        }
        KeyCode::Up => {
            if let Some(dialog) = &mut state.dialog {
                if dialog.selected > 0 {
                    dialog.selected -= 1;
                }
            }
            Action::None
        }
        KeyCode::Down => {
            if let Some(dialog) = &mut state.dialog {
                if dialog.selected + 1 < dialog.options.len() {
                    dialog.selected += 1;
                }
            }
            Action::None
        }
        KeyCode::Enter => {
            if let Some(dialog) = &state.dialog {
                let run_id = dialog.run_id.clone();
                let choice = dialog.options[dialog.selected].clone();
                Action::RespondToDialog { run_id, choice }
            } else {
                Action::None
            }
        }
        _ => Action::None,
    }
}

// ---------------------------------------------------------------------------
// Local command execution
// ---------------------------------------------------------------------------

/// Execute a local slash command (TUI-only, no Gateway RPC needed).
fn execute_local_command(
    state: &mut AppState,
    textarea: &mut TextArea<'_>,
    cmd: LocalCommand,
) {
    match cmd {
        LocalCommand::Clear => {
            state.clear_screen();
        }
        LocalCommand::Verbose => {
            state.toggle_verbose();
            let mode = if state.verbose { "on" } else { "off" };
            state.add_system_message(format!("Verbose mode: {}", mode));
        }
        LocalCommand::Quit => {
            state.request_quit();
        }
        LocalCommand::Help => {
            let help_text = build_help_text(state);
            state.add_system_message(help_text);
        }
    }

    // Ensure textarea still has focus hint after command execution
    let _ = textarea;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the help text shown by /help.
fn build_help_text(state: &AppState) -> String {
    let mut lines = vec!["Local commands:".to_string()];
    for (name, desc) in slash::local_commands() {
        lines.push(format!("  {:<14} {}", name, desc));
    }

    if !state.gateway_commands.is_empty() {
        lines.push(String::new());
        lines.push("Gateway commands (handled by server):".to_string());
        for entry in &state.gateway_commands {
            if entry.is_namespace {
                let param = entry
                    .param_hint
                    .as_deref()
                    .map(|p| format!(" {}", p))
                    .unwrap_or_default();
                lines.push(format!(
                    "  /{:<13} {} (namespace)",
                    format!("{}{}", entry.name, param),
                    entry.hint,
                ));
                for child in &entry.children {
                    let child_param = child
                        .param_hint
                        .as_deref()
                        .map(|p| format!(" {}", p))
                        .unwrap_or_default();
                    lines.push(format!(
                        "    /{} {}{:<6} {}",
                        entry.name, child.name, child_param, child.hint,
                    ));
                }
            } else {
                let param = entry
                    .param_hint
                    .as_deref()
                    .map(|p| format!(" {}", p))
                    .unwrap_or_default();
                lines.push(format!(
                    "  /{:<13} {}",
                    format!("{}{}", entry.name, param),
                    entry.hint,
                ));
            }
        }
    }

    lines.push(String::new());
    lines.push("Other slash commands are forwarded to the Gateway as chat messages.".to_string());
    lines.push(String::new());
    lines.push("Keyboard shortcuts:".to_string());
    lines.push("  Enter          Send message".to_string());
    lines.push("  Shift+Enter    Insert newline".to_string());
    lines.push("  Ctrl+C         Cancel run / Clear input / Quit".to_string());
    lines.push("  Ctrl+D         Quit immediately".to_string());
    lines.push("  Tab            Switch focus (Input <-> Chat)".to_string());
    lines.push("  Up/Down        Scroll chat or browse history".to_string());
    lines.push("  /              Open command palette".to_string());
    lines.push("  F1             Show this help".to_string());
    lines.join("\n")
}

/// Fetch available commands from the Gateway for command palette display.
/// Gracefully degrades to empty list if the Gateway doesn't support commands.list.
/// Parses tree-structured command responses with namespace support.
async fn fetch_gateway_commands(client: &AlephClient) -> Vec<command_tree::CommandEntry> {
    match client
        .call::<_, Value>("commands.list", Some(json!({"interface": "tui"})))
        .await
    {
        Ok(result) => command_tree::CommandEntry::parse_from_json(&result),
        Err(_) => {
            // Old Gateway or connection issue — graceful degradation
            Vec::new()
        }
    }
}

