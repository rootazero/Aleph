// TUI module: full-screen terminal UI for interactive chat.
//
// Provides the main event loop that integrates terminal events (keyboard, resize),
// gateway events (streaming responses, tool updates), and a 50ms tick for spinner
// animation. Terminal-event routing lives in `keys.rs`, local slash-command
// execution in `commands.rs`, and all rendering in `render.rs`, which splits the
// layout into chat area, input area, status bar, and overlays.

mod app;
mod approval;
mod btw_overlay;
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

use std::future::Future;
use std::io;
use std::pin::Pin;
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
    attach_session, btw_abort_or_close, confirm_provider_pick, confirm_session_switch,
    dispatch_gateway_text, execute_local_command, fetch_gateway_commands, fetch_my_user_id,
    reconcile_side_question, refresh_picker_provider, send_to_agent, shadowed_gateway_commands,
    AttachMode,
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
    config: &CliConfig,
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

    // 3b-ii. This client's own principal id. One call, at startup, because the
    // answer cannot change for the life of a connection — the role and the
    // principal are both fixed by the `connect` handshake. It exists to answer
    // one question: on a shared room session, did somebody ELSE type the
    // message the server just echoed. `None` (older gateway, transport error,
    // or a caller with no principal record) turns the echo off rather than
    // guessing, which is the behaviour this screen had before it existed.
    let my_user_id = fetch_my_user_id(&client).await;

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
    state.my_user_id = my_user_id;
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
        attach_session(&mut state, &client, key, AttachMode::Append).await;
    }
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("Type a message... (/ for commands)");

    // 4. Spawn event collector
    let mut term_events = event::spawn_event_collector();

    // 5. Main loop
    let result = main_loop(
        &mut terminal,
        &mut state,
        &mut textarea,
        &client,
        config,
        &mut gateway_events,
        &mut term_events,
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

/// A reconnect attempt in flight, borrowing the client it will rebuild.
type ReconnectAttempt<'a> = Pin<Box<dyn Future<Output = CliResult<()>> + 'a>>;

/// One reconnect attempt, `delay` from now.
///
/// The wait lives INSIDE the future rather than in a second timer beside it, so
/// one object answers both "is an attempt outstanding" and "when does it
/// happen". Two pieces of state would be two things to get out of step, and the
/// failure would be a client that either never retries or retries continuously.
fn reconnect_after<'a>(
    client: &'a AlephClient,
    config: &'a CliConfig,
    delay: Duration,
) -> ReconnectAttempt<'a> {
    Box::pin(async move {
        tokio::time::sleep(delay).await;
        client.reconnect(config).await
    })
}

/// Await the attempt in flight, or never.
///
/// `None` has to be a branch that NEVER fires. A branch that resolved
/// immediately would spin the select at full speed and burn a core for as long
/// as the client is merely connected, which is almost always.
///
/// `select!` drops this wrapper every time another branch wins, and that is
/// safe: the attempt itself lives in `pending` and is only borrowed here, so
/// cancelling loses no progress — including none of the sleep.
async fn awaiting_reconnect<'a>(pending: &mut Option<ReconnectAttempt<'a>>) -> CliResult<()> {
    match pending {
        Some(attempt) => attempt.await,
        None => std::future::pending().await,
    }
}

/// How long to wait before the attempt after a failed one.
///
/// Starts at zero: the first try after a drop goes out immediately, because the
/// common case is a gateway that restarted and is already listening again. The
/// ceiling keeps a long outage from becoming a busy loop while staying short
/// enough that a user who fixes the network does not sit and wait for it.
fn next_backoff(current: Duration) -> Duration {
    const CEILING: Duration = Duration::from_secs(15);
    if current.is_zero() {
        Duration::from_secs(1)
    } else {
        (current * 2).min(CEILING)
    }
}

/// Whether a pure `Action::Tick` (no gateway/terminal event) needs a redraw.
///
/// A tick that only bumped the spinner counter with nothing on screen
/// depending on it (`has_active_run == false`) changes nothing visible and
/// can skip the draw entirely — the per-message line cache (see
/// `widgets::chat_area::LineCache`) already makes an idle draw cheap, but
/// cheap is not free, and a genuinely idle terminal has no reason to redraw
/// 20 times a second. `connection_state_changed` covers the one other thing
/// a pure tick can affect: the status dot flips on the disconnect/reconnect
/// edge (see the tick handler's own comment on why the edge, not the level,
/// matters).
fn should_redraw_after_tick(has_active_run: bool, connection_state_changed: bool) -> bool {
    has_active_run || connection_state_changed
}

/// The main event loop. Separated from `run()` so terminal restoration
/// happens even if this function returns an error.
async fn main_loop<'c>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    textarea: &mut TextArea<'_>,
    client: &'c AlephClient,
    config: &'c CliConfig,
    gateway_events: &mut mpsc::Receiver<StreamEvent>,
    term_events: &mut mpsc::Receiver<event::TermEvent>,
) -> CliResult<()> {
    // Owned here rather than passed in: nothing outside this loop reads it, and
    // a parameter that only one caller can supply is a parameter that only
    // spends the argument budget.
    let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
    // At most one reconnect attempt is ever outstanding, and the loop owns the
    // policy: `shared/client` supplies only the mechanism, deliberately, so
    // that a socket cannot come back without this screen learning of it and
    // re-asking the server what is true (see `AlephClient::reconnect`).
    let mut reconnecting: Option<ReconnectAttempt<'c>> = None;
    let mut backoff = Duration::ZERO;
    let mut reported_reconnect_failure = false;
    // Starts `true` so the first frame always draws — never delayed behind a
    // wait for the first event. Cleared right after each draw; only actions
    // that changed something visible set it back to `true` (see
    // `should_redraw_after_tick` for the one arm, `Action::Tick`, that
    // decides for itself instead of redrawing unconditionally).
    let mut needs_redraw = true;
    loop {
        // Settled by the reconnect branch below; read after the select, where
        // `reconnecting` is no longer borrowed.
        let mut reconnect_outcome: Option<CliResult<()>> = None;
        // Draw only when something visible changed since the last frame.
        if needs_redraw {
            terminal.draw(|f| render::render(f, state, textarea))?;
            needs_redraw = false;
        }

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
            outcome = awaiting_reconnect(&mut reconnecting) => {
                reconnect_outcome = Some(outcome);
                Action::None
            }
        };

        // A reconnect attempt settled. Handled here rather than inside the
        // select branch because both outcomes need to touch `reconnecting`,
        // which that branch still has borrowed.
        if let Some(outcome) = reconnect_outcome {
            reconnecting = None;
            // A reconnect completing or failing always changes visible
            // state: the status dot flips and/or a system message is
            // appended below.
            needs_redraw = true;
            match outcome {
                Ok(()) => {
                    backoff = Duration::ZERO;
                    state.is_connected = true;
                    // Everything on this screen came from a connection that no
                    // longer exists, and it stopped moving at a point nobody
                    // recorded. Rebuild from the server rather than trusting a
                    // transcript with an unknown hole in it — turns can have
                    // completed, and a peer can have spoken, while this client
                    // was away.
                    state.begin_reattach();
                    let key = state.session_key.clone();
                    if key.is_empty() {
                        // Nothing to re-open: the gateway has not routed a
                        // conversation for this screen yet. `begin_reattach`
                        // still ran, which is what re-arms reconciliation.
                        state.add_system_message("Reconnected.".to_string());
                    } else {
                        attach_session(state, client, &key, AttachMode::Replace).await;
                        state.add_system_message(
                            "Reconnected; conversation reloaded from the server. \
                             Locally-generated notices from before the drop are not \
                             part of it and are gone."
                                .to_string(),
                        );
                        state.scroll_to_bottom();
                    }
                    // The side thread is NOT covered by the reattach above.
                    // `chat.history` answers for the conversation on screen; a
                    // `/btw` run executes on a derived session this client
                    // cannot name, so its terminal frame is the only thing that
                    // ever settles the overlay — and that frame may have been
                    // sent to the socket that just died. Outside the
                    // `key.is_empty()` branch on purpose: a side question
                    // implies a prior `agent.run`, but which conversation this
                    // screen is on has no bearing on whether one is in flight.
                    reconcile_side_question(state, client).await;
                }
                Err(e) => {
                    backoff = next_backoff(backoff);
                    // Said once. A long outage retries every 15 s, and a line
                    // per attempt would bury the transcript the user is still
                    // reading — the status dot already reports the level.
                    if !reported_reconnect_failure {
                        reported_reconnect_failure = true;
                        state.add_system_message(format!(
                            "Reconnect failed ({e}). Still trying; the status dot \
                             turns green when it succeeds."
                        ));
                    }
                    reconnecting = Some(reconnect_after(client, config, backoff));
                }
            }
        }

        // Every action other than a pure idle tick represents a real state
        // change worth showing; `Tick` decides for itself in its own arm
        // below (see `should_redraw_after_tick`), and `None`/`Quit` need no
        // redraw (nothing changed, or the loop is exiting). Computed before
        // the match below, which moves owned fields (e.g. `SendMessage`'s
        // `String`) out of `action` in several arms.
        let action_always_redraws = !matches!(action, Action::Tick | Action::None | Action::Quit);

        // Execute action
        match action {
            Action::None => {}
            Action::Quit => {
                break;
            }
            Action::Tick => {
                state.spinner_frame = state.spinner_frame.wrapping_add(1);
                let was_connected = state.is_connected;
                // Reflect the live connection state in the status dot even while
                // idle (no in-flight call). The gateway-event channel never
                // yields None on a WS drop — the client keeps an ownership anchor
                // on the receiver — so the connection's own atomic is the only
                // reliable disconnect signal.
                //
                // The EDGE is what matters, not the level. The moment the
                // socket goes from up to down is the moment everything this
                // screen reconciled against it stops being true, and it is the
                // only moment worth starting a reconnect from — reading the
                // level alone would restart the attempt on every 50 ms tick.
                let live = client.is_connected();
                if live {
                    state.is_connected = true;
                } else if state.is_connected {
                    state.on_disconnected();
                    state.add_system_message(
                        "Connection lost — reconnecting in the background.".to_string(),
                    );
                    backoff = Duration::ZERO;
                    reported_reconnect_failure = false;
                    reconnecting = Some(reconnect_after(client, config, backoff));
                }
                // Poll for pending tool approvals while a run is active. Ask
                // exec tier can park a run waiting on a decision the thin client
                // receives no event for; ~1s cadence (every 20th 50ms tick)
                // keeps the 120s approval window responsive without chatter.
                if state.current_run.is_some() && state.spinner_frame.is_multiple_of(20) {
                    approval::poll_approvals(state, client).await;
                }
                needs_redraw = should_redraw_after_tick(
                    state.current_run.is_some(),
                    state.is_connected != was_connected,
                );
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
                state.ctrl_c_count = 0;

                if !text.is_empty() {
                    state.send_history.push(text.clone());
                    state.history_index = None;
                }

                // Not `send_to_agent` directly: a `/btw` must reach the
                // overlay instead of the transcript, and that decision has to
                // be made BEFORE anything is echoed into the conversation the
                // side question is supposed to stay out of.
                dispatch_gateway_text(state, client, &text, "Command error").await;
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
                                // Same single dispatcher as the typed path —
                                // the palette is a second way to type a
                                // command, not a second set of rules for what
                                // one means.
                                dispatch_gateway_text(state, client, &text, "Command error").await;
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

            // -- Side question (`/btw`) --
            Action::BtwAbortOrClose => {
                btw_abort_or_close(state, client).await;
            }
            Action::BtwCopy => {
                // Fire-and-forget: OSC 52 has no reply, so the notice says
                // what was actually done rather than claiming success. Written
                // straight to the terminal rather than into ratatui's buffer —
                // it is a control sequence, not content, and the next frame
                // repaints over nothing.
                let notice = match state.btw.copyable() {
                    Some(answer) => {
                        let sequence = btw_overlay::osc52_clipboard_sequence(answer);
                        use std::io::Write as _;
                        let mut out = io::stdout();
                        if write!(out, "{sequence}").and_then(|()| out.flush()).is_ok() {
                            "Side answer sent to the terminal's clipboard (OSC 52)."
                        } else {
                            "Could not write to the terminal to copy the side answer."
                        }
                    }
                    None => "Nothing to copy yet.",
                };
                state.add_system_message(notice.to_string());
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
            Action::ProviderPickerRefresh => {
                refresh_picker_provider(state, client).await;
            }
        }

        if action_always_redraws {
            needs_redraw = true;
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
    use super::{model_caption, should_redraw_after_tick, ModelCaption};
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

    #[test]
    fn tick_with_no_active_run_and_no_connection_change_does_not_redraw() {
        assert!(!should_redraw_after_tick(false, false));
    }

    #[test]
    fn tick_with_an_active_run_redraws_to_animate_the_spinner() {
        assert!(should_redraw_after_tick(true, false));
    }

    #[test]
    fn tick_with_a_connection_state_change_redraws_even_when_idle() {
        assert!(should_redraw_after_tick(false, true));
    }
}

#[cfg(test)]
mod reconnect_tests {
    use super::next_backoff;
    use std::time::Duration;

    /// The first attempt after a drop goes out immediately. The common case is
    /// a gateway that restarted and is already listening again, and making that
    /// case wait is making every user wait for the rare one.
    #[test]
    fn the_first_attempt_is_not_delayed() {
        assert_eq!(next_backoff(Duration::ZERO), Duration::from_secs(1));
    }

    /// Doubling, so a long outage is not a busy loop.
    #[test]
    fn each_failure_waits_longer_than_the_last() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(next_backoff(Duration::from_secs(4)), Duration::from_secs(8));
    }

    /// …to a ceiling, so a user who fixes the network is not left waiting for
    /// an exponent. Unbounded doubling is how a client that "reconnects
    /// automatically" ends up taking ten minutes to notice a server that came
    /// back in ten seconds.
    #[test]
    fn the_wait_stops_growing_at_the_ceiling() {
        assert_eq!(
            next_backoff(Duration::from_secs(10)),
            Duration::from_secs(15)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(15)),
            Duration::from_secs(15)
        );
    }

    /// The side-question repair is actually reached when a reconnect succeeds.
    ///
    /// `main_loop` owns a terminal and two channels, so this arm has no
    /// in-process test — and the defect it closes is precisely a severed wire:
    /// `reconcile_side_question` is fully implemented and fully tested on its
    /// own, and an overlay that spins forever looks exactly the same whether
    /// the repair is wrong or simply never called.
    ///
    /// Source-level because a runtime check cannot tell "never called" from
    /// "called and found nothing to do". Comment lines are stripped first: a
    /// comment naming the function must not satisfy a guard about calling it.
    /// `\r` goes first because this repo is checked out CRLF on Windows, where
    /// a separator anchored to a bare `\n` matches nothing and the scan reads
    /// the whole file — its own test module included — as production code.
    #[test]
    fn a_successful_reconnect_reconciles_the_side_question() {
        let src = include_str!("mod.rs").replace('\r', "");
        let production = src.split("#[cfg(test)]").next().expect("split yields one");
        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // Self-protection: these two bound the reconnect's success arm, and
        // without them the search below would pass over an empty slice.
        let start = code
            .find("state.begin_reattach();")
            .expect("the reconnect success arm must still reset the run state");
        let end = code
            .find("backoff = next_backoff(backoff);")
            .expect("the reconnect failure arm must still back off");
        assert!(start < end, "the two arms are no longer in that order");

        assert!(
            code[start..end].contains("reconcile_side_question(state, client)"),
            "a reconnect must re-decide the side question's fate. `chat.history` \
             answers only for the conversation on screen; a `/btw` run executes \
             on a derived session this client cannot name, so without this call \
             its overlay spins forever on a terminal frame that went to the dead \
             socket."
        );
    }
}
