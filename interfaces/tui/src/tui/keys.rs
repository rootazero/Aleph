// Terminal/key event handling: maps a `TermEvent` (and, within it, a `KeyEvent`
// plus the current `Focus`) to an `Action` for the main loop to execute.
//
// Extracted from `mod.rs` so the orchestrator file keeps only `run()` +
// `main_loop`. This is the single home for "terminal event -> Action" routing;
// the `event.rs::map_event` gate (KeyEventKind::Press, paste mapping) sits one
// layer earlier and must not be duplicated here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{Input, TextArea};

use super::app::{Action, AppState, Focus, APPROVAL_DECISIONS};
use super::command_tree;
use super::event;
use super::slash::{self, LocalCommand, ParsedInput};

/// Route a terminal event to an Action based on current focus.
pub(super) fn handle_terminal_event(
    state: &mut AppState,
    textarea: &mut TextArea,
    event: &event::TermEvent,
) -> Action {
    match event {
        event::TermEvent::Key(key) => handle_key_event(state, textarea, *key),
        event::TermEvent::Resize => {
            // Terminal resize is handled automatically by ratatui
            Action::None
        }
        event::TermEvent::Paste(text) => {
            // Insert a bracketed paste verbatim (multi-line safe) — never route
            // it through the Enter/send path, so a multi-line paste no longer
            // auto-sends its first line.
            textarea.insert_str(text);
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
        Focus::SessionPicker => handle_session_picker_key(state, key),
        Focus::Approval => handle_approval_key(state, key),
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

    // Esc: close the command palette (a purely local overlay). Do NOT dismiss an
    // AskUser dialog — it is backed by a server run parked on a oneshot, so
    // silently closing it would orphan that run with no response. Keep the
    // dialog on screen and force the user to answer (or /stop / Ctrl+C to abort).
    if key.code == KeyCode::Esc {
        if state.palette.is_some() || state.session_picker.is_some() {
            return Some(Action::CloseOverlay);
        }
        if state.dialog.is_some() {
            return Some(Action::None);
        }
        // Same for a tool-approval overlay: the run is parked on a decision.
        // Esc must not orphan it — Deny is the safe way out.
        if state.approval.is_some() {
            return Some(Action::None);
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
        // Ctrl+J: portable newline. Enhanced terminals deliver this as a
        // distinct Char('j')+CONTROL; on terminals that collapse it to a bare
        // Enter it simply behaves like Enter (harmless). The always-works
        // newline is the `\`+Enter continuation handled in the Enter arm below.
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            textarea.insert_newline();
            Action::None
        }

        // Enter: insert a newline (Shift/Ctrl held, or `\`-continuation) or send.
        KeyCode::Enter => {
            // Enhanced terminals expose Shift+Enter / Ctrl+Enter as a distinct
            // modifier — treat either as an explicit newline.
            if key.modifiers.contains(KeyModifiers::SHIFT)
                || key.modifiers.contains(KeyModifiers::CONTROL)
            {
                textarea.insert_newline();
                return Action::None;
            }

            // Portable newline for terminals that collapse every Enter to a bare
            // '\r' (Windows Terminal, WSL, plain Terminal.app, SSH): a trailing
            // backslash immediately left of the cursor becomes a newline instead
            // of submitting. Mirrors the Claude Code / hermes-agent convention.
            let (row, col) = textarea.cursor();
            let ends_with_backslash = col > 0
                && textarea
                    .lines()
                    .get(row)
                    .and_then(|line| line.chars().nth(col - 1))
                    == Some('\\');
            if ends_with_backslash {
                textarea.delete_char(); // remove the trailing '\'
                textarea.insert_newline();
                return Action::None;
            }

            // Otherwise: collect text and send.
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

        // Up arrow: browse history or focus chat
        KeyCode::Up => {
            let lines = textarea.lines();
            if lines.len() > 1 {
                // Multi-line: let textarea handle cursor movement
                textarea.input(Input::from(crossterm::event::Event::Key(key)));
                return Action::None;
            }

            // Single-line: browse input history
            let current_text = lines.first().map_or("", std::string::String::as_str);

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
                let history_text = state.send_history.get(idx).cloned().unwrap_or_default();
                textarea.select_all();
                textarea.delete_char();
                textarea.insert_str(&history_text);
            }

            Action::None
        }

        // Down arrow: browse history forward
        KeyCode::Down => {
            let lines = textarea.lines();
            if lines.len() > 1 {
                textarea.input(Input::from(crossterm::event::Event::Key(key)));
                return Action::None;
            }

            if let Some(idx) = state.history_index {
                if idx + 1 < state.send_history.len() {
                    state.history_index = Some(idx + 1);
                    let history_text = state.send_history.get(idx + 1).cloned().unwrap_or_default();
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
        }

        // Tab: cycle focus
        KeyCode::Tab => Action::FocusChat,

        // '/' at beginning of empty line: open command palette
        KeyCode::Char('/') => {
            let is_empty = textarea.lines().iter().all(std::string::String::is_empty);
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
const fn handle_chat_key(state: &mut AppState, key: KeyEvent) -> Action {
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
            let is_empty = state.palette.as_ref().is_none_or(|p| p.input.is_empty());
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
                    if let Some(ns) =
                        command_tree::CommandEntry::find_namespace(&state.gateway_commands, &input)
                    {
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
                    let session_key = dialog.session_key.clone();
                    let reply = dialog.options[idx].clone();
                    return Action::RespondToDialog { session_key, reply };
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
            let Some(dialog) = &state.dialog else {
                return Action::None;
            };
            dialog
                .options
                .get(dialog.selected)
                .map_or(Action::None, |choice| Action::RespondToDialog {
                    session_key: dialog.session_key.clone(),
                    reply: choice.clone(),
                })
        }
        _ => Action::None,
    }
}

/// Handle key events when the tool-approval overlay is focused.
///
/// The only exits are the three decisions — there is no dismiss, because a
/// parked Ask-tier run must be resolved (Deny is the safe way out). Mirrors
/// `handle_dialog_key`'s number/arrow/Enter scheme; number keys resolve
/// immediately, arrows move the highlight, Enter confirms the highlight.
fn handle_approval_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char(c) if ('1'..='3').contains(&c) => Action::ResolveApproval {
            index: (c as usize) - ('1' as usize),
        },
        KeyCode::Up => {
            if let Some(approval) = &mut state.approval {
                if approval.selected > 0 {
                    approval.selected -= 1;
                }
            }
            Action::None
        }
        KeyCode::Down => {
            if let Some(approval) = &mut state.approval {
                if approval.selected + 1 < APPROVAL_DECISIONS.len() {
                    approval.selected += 1;
                }
            }
            Action::None
        }
        KeyCode::Enter => {
            state
                .approval
                .as_ref()
                .map_or(Action::None, |approval| Action::ResolveApproval {
                    index: approval.selected,
                })
        }
        _ => Action::None,
    }
}

/// Handle key events when the session picker is focused.
fn handle_session_picker_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Up => Action::SessionPickerUp,
        KeyCode::Down => Action::SessionPickerDown,
        KeyCode::Enter | KeyCode::Tab => Action::SessionPickerConfirm,
        KeyCode::Backspace => {
            let is_empty = state
                .session_picker
                .as_ref()
                .is_none_or(|p| p.input.is_empty());
            if is_empty {
                // Empty filter — close the picker.
                Action::CloseOverlay
            } else {
                if let Some(picker) = &mut state.session_picker {
                    picker.input.pop();
                }
                state.recompute_session_filter();
                Action::None
            }
        }
        KeyCode::Char(c) => {
            if let Some(picker) = &mut state.session_picker {
                picker.input.push(c);
            }
            state.recompute_session_filter();
            Action::None
        }
        _ => Action::None,
    }
}
