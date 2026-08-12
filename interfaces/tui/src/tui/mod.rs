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
mod gateway_error;
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

use aleph_protocol::providers::ProviderListResult;
use aleph_protocol::StreamEvent;

use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

use app::{Action, AppState, Focus};
use commands::{
    attach_session, confirm_provider_pick, confirm_session_switch, execute_local_command,
    fetch_gateway_commands, send_to_agent, shadowed_gateway_commands,
};
use slash::ParsedInput;

/// The status bar's launch model caption, plus what to say in the transcript
/// when that caption is not a model name.
struct ModelCaption {
    caption: String,
    note: Option<String>,
}

/// Resolve the launch caption from a `providers.list` reply.
///
/// Three outcomes, and they used to be one word. `Err(_) => "unknown"` folded
/// "the gateway refused to tell me" into "the gateway has nothing to tell" —
/// and `providers.*` is operator-only, so a member connection takes the refusal
/// branch every single launch and reads a claim about their install that the
/// server never made. Only the `Ok` arm may say anything about what is
/// configured; the other two say what happened to us instead.
fn model_caption(reply: Result<Value, CliError>) -> ModelCaption {
    match reply {
        Ok(result) => default_provider_model(&result).map_or_else(
            || ModelCaption {
                caption: "none".to_string(),
                note: Some(
                    "The gateway reports no default provider model; name one with /providers."
                        .to_string(),
                ),
            },
            |model| ModelCaption {
                caption: model,
                note: None,
            },
        ),
        Err(e) => ModelCaption {
            caption: gateway_error::classify(&e).caption().to_string(),
            note: Some(gateway_error::explain(&e, "the gateway's default model")),
        },
    }
}

/// The default provider's model, read through the shared [`ProviderInfo`].
///
/// The keys used to be poked out of the JSON by hand here (`is_default`,
/// `model`) — the same shape that shipped `aleph providers list` rendering two
/// columns the server has never emitted. Deserialising the contract type makes
/// a rename a compile error on both sides at once.
fn default_provider_model(result: &Value) -> Option<String> {
    let providers = serde_json::from_value::<ProviderListResult>(result.clone())
        .ok()?
        .providers;
    providers
        .iter()
        .find(|p| p.is_default)
        .or_else(|| providers.first())
        .map(|p| p.model.clone())
        .filter(|model| !model.is_empty())
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
    let ModelCaption {
        caption: model_name,
        note: model_caption_note,
    } = model_caption(client.call::<_, Value>("providers.list", None::<()>).await);

    // 3b. Fetch gateway commands for command palette
    let gateway_commands = fetch_gateway_commands(&client).await;

    // An empty key means "not routed yet": the first `agent.run` omits
    // `session_key` entirely and adopts whatever canonical key the gateway
    // reports back. Inventing a key here is what used to strand every keyed RPC
    // this client made.
    let mut state = AppState::new(session_key.clone().unwrap_or_default(), model_name);
    // Honor the CLI `--verbose` flag from launch, not only the /verbose command.
    state.verbose = verbose;
    // A one-word caption cannot explain itself. When it is not a model name,
    // say once why — the status bar goes on showing the short form.
    if let Some(note) = model_caption_note {
        state.add_system_message(note);
    }
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
            // `DialogSelect` is gone: its only producer was the out-of-range
            // branch of the digit key, and an out-of-range digit is now the
            // first character of a typed answer. Moving the highlight is what
            // the arrow keys do, and they mutate `DialogState` in place.
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

            // -- Provider picker --
            Action::ProviderPickerUp => {
                if let Some(picker) = &mut state.provider_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                    }
                }
            }
            Action::ProviderPickerDown => {
                if let Some(picker) = &mut state.provider_picker {
                    if picker.selected + 1 < picker.rows.len() {
                        picker.selected += 1;
                    }
                }
            }
            Action::ProviderPickerConfirm => {
                confirm_provider_pick(state, client).await;
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
    use super::{model_caption, ModelCaption};
    use aleph_client::CliError;
    use aleph_protocol::jsonrpc::{ADMIN_REQUIRED_MESSAGE, AUTH_REQUIRED};
    use aleph_protocol::providers::ProviderInfo;

    /// Build a `providers.list` reply the way the server does — from
    /// `ProviderInfo`, not from a JSON literal written here. A literal would
    /// only prove serde round-trips its own bytes; serialising the contract
    /// type is what makes a wire rename break this test.
    fn reply(rows: Vec<ProviderInfo>) -> Result<serde_json::Value, CliError> {
        Ok(serde_json::json!({ "providers": rows }))
    }

    fn provider(name: &str, models: &[&str], is_default: bool) -> ProviderInfo {
        let info: ProviderInfo = serde_json::from_value(serde_json::json!({
            "name": name,
            "is_default": is_default,
        }))
        .expect("ProviderInfo defaults cover every unset field");
        info.with_models(models.iter().map(|m| (*m).to_string()).collect())
    }

    #[test]
    fn the_caption_is_the_default_provider_s_first_rung() {
        let ModelCaption { caption, note } = model_caption(reply(vec![
            provider("relay", &["fallback-model"], false),
            provider("openai", &["default-model", "second-rung"], true),
        ]));
        assert_eq!(caption, "default-model");
        assert!(note.is_none(), "a plain answer needs no explanation");
    }

    /// "I was refused" and "there is nothing" must not share a caption. The
    /// `providers.*` family is operator-only, so a member takes this branch on
    /// every launch — captioning it like an empty install states something the
    /// server never said.
    #[test]
    fn a_refusal_is_not_an_empty_install() {
        let refused = model_caption(Err(CliError::Rpc {
            code: AUTH_REQUIRED,
            message: ADMIN_REQUIRED_MESSAGE.to_string(),
        }));
        let empty = model_caption(reply(Vec::new()));
        let offline = model_caption(Err(CliError::Timeout("no response in 30s".into())));

        assert_eq!(refused.caption, "restricted");
        assert_eq!(empty.caption, "none");
        assert_eq!(offline.caption, "unavailable");
        // Each of the three non-answers explains itself once in the transcript;
        // the caption alone cannot.
        for outcome in [&refused, &empty, &offline] {
            assert!(outcome.note.is_some(), "{} says nothing", outcome.caption);
        }
        assert!(refused.note.as_ref().unwrap().contains("not an operator"));
        assert!(offline
            .note
            .as_ref()
            .unwrap()
            .contains("no response in 30s"));
    }
}
