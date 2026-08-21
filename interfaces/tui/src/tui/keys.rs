// Terminal/key event handling: maps a `TermEvent` (and, within it, a `KeyEvent`
// plus the current `Focus`) to an `Action` for the main loop to execute.
//
// Extracted from `mod.rs` so the orchestrator file keeps only `run()` +
// `main_loop`. This is the single home for "terminal event -> Action" routing;
// the `event.rs::map_event` gate (KeyEventKind::Press, paste mapping) sits one
// layer earlier and must not be duplicated here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{Input, TextArea};

use super::app::{Action, AppState, Focus};
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
        Focus::ProviderPicker => handle_provider_picker_key(state, key),
        Focus::Approval => handle_approval_key(state, key),
        Focus::Btw => handle_btw_key(state, key),
    }
}

/// Handle key events when the `/btw` side-question overlay is focused.
///
/// # Why there are two modes at all
///
/// The overlay owes the user two incompatible things at once: single-letter
/// shortcuts (`c` copy, `p` promote) and a free-text follow-up. A flat key
/// table cannot have both — bind `c` to copy and no follow-up can begin with
/// the letter c. So `composing` decides, and Tab toggles it explicitly in both
/// directions.
///
/// It is deliberately NOT derived from `composer.is_empty()`. That is the
/// shape `DialogState::typing` documents as a defect: clearing the buffer
/// would silently drop the user back into a mode where the next letter they
/// type means something else entirely. Typing an ordinary character in browse
/// mode does flip *into* composing (so "just start typing" works), but nothing
/// ever flips back on its own.
fn handle_btw_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        // Abort while it is answering, close when it is idle. The two are one
        // key because they are one intent — "I am done with this" — and the
        // overlay knows which of them applies.
        KeyCode::Esc => Action::BtwAbortOrClose,

        // Page history. Not text even in compose mode: the composer is a
        // single line the user only ever appends to and backspaces, so
        // horizontal cursor movement has nothing to do, and losing the pager
        // in the mode where you are most likely to want to re-read the answer
        // you are replying to would be the worse trade.
        KeyCode::Left => {
            state.btw.page_left();
            Action::None
        }
        KeyCode::Right => {
            state.btw.page_right();
            Action::None
        }
        KeyCode::Up => {
            state.btw.scroll_up(1);
            Action::None
        }
        KeyCode::Down => {
            state.btw.scroll_down(1);
            Action::None
        }
        KeyCode::PageUp => {
            state.btw.scroll_up(10);
            Action::None
        }
        KeyCode::PageDown => {
            state.btw.scroll_down(10);
            Action::None
        }

        KeyCode::Tab => {
            state.btw.composing = !state.btw.composing;
            Action::None
        }

        KeyCode::Enter => {
            let body = state.btw.composer.trim().to_string();
            if body.is_empty() {
                // Nothing to send. Put them where typing works rather than
                // doing nothing silently.
                state.btw.composing = true;
                return Action::None;
            }
            state.btw.composer.clear();
            // The composer holds a question body, not a command line — so the
            // `/btw` is constructed here rather than tested for. Resolving
            // whether the text "is already a btw" would be a second copy of a
            // predicate this client answers in exactly one place
            // (`commands::dispatch_gateway_text`).
            Action::GatewayCommand(format!("/btw {body}"))
        }

        KeyCode::Backspace if state.btw.composing => {
            state.btw.composer.pop();
            Action::None
        }

        KeyCode::Char(c) if state.btw.composing => {
            state.btw.composer.push(c);
            Action::None
        }
        KeyCode::Char('c') => Action::BtwCopy,
        // Promotion is a request to move this answer INTO the main
        // conversation, which is a boundary crossing the user has to ask for
        // out loud. The key does exactly what typing it would: it sends
        // `/btw promote`. Nothing about what the server then does with it is
        // decided here.
        KeyCode::Char('p') => Action::GatewayCommand("/btw promote".to_string()),
        KeyCode::Char(c) => {
            state.btw.composing = true;
            state.btw.composer.push(c);
            Action::None
        }

        _ => Action::None,
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
        // The session and provider pickers are local too: nothing on the server
        // is parked on either, and neither has sent anything until Enter.
        if state.palette.is_some()
            || state.session_picker.is_some()
            || state.provider_picker.is_some()
        {
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

/// Recompute the filtered display entries — and the argument tail — from the
/// current palette input and namespace stack.
///
/// The one place that decides whether a word the user typed is a search term
/// or an argument. The confirm path reads the answer off `palette.args` rather
/// than splitting the input a second time, so the list on screen and the
/// command that runs can never disagree about it.
fn recompute_palette_filter(state: &mut AppState) {
    let (stack, input) = match &state.palette {
        Some(p) => (p.namespace_stack.clone(), p.input.clone()),
        None => return,
    };
    let (head, tail) = command_tree::split_palette_input(&input);
    let mut filtered = state.filter_display_entries(&stack, head);
    let mut args = tail.to_string();
    // Fallback: the head narrowed to nothing, so it was not a command name —
    // treat the whole string as one search term, exactly as before there was
    // a split at all, and carry no arguments.
    if filtered.is_empty() && !args.is_empty() {
        filtered = state.filter_display_entries(&stack, &input);
        args.clear();
    }
    if let Some(palette) = &mut state.palette {
        palette.filtered = filtered;
        palette.selected = 0;
        palette.args = args;
    }
}

/// Handle key events when the dialog is focused.
///
/// Two modes, because the overlay has to be able to produce **every** answer
/// the server accepts, and free text is a legal answer to every question — a
/// menu never forbids it, which is why `ask_user` tells the model not to add an
/// "other" choice:
///
/// * **pick** — digits answer outright, arrows move the highlight, Enter
///   confirms it. Offered only when a single index *can* answer the question
///   ([`DialogState::has_quick_pick`]), the same predicate the server uses to
///   decide whether a channel gets an inline keyboard.
/// * **type** — printable keys build a free-text answer, Backspace deletes,
///   Enter sends it. It is the *only* mode a free-text or multi-select
///   question has, and the mode `Tab` reaches from a menu.
///
/// Before this, the overlay was menu-only: a question with no choices had no
/// answerable key at all, and `Esc` is deliberately swallowed for this overlay
/// (the run is parked on a oneshot), so the TUI was held by a modal nothing
/// could dismiss.
fn handle_dialog_key(state: &mut AppState, key: KeyEvent) -> Action {
    let Some(dialog) = &mut state.dialog else {
        return Action::None;
    };
    // A chord is a command, not text. Ctrl+C / Ctrl+D are already claimed by
    // the global handler; everything else with a modifier must not silently
    // land in the answer buffer.
    let plain = !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    match key.code {
        // Tab flips between the menu and a typed answer. Only meaningful when
        // there is a menu to flip away from.
        KeyCode::Tab if dialog.has_quick_pick() => {
            dialog.typing = !dialog.typing;
            Action::None
        }
        // Quick pick: 1–9 answers outright while the menu has focus. In text
        // mode a digit is just a character — that is what makes "3 days" a
        // sendable answer to a three-choice question.
        //
        // A digit PAST the end of the menu is not a pick, so it falls through
        // to the typing arm below rather than being eaten: on a two-choice
        // question, "5 minutes" has to be typeable, and swallowing the leading
        // "5" would silently send "minutes".
        KeyCode::Char(c)
            if plain
                && !dialog.typing
                && c.is_ascii_digit()
                && c != '0'
                && (c as usize) - ('1' as usize) < dialog.options.len() =>
        {
            let idx = (c as usize) - ('1' as usize);
            Action::RespondToDialog {
                session_key: dialog.session_key.clone(),
                // 1-based index, not the label — see `pending_reply`.
                reply: (idx + 1).to_string(),
            }
        }
        // Any other printable key is an answer being typed. On a menu question
        // the first one switches modes, so the character is never lost.
        KeyCode::Char(c) if plain => {
            dialog.typing = true;
            dialog.input.push(c);
            Action::None
        }
        KeyCode::Backspace if plain => {
            dialog.input.pop();
            Action::None
        }
        // Arrows drive the menu only. In text mode they do nothing rather than
        // moving a highlight the Enter key is not going to read.
        KeyCode::Up if !dialog.typing => {
            dialog.selected = dialog.selected.saturating_sub(1);
            Action::None
        }
        KeyCode::Down if !dialog.typing => {
            if dialog.selected + 1 < dialog.options.len() {
                dialog.selected += 1;
            }
            Action::None
        }
        KeyCode::Enter => {
            dialog
                .pending_reply()
                .map_or(Action::None, |reply| Action::RespondToDialog {
                    session_key: dialog.session_key.clone(),
                    reply,
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
                if approval.selected + 1 < approval.decisions.len() {
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

/// Handle key events when the provider/model picker is focused.
///
/// Same scheme as the session picker, with one addition the two levels need:
/// Backspace on an already-empty filter climbs back to the provider level
/// before it closes the overlay, so descending into the wrong provider costs
/// one keystroke rather than a reopen and a re-type.
fn handle_provider_picker_key(state: &mut AppState, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Up => Action::ProviderPickerUp,
        KeyCode::Down => Action::ProviderPickerDown,
        KeyCode::Enter | KeyCode::Tab => Action::ProviderPickerConfirm,
        // Ctrl+R, not a bare letter: every unmodified character is filter text
        // here, and stealing one would make a provider whose id contains it
        // unreachable by typing.
        KeyCode::Char('r' | 'R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::ProviderPickerRefresh
        }
        KeyCode::Backspace => {
            let is_empty = state
                .provider_picker
                .as_ref()
                .is_none_or(|p| p.input.is_empty());
            if !is_empty {
                if let Some(picker) = &mut state.provider_picker {
                    picker.input.pop();
                }
                state.recompute_provider_filter();
                Action::None
            } else if state.provider_picker_go_back() {
                Action::None
            } else {
                Action::CloseOverlay
            }
        }
        KeyCode::Char(c) => {
            if let Some(picker) = &mut state.provider_picker {
                picker.input.push(c);
            }
            state.recompute_provider_filter();
            Action::None
        }
        _ => Action::None,
    }
}

#[cfg(test)]
// Two complementary test modules live here:
//
// * `palette_key_tests` covers `handle_palette_key`: a regression where the
//   session knobs were readable from the TUI but settable from nowhere
//   because everything typed after the command name was being dropped.
// * `dialog_key_tests` covers `handle_dialog_key`: the modal overlay's
//   free-text / quick-pick / secret path. Before this module existed the
//   overlay was menu-only — a question with no choices had no answerable
//   key at all, and `Esc` is deliberately swallowed for it.
//
// * `provider_key_tests` covers `handle_provider_picker_key`: the one place
//   its scheme departs from the session picker's, which is Backspace climbing
//   a level before it closes.
//
// They are named (not all `mod tests`) so the file keeps several co-existing
// `#[cfg(test)]` modules in one file without Rust complaining.
mod palette_key_tests {
    use super::*;
    use crate::tui::app::PaletteState;
    use crossterm::event::KeyEventKind;

    fn key(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    /// Type into an open palette, one character at a time, the way a user does.
    fn type_into_palette(state: &mut AppState, text: &str) {
        for c in text.chars() {
            handle_palette_key(state, key(c));
        }
    }

    fn palette_with(text: &str) -> AppState {
        let mut state = AppState::new("s".into(), "m".into());
        state.open_command_palette();
        type_into_palette(&mut state, text);
        state
    }

    /// The regression this exists for: every session knob was readable from
    /// the TUI and settable from nowhere.
    ///
    /// `/` on an empty composer opens the palette instead of typing a slash,
    /// so the palette is the only route to a slash command — and it ran the
    /// selected entry's bare `full_command`, dropping anything typed after it.
    /// `/think high` therefore reached `parse_input` as `/think`, which prints
    /// the current level and a usage line. Same for `/tier`, `/mode`,
    /// `/memory-mode`, `/tools` and `/compact <instructions>`.
    #[test]
    fn a_value_typed_after_the_command_reaches_the_command() {
        let state = palette_with("think high");
        let palette = state.palette.as_ref().expect("palette open");
        assert_eq!(palette.args, "high");
        assert_eq!(
            palette.selected_command().as_deref(),
            Some("/think high"),
            "the value the user typed must ride along to the parser"
        );
    }

    /// The whole chain for `/providers <query>`, end to end.
    ///
    /// The palette is the only route to a slash command from an empty composer,
    /// so a picker that opens unfiltered no matter what was typed is the same
    /// defect the knobs had. Asserting the parsed command — not the string —
    /// is what makes this cover the split, the confirm path AND the parser
    /// together; any one of the three dropping the tail turns it red.
    #[test]
    fn providers_opens_pre_filtered_from_the_palette() {
        let state = palette_with("providers gpt-5.6");
        let command = state
            .palette
            .as_ref()
            .and_then(PaletteState::selected_command)
            .expect("the exact command name ranks first");
        assert_eq!(
            slash::parse_input(&command),
            ParsedInput::Local(LocalCommand::Providers {
                query: "gpt-5.6".to_string()
            })
        );
    }

    /// Free text after the command survives verbatim, spaces and all — this
    /// is what `/compress [instructions]` is.
    #[test]
    fn multi_word_arguments_survive_intact() {
        let state = palette_with("compress keep the api decisions");
        assert_eq!(
            state
                .palette
                .as_ref()
                .and_then(PaletteState::selected_command)
                .as_deref(),
            Some("/compress keep the api decisions")
        );
    }

    /// With no argument, nothing changes: the bare command runs, as it always
    /// did (and printing the current value is what a bare knob command is for).
    #[test]
    fn a_bare_command_is_unchanged() {
        let state = palette_with("think");
        let palette = state.palette.as_ref().expect("palette open");
        assert!(palette.args.is_empty());
        assert_eq!(palette.selected_command().as_deref(), Some("/think"));
    }

    /// The fallback: when the first word names no command, the whole string
    /// stays one search term and nothing is treated as an argument — so
    /// searching the entries' description text keeps working, and the split
    /// can never produce a worse candidate list than no split at all.
    #[test]
    fn a_first_word_that_names_nothing_is_still_just_a_search() {
        let state = palette_with("zzzznotacommand qqqq");
        let palette = state.palette.as_ref().expect("palette open");
        assert!(
            palette.args.is_empty(),
            "a failed head match must not leave an argument behind"
        );
    }
}

#[cfg(test)]
mod dialog_key_tests {
    use super::*;
    use crate::tui::app::AskDialogView;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn state_with(view: AskDialogView) -> AppState {
        let mut state = AppState::new("s".into(), "m".into());
        state.show_dialog("telegram:bot:1:u1".into(), view);
        state
    }

    fn menu() -> AskDialogView {
        AskDialogView {
            question: "Deploy where?".into(),
            options: vec!["staging — shared QA".into(), "prod — live".into()],
            multi_select: false,
            secret: false,
        }
    }

    fn free_text() -> AskDialogView {
        AskDialogView {
            question: "Which language?".into(),
            options: vec![],
            multi_select: false,
            secret: false,
        }
    }

    /// The bug this whole path exists to fix. Every key a user could plausibly
    /// press on a free-text question used to resolve to `Action::None`, and
    /// `Esc` is swallowed by the global handler for this overlay, so the only
    /// way out of the modal was killing the process.
    #[test]
    fn a_free_text_question_can_be_answered() {
        let mut state = state_with(free_text());
        for c in "Ελληνικά".chars() {
            assert!(matches!(
                handle_dialog_key(&mut state, key(KeyCode::Char(c))),
                Action::None
            ));
        }
        let action = handle_dialog_key(&mut state, key(KeyCode::Enter));
        match action {
            Action::RespondToDialog { session_key, reply } => {
                assert_eq!(session_key, "telegram:bot:1:u1");
                assert_eq!(reply, "Ελληνικά");
            }
            other => panic!("a typed answer must be sendable, got {other:?}"),
        }
    }

    /// Enter on an empty buffer must not post a blank answer — core would take
    /// it as the user's free-text reply and unpark the tool with nothing.
    #[test]
    fn an_empty_buffer_sends_nothing() {
        let mut state = state_with(free_text());
        assert!(matches!(
            handle_dialog_key(&mut state, key(KeyCode::Enter)),
            Action::None
        ));
        handle_dialog_key(&mut state, key(KeyCode::Char(' ')));
        assert!(matches!(
            handle_dialog_key(&mut state, key(KeyCode::Enter)),
            Action::None
        ));
    }

    /// One keypress still answers a menu question, and it answers with the
    /// INDEX — the label carries its `— description` suffix and core matches
    /// labels exactly, so a label reply lands as free text.
    #[test]
    fn a_digit_still_answers_a_menu_outright() {
        let mut state = state_with(menu());
        match handle_dialog_key(&mut state, key(KeyCode::Char('2'))) {
            Action::RespondToDialog { reply, .. } => assert_eq!(reply, "2"),
            other => panic!("expected an immediate answer, got {other:?}"),
        }
    }

    /// …and the same index arrives via the arrow + Enter route.
    #[test]
    fn arrows_move_the_highlight_and_enter_confirms_it() {
        let mut state = state_with(menu());
        handle_dialog_key(&mut state, key(KeyCode::Down));
        assert_eq!(state.dialog.as_ref().unwrap().selected, 1);
        // Clamped at both ends rather than wrapping.
        handle_dialog_key(&mut state, key(KeyCode::Down));
        assert_eq!(state.dialog.as_ref().unwrap().selected, 1);
        handle_dialog_key(&mut state, key(KeyCode::Up));
        handle_dialog_key(&mut state, key(KeyCode::Up));
        assert_eq!(state.dialog.as_ref().unwrap().selected, 0);
        match handle_dialog_key(&mut state, key(KeyCode::Enter)) {
            Action::RespondToDialog { reply, .. } => assert_eq!(reply, "1"),
            other => panic!("expected the highlighted choice, got {other:?}"),
        }
    }

    /// A menu never forbids free text — `ask_user` tells the model not to add
    /// an "other" choice precisely because this exists. Tab reaches it, and a
    /// digit typed there is a character, not a pick: "3 days" must be sendable
    /// as an answer to a three-choice question.
    #[test]
    fn tab_reaches_free_text_from_a_menu_and_digits_become_characters() {
        let mut state = state_with(menu());
        handle_dialog_key(&mut state, key(KeyCode::Tab));
        assert!(state.dialog.as_ref().unwrap().typing);
        for c in "3 days".chars() {
            handle_dialog_key(&mut state, key(KeyCode::Char(c)));
        }
        assert_eq!(state.dialog.as_ref().unwrap().input, "3 days");
        match handle_dialog_key(&mut state, key(KeyCode::Enter)) {
            Action::RespondToDialog { reply, .. } => assert_eq!(reply, "3 days"),
            other => panic!("expected the typed answer, got {other:?}"),
        }
    }

    /// Tab is a no-op where there is no menu to go back to, and must not eat
    /// the mode a free-text question opened in.
    #[test]
    fn tab_cannot_strand_a_free_text_question_in_menu_mode() {
        let mut state = state_with(free_text());
        handle_dialog_key(&mut state, key(KeyCode::Tab));
        assert!(
            state.dialog.as_ref().unwrap().typing,
            "a question with no menu has nowhere else to be"
        );
    }

    /// A digit PAST the end of the menu is not a pick. It must start a typed
    /// answer instead of being swallowed: on a two-choice question "5 minutes"
    /// has to be sendable, and eating the leading "5" would silently send
    /// "minutes" — an answer the user never wrote.
    #[test]
    fn a_digit_beyond_the_menu_starts_a_typed_answer() {
        let mut state = state_with(menu());
        for c in "5 minutes".chars() {
            handle_dialog_key(&mut state, key(KeyCode::Char(c)));
        }
        assert!(state.dialog.as_ref().unwrap().typing);
        assert_eq!(state.dialog.as_ref().unwrap().input, "5 minutes");
        match handle_dialog_key(&mut state, key(KeyCode::Enter)) {
            Action::RespondToDialog { reply, .. } => assert_eq!(reply, "5 minutes"),
            other => panic!("expected the typed answer, got {other:?}"),
        }
    }

    #[test]
    fn backspace_deletes_the_last_character() {
        let mut state = state_with(free_text());
        for c in "ab".chars() {
            handle_dialog_key(&mut state, key(KeyCode::Char(c)));
        }
        handle_dialog_key(&mut state, key(KeyCode::Backspace));
        assert_eq!(state.dialog.as_ref().unwrap().input, "a");
    }

    /// A chord is a command, not text. Without this guard every unclaimed
    /// Ctrl-key would silently deposit its letter into the answer.
    #[test]
    fn a_modified_key_never_lands_in_the_buffer() {
        let mut state = state_with(free_text());
        handle_dialog_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        );
        handle_dialog_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
        );
        assert!(state.dialog.as_ref().unwrap().input.is_empty());
    }
}

#[cfg(test)]
mod provider_key_tests {
    use super::*;
    use crate::tui::app::sample_catalog_entry;

    fn opened() -> AppState {
        let mut state = AppState::new("s".into(), "m".into());
        state.open_provider_picker(
            vec![sample_catalog_entry("openai", &["gpt-5.6", "gpt-5.6-luna"])],
            String::new(),
        );
        state
    }

    fn press(state: &mut AppState, code: KeyCode) -> Action {
        handle_provider_picker_key(state, KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Typing narrows, Backspace widens. The rows have to move with the filter
    /// on every keystroke, because the row a bare Enter selects is whatever the
    /// last keystroke left at the top.
    #[test]
    fn typing_and_deleting_both_recompute_the_rows() {
        let mut state = opened();
        for c in "luna".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        let picker = state.provider_picker.as_ref().unwrap();
        assert_eq!(picker.input, "luna");
        assert_eq!(picker.rows.len(), 1);

        press(&mut state, KeyCode::Backspace);
        assert_eq!(state.provider_picker.as_ref().unwrap().input, "lun");
    }

    /// Ctrl+R asks the vendor; a bare `r` is filter text.
    ///
    /// Every unmodified character in this overlay is search input, so stealing
    /// one would make a provider whose id contains it unreachable by typing —
    /// and `openrouter`, `openrouter`'s roster and `cerebras` all contain `r`.
    #[test]
    fn only_the_modified_r_refreshes() {
        let mut state = opened();

        assert!(matches!(
            handle_provider_picker_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
            ),
            Action::ProviderPickerRefresh
        ));
        assert!(
            state.provider_picker.as_ref().unwrap().input.is_empty(),
            "the refresh key must not also land in the filter"
        );

        assert!(matches!(
            press(&mut state, KeyCode::Char('r')),
            Action::None
        ));
        assert_eq!(state.provider_picker.as_ref().unwrap().input, "r");
    }

    /// The refresh target is the row under the cursor at the provider level,
    /// and the descended-into provider at the model level — never "whatever",
    /// because this key dials a vendor.
    #[test]
    fn the_refresh_target_follows_the_level() {
        let mut state = AppState::new("s".into(), "m".into());
        state.open_provider_picker(
            vec![
                sample_catalog_entry("openai", &["gpt-5.6"]),
                sample_catalog_entry("moonshot", &["kimi-k2.6"]),
            ],
            String::new(),
        );

        assert_eq!(
            state.provider_picker_refresh_target().as_deref(),
            Some("openai")
        );

        state.enter_provider(1);
        assert_eq!(
            state.provider_picker_refresh_target().as_deref(),
            Some("moonshot"),
            "inside a roster the target is the open provider, not a model row"
        );
    }

    /// A refetch re-resolves the open provider by id, not by position.
    ///
    /// The server sorts and filters the catalogue, so an index is not a stable
    /// handle across two calls — keeping it would silently move the open roster
    /// to a different vendor while the user was looking at it.
    #[test]
    fn a_refetch_keeps_the_open_provider_even_when_the_rows_move() {
        let mut state = opened();
        state.replace_provider_catalog(vec![
            sample_catalog_entry("openai", &["gpt-5.6"]),
            sample_catalog_entry("moonshot", &["kimi-k2.6"]),
        ]);
        state.enter_provider(1);
        assert_eq!(
            state.provider_picker_refresh_target().as_deref(),
            Some("moonshot")
        );

        // The same catalogue, in the other order.
        state.replace_provider_catalog(vec![
            sample_catalog_entry("moonshot", &["kimi-k2.6", "kimi-k2.6-turbo"]),
            sample_catalog_entry("openai", &["gpt-5.6"]),
        ]);
        assert_eq!(
            state.provider_picker_refresh_target().as_deref(),
            Some("moonshot"),
            "the open roster must follow the id, not the index"
        );
        assert_eq!(
            state.provider_picker.as_ref().unwrap().rows.len(),
            2,
            "and it must show the newly discovered ids"
        );
    }

    /// The cursor follows the row it was on, not the position it was at.
    ///
    /// `recompute_provider_filter` resets the cursor to the top, so a refetch
    /// that reorders the catalogue would leave the user looking at a different
    /// provider than the one they just refreshed.
    #[test]
    fn a_refetch_keeps_the_cursor_on_the_row_it_was_on() {
        let mut state = AppState::new("s".into(), "m".into());
        state.open_provider_picker(
            vec![
                sample_catalog_entry("openai", &["gpt-5.6"]),
                sample_catalog_entry("moonshot", &["kimi-k2.6"]),
            ],
            String::new(),
        );
        // `handle_provider_picker_key` only *returns* the action; the main loop
        // is what moves the cursor, so move it the way the loop does.
        state.provider_picker.as_mut().unwrap().selected = 1;
        assert_eq!(
            state.provider_picker_refresh_target().as_deref(),
            Some("moonshot")
        );

        // Same catalogue, other order.
        state.replace_provider_catalog(vec![
            sample_catalog_entry("moonshot", &["kimi-k2.6"]),
            sample_catalog_entry("openai", &["gpt-5.6"]),
        ]);
        assert_eq!(
            state.provider_picker_refresh_target().as_deref(),
            Some("moonshot"),
            "the cursor must follow the provider, not the index"
        );
    }

    /// A provider that disappears from the catalogue between two reads leaves
    /// the picker at the provider level rather than pointing at a stranger.
    #[test]
    fn a_refetch_that_drops_the_open_provider_climbs_out() {
        let mut state = opened();
        state.enter_provider(0);
        state.replace_provider_catalog(vec![sample_catalog_entry("moonshot", &["kimi-k2.6"])]);
        assert!(
            state.provider_picker.as_ref().unwrap().provider.is_none(),
            "an open provider the server no longer sends must not resolve to another row"
        );
    }

    /// Backspace on an already-empty filter climbs out of a provider before it
    /// closes the overlay — descending into the wrong one costs a keystroke,
    /// not a reopen. At the top level there is nothing above, so it closes.
    #[test]
    fn backspace_climbs_a_level_before_it_closes() {
        let mut state = opened();
        state.enter_provider(0);

        assert!(matches!(
            press(&mut state, KeyCode::Backspace),
            Action::None
        ));
        assert!(
            state.provider_picker.as_ref().unwrap().provider.is_none(),
            "the first empty Backspace climbs"
        );

        assert!(matches!(
            press(&mut state, KeyCode::Backspace),
            Action::CloseOverlay
        ));
    }

    /// Esc closes the picker outright. It is a purely local overlay — no server
    /// run is parked on it and nothing has been sent until Enter — so unlike
    /// the AskUser and approval overlays it must not swallow the key.
    #[test]
    fn esc_closes_the_picker() {
        let mut state = opened();
        let mut textarea = TextArea::default();
        let action = handle_global_key(
            &mut state,
            &mut textarea,
            &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(matches!(action, Some(Action::CloseOverlay)));
    }
}

#[cfg(test)]
mod btw_key_tests {
    use super::*;

    fn opened() -> AppState {
        let mut state = AppState::new("agent:main:main".into(), "m".into());
        state.open_btw("why?".into());
        state
    }

    fn press(state: &mut AppState, code: KeyCode) -> Action {
        handle_btw_key(state, KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Esc is one key for one intent — "I am done with this" — and the
    /// overlay, not the key table, decides which of abort/close applies. The
    /// key handler's whole job is to say so without deciding.
    #[test]
    fn esc_asks_for_abort_or_close_either_way() {
        let mut state = opened();
        assert!(matches!(
            press(&mut state, KeyCode::Esc),
            Action::BtwAbortOrClose
        ));
        state.btw.finish_active(Some("because"));
        assert!(matches!(
            press(&mut state, KeyCode::Esc),
            Action::BtwAbortOrClose
        ));
    }

    /// The overlay opens in browse mode, so the bare shortcuts work on the
    /// answer that just arrived — which is when a user reaches for them.
    #[test]
    fn browse_mode_binds_the_bare_shortcuts() {
        let mut state = opened();
        assert!(!state.btw.composing, "the overlay opens ready to read");
        assert!(matches!(
            press(&mut state, KeyCode::Char('c')),
            Action::BtwCopy
        ));
        match press(&mut state, KeyCode::Char('p')) {
            Action::GatewayCommand(text) => assert_eq!(text, "/btw promote"),
            other => panic!("p must send the promote verb, got: {other:?}"),
        }
    }

    /// Typing an ordinary letter in browse mode starts a follow-up and keeps
    /// the letter. Tab is the way to a follow-up that starts with `c` or `p`,
    /// and the only way back — the mode never flips to browse on its own.
    #[test]
    fn typing_starts_a_follow_up_and_only_tab_goes_back() {
        let mut state = opened();
        press(&mut state, KeyCode::Char('h'));
        press(&mut state, KeyCode::Char('i'));
        assert!(state.btw.composing);
        assert_eq!(state.btw.composer, "hi");

        // In compose mode the shortcuts are letters again.
        press(&mut state, KeyCode::Char('c'));
        press(&mut state, KeyCode::Char('p'));
        assert_eq!(state.btw.composer, "hicp");

        press(&mut state, KeyCode::Backspace);
        assert_eq!(state.btw.composer, "hic");

        // Emptying the buffer must NOT drop back to browse: the next letter
        // would silently mean something else.
        for _ in 0..3 {
            press(&mut state, KeyCode::Backspace);
        }
        assert!(state.btw.composer.is_empty());
        assert!(state.btw.composing, "the mode is structural, not derived");

        press(&mut state, KeyCode::Tab);
        assert!(!state.btw.composing);
    }

    /// Enter sends the composer as the side thread's next question. The
    /// `/btw` is CONSTRUCTED here, never tested for — asking whether the text
    /// "is already a btw" would be a second copy of a predicate this client
    /// answers in exactly one place.
    #[test]
    fn enter_sends_the_follow_up_as_a_side_question() {
        let mut state = opened();
        state.btw.finish_active(Some("because"));
        press(&mut state, KeyCode::Char('a'));
        press(&mut state, KeyCode::Char('n'));
        press(&mut state, KeyCode::Char('d'));

        match press(&mut state, KeyCode::Enter) {
            Action::GatewayCommand(text) => assert_eq!(text, "/btw and"),
            other => panic!("expected a side question, got: {other:?}"),
        }
        assert!(
            state.btw.composer.is_empty(),
            "the composer must clear on send, or Enter twice sends twice"
        );
    }

    /// Enter on an empty composer sends nothing and puts the user where
    /// typing works, rather than doing nothing at all.
    #[test]
    fn enter_on_an_empty_composer_is_not_a_silent_no_op() {
        let mut state = opened();
        assert!(matches!(press(&mut state, KeyCode::Enter), Action::None));
        assert!(state.btw.composing);
    }

    /// Paging stays live in compose mode: the composer is a single line that
    /// is only ever appended to, so ←→ has nothing else to do, and re-reading
    /// the answer you are replying to is exactly when you want it.
    #[test]
    fn paging_works_in_both_modes() {
        use crate::tui::btw_overlay::BtwExchange;
        let mut state = opened();
        state.btw.finish_active(Some("a1"));
        state.btw.finish_exchange(BtwExchange::answered("q2", "a2"));
        assert_eq!(state.btw.view_index, 1);

        press(&mut state, KeyCode::Left);
        assert_eq!(state.btw.view_index, 0);

        state.btw.composing = true;
        press(&mut state, KeyCode::Right);
        assert_eq!(state.btw.view_index, 1);
        assert!(
            state.btw.composer.is_empty(),
            "an arrow key must never land in the composer"
        );
    }
}
