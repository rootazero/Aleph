// TUI module: full-screen terminal UI for interactive chat.
//
// Provides the main event loop that integrates terminal events (keyboard, resize),
// gateway events (streaming responses, tool updates), and a 50ms tick for spinner
// animation. Terminal-event routing lives in `keys.rs`, local slash-command
// execution in `commands.rs`, and all rendering in `render.rs`, which splits the
// layout into chat area, input area, status bar, and overlays.

mod app;
mod approval;
mod command_tree;
mod commands;
mod event;
mod keys;
mod markdown;
mod render;
mod slash;
mod theme;
mod widgets;

use std::io;
use std::time::Duration;

use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use aleph_protocol::StreamEvent;

use aleph_client::{AlephClient, CliConfig, CliResult};

use app::{Action, AppState, Focus};
use commands::{
    attach_session, confirm_session_switch, execute_local_command, fetch_gateway_commands,
    send_to_agent, shadowed_gateway_commands,
};
use slash::ParsedInput;

fn model_name_from_provider_list(result: &Value) -> String {
    result
        .get("providers")
        .and_then(Value::as_array)
        .and_then(|providers| {
            providers
                .iter()
                .find(|provider| provider.get("is_default").and_then(Value::as_bool) == Some(true))
                .or_else(|| providers.first())
        })
        .and_then(|provider| provider.get("model").and_then(Value::as_str))
        .filter(|model| !model.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

/// Entry point: run the TUI application.
///
/// Sets up the terminal, spawns the event collector, and runs the main loop
/// until the user quits. Terminal is always restored on exit (including panics).
///
/// # Errors
///
/// Returns an error if terminal setup, the gateway handshake, or the main loop fails.
pub async fn run(
    client: AlephClient,
    mut gateway_events: mpsc::Receiver<StreamEvent>,
    _config: &CliConfig,
    session_key: Option<String>,
    verbose: bool,
) -> CliResult<()> {
    // 1. Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Bracketed paste lets the terminal deliver a multi-line paste as one
    // `Event::Paste` instead of a stream of key events (each newline a bare
    // Enter), which would otherwise auto-send the first pasted line. Unix/macOS
    // only — crossterm's Windows console source does not emit paste events, so
    // this is inert (but harmless) there.
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 2. Set panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste);
        let _ = execute!(io::stdout(), crossterm::cursor::Show);
        original_hook(info);
    }));

    // 3. Fetch model name from gateway, then create AppState and TextArea
    let model_name = match client.call::<_, Value>("providers.list", None::<()>).await {
        Ok(result) => model_name_from_provider_list(&result),
        Err(_) => "unknown".to_string(),
    };

    // 3b. Fetch gateway commands for command palette
    let gateway_commands = fetch_gateway_commands(&client).await;

    // An empty key means "not routed yet": the first `agent.run` omits
    // `session_key` entirely and adopts whatever canonical key the gateway
    // reports back. Inventing a key here is what used to strand every keyed RPC
    // this client made.
    let mut state = AppState::new(session_key.clone().unwrap_or_default(), model_name);
    // Honor the CLI `--verbose` flag from launch, not only the /verbose command.
    state.verbose = verbose;
    state.gateway_commands = gateway_commands;
    // A local command that matches a gateway one makes the gateway one
    // unreachable — this client resolves local first and never falls through.
    // Say so once at startup rather than letting a whole namespace vanish
    // quietly (see `shadowed_gateway_commands`).
    let shadowed = shadowed_gateway_commands(&state.gateway_commands);
    if !shadowed.is_empty() {
        state.add_system_message(format!(
            "Note: local commands shadow these gateway commands, which are now unreachable \
             from this client: {}.",
            shadowed
                .iter()
                .map(|c| format!("/{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // 3c. Attach to the named conversation: transcript + the settings that
    // govern it (mode / tier / thinking depth / model / cumulative tokens /
    // memory mode). Reopening a terminal mid-task lands you back where you
    // were; before this, `--session <key>` opened a blank screen and the status
    // bar reported the install defaults over a conversation that had its own.
    if let Some(key) = session_key.as_deref().filter(|k| !k.is_empty()) {
        attach_session(&mut state, &client, key).await;
    }
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
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
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
                keys::handle_terminal_event(state, textarea, &te)
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
                // Reflect the live connection state in the status dot even while
                // idle (no in-flight call). The gateway-event channel never
                // yields None on a WS drop — the client keeps an ownership anchor
                // on the receiver — so the connection's own atomic is the only
                // reliable disconnect signal.
                state.is_connected = client.is_connected();
                // Poll for pending tool approvals while a run is active. Ask
                // exec tier can park a run waiting on a decision the thin client
                // receives no event for; ~1s cadence (every 20th 50ms tick)
                // keeps the 120s approval window responsive without chatter.
                if state.current_run.is_some() && state.spinner_frame.is_multiple_of(20) {
                    approval::poll_approvals(state, client).await;
                }
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

                send_to_agent(state, client, &msg, "Send error").await;
            }
            Action::LocalCommand(cmd) => {
                execute_local_command(state, textarea, client, cmd).await;
            }
            Action::GatewayCommand(text) => {
                // Send slash command to Gateway as a regular message
                state.add_user_message(text.clone());
                state.ctrl_c_count = 0;

                if !text.is_empty() {
                    state.send_history.push(text.clone());
                    state.history_index = None;
                }

                send_to_agent(state, client, &text, "Command error").await;
            }
            Action::CancelRun(run_id) => {
                let params = json!({ "run_id": run_id });
                match client.call::<_, Value>("agent.cancel", Some(params)).await {
                    Ok(_) => {
                        state.add_system_message("Run cancelled.".to_string());
                        state.current_run = None;
                        state.run_started_at = None;
                        // The cancelled run may have been parked on an approval;
                        // retract its overlay (the poll stops once current_run
                        // clears and can no longer do it).
                        state.dismiss_pending_approval();
                    }
                    Err(e) => {
                        state.add_system_message(format!("Cancel error: {e}"));
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
                    // The entry names the command; anything the user typed
                    // after it is its argument (`PaletteState::selected_command`).
                    // Without that, the palette could only ever run a command
                    // bare — and it is the only route to one, `/` on an empty
                    // composer opening the palette instead of typing a slash.
                    if let Some(cmd_str) = palette.selected_command() {
                        state.close_overlay();
                        // Parse through our unified parser
                        match slash::parse_input(&cmd_str) {
                            ParsedInput::Local(cmd) => {
                                execute_local_command(state, textarea, client, cmd).await;
                            }
                            ParsedInput::Gateway(text) => {
                                // Send gateway command as chat message
                                state.add_user_message(text.clone());
                                send_to_agent(state, client, &text, "Command error").await;
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
            Action::RespondToDialog { session_key, reply } => {
                // The AskUser answer resolves the clarification the server's
                // `ask_user` tool is parked on — keyed by session, not run
                // (`agent.respondToInput` never existed server-side; this was a
                // dead wire that timed every TUI answer out after 600s). Same
                // RPC the Panel and CLI use.
                let params = json!({
                    "session_key": session_key,
                    "reply": reply,
                });
                match client
                    .call::<_, Value>("clarification.resolve", Some(params))
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        state.add_system_message(format!("Dialog response error: {e}"));
                    }
                }
                state.close_overlay();
            }

            // -- Tool approval --
            Action::ResolveApproval { index } => {
                approval::resolve_approval(state, client, index).await;
            }

            // -- Session picker --
            Action::SessionPickerUp => {
                if let Some(picker) = &mut state.session_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                    }
                }
            }
            Action::SessionPickerDown => {
                if let Some(picker) = &mut state.session_picker {
                    if picker.selected + 1 < picker.filtered.len() {
                        picker.selected += 1;
                    }
                }
            }
            Action::SessionPickerConfirm => {
                confirm_session_switch(state, client).await;
            }
        }

        // Check quit flag
        if state.should_quit {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::model_name_from_provider_list;

    #[test]
    fn model_name_uses_default_provider_from_list() {
        let result = serde_json::json!({
            "providers": [
                {"model": "fallback-model", "is_default": false},
                {"model": "default-model", "is_default": true}
            ]
        });

        assert_eq!(model_name_from_provider_list(&result), "default-model");
    }
}
