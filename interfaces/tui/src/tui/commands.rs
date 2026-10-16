// Local slash-command execution: the TUI-side handlers for commands that resolve
// to Gateway RPCs (or pure client state), plus the session picker plumbing and
// result formatters.
//
// Extracted from `mod.rs` so the orchestrator file keeps only `run()` +
// `main_loop`. Every `agent.run` invocation funnels through `send_to_agent`, the
// single shared send site.

use serde_json::{json, Value};
use tui_textarea::TextArea;

use aleph_protocol::btw::BtwTurn;
use aleph_protocol::providers::{
    CatalogEntry, CatalogParams, CatalogResult, CatalogView, ModelsRefreshParams,
    ModelsRefreshResult, ModelsRefreshRow, RefreshOutcome,
};
use aleph_protocol::{
    AgentRunAccepted, AgentRunRequest, AgentRunStatusReport, AgentTraceListPage,
    LastRunDisposition, LastRunState, RunPhase, SessionListRow, SessionSnapshot,
};

use aleph_client::{AlephClient, CliError, CliResult};

use super::app::{self, AppState};
use super::command_tree;
use super::gateway_error;
use super::slash::{self, LocalCommand, SessionKnob as SlashKnob, ToolProgressMode};

/// Fire a message to the agent via `agent.run`.
///
/// This is the single `agent.run` call site shared by every sender
/// (SendMessage, GatewayCommand, the palette-confirm gateway path, and
/// `/retry`). Callers keep their own transcript/history bookkeeping and delegate
/// only the RPC + failure notice here.
///
/// The request is **built from [`AgentRunRequest`]**, not from a hand-written
/// `json!` literal. It used to be a literal sending `{"session_key", "message"}`
/// — and `agent.run` takes `input` (`message` is `chat.send`'s key), so every
/// send this TUI ever made came back `INVALID_PARAMS`. Nothing went red: this
/// crate cannot depend on `alephcore`, so the literal had nothing to disagree
/// with. Sharing the type makes a rename a compile error here and a red
/// reconciliation test in the crate that depends on both sides.
///
/// The gateway's reply carries the **canonical** session key it actually routed
/// to, which is not always the one we asked for (an unparseable key makes
/// `AgentRouter::route` mint a fresh epoch instead of failing). Adopting it
/// keeps every later `/usage`, `/tier`, `/compress` and `chat.history` call
/// addressing the conversation the user is actually in.
pub(super) async fn send_to_agent(
    state: &mut AppState,
    client: &AlephClient,
    message: &str,
    err_label: &str,
) {
    let request = AgentRunRequest {
        input: message.to_string(),
        // Empty = not routed yet: omit the key so the gateway routes one and
        // tells us its canonical spelling, rather than us inventing one it
        // cannot parse.
        session_key: (!state.session_key.is_empty()).then(|| state.session_key.clone()),
        ..AgentRunRequest::default()
    };
    match client
        .call::<_, AgentRunAccepted>("agent.run", Some(request))
        .await
    {
        Ok(accepted) => {
            // The key in this reply is the one the gateway ROUTED to, resolved
            // before the engine runs — so for a `/btw` it is the main
            // conversation's key, not the derived side key the run will
            // actually execute on (the redirect happens inside `execute()`,
            // long after the reply is built). Adopting it is therefore correct
            // on both paths: a side question cannot repoint this screen.
            state.adopt_canonical_session_key(&accepted.session_key);
            // A side question opened moments ago is waiting for the id of the
            // run that answers it, and this is the only place that id exists.
            // No frame can have arrived in between: gateway events are drained
            // only at the top of the main loop, never while an action's RPC is
            // awaited.
            state.btw.claim_pending_run(accepted.run_id);
        }
        Err(e) => state.add_system_message(format!("{err_label}: {e}")),
    }
}

/// Send `text` to the agent, routing a side question to the `/btw` overlay and
/// everything else to the transcript.
///
/// **The one place this client asks whether an input is a side question.**
/// The predicate is [`BtwTurn::resolve`], the resolver core and every channel
/// share (moved into `aleph-protocol` so this crate can reach it without
/// depending on `alephcore`, which its manifest forbids). A prefix test
/// written here would be a second answer to a question that already has one,
/// and would drift from the server's on the first spelling the server learns —
/// `/BTW`, `/btw@bot`, the empty body that is deliberately *not* a side
/// question.
///
/// The routing decision has to happen **before** the transcript is touched,
/// not after: a side question runs on a session the user is not looking at,
/// and echoing it into the conversation they ARE looking at is precisely the
/// leak this feature exists to prevent.
///
/// `/btw promote` is deliberately NOT routed here. It is a request to move a
/// side answer *into* the main conversation — a crossing of that boundary the
/// user asked for out loud — so it belongs in the transcript like any other
/// message, and its effect lands wherever the server decides to put it.
pub(super) async fn dispatch_gateway_text(
    state: &mut AppState,
    client: &AlephClient,
    text: &str,
    err_label: &str,
) {
    match BtwTurn::resolve(text) {
        Some(turn) if !turn.promote => {
            state.open_btw(turn.question);
            send_to_agent(state, client, text, "Side question error").await;
            if state.btw.active_run_id().is_none() {
                // The call failed; `send_to_agent` already said why. Settle the
                // question rather than leaving the overlay spinning on a run
                // that was never accepted.
                state
                    .btw
                    .fail_unclaimed("the side question was not accepted".to_string());
            }
        }
        _ => {
            state.add_user_message(text.to_string(), vec![]);
            send_to_agent(state, client, text, err_label).await;
        }
    }
}

/// What one `agent.status` answer means for a side question the overlay still
/// believes is being answered.
///
/// Three outcomes, not two, and the third is the load-bearing one: "the server
/// said it is over" and "I could not ask the server" are different facts, and
/// folding the second into the first settles a question over a run that may be
/// answering perfectly well on the other side of a socket that is still coming
/// up.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SideRunVerdict {
    /// The gateway says it is still in flight. The spinner is honest and the
    /// frames resume on the new socket — `EventVisibilityIndex` is
    /// process-shared, so the run→session seed this run was given on the dead
    /// connection is still there for the new one.
    StillAnswering,
    /// It ended in an error the gateway can name.
    Failed(String),
    /// It is no longer being answered here, for a reason that is not a failure.
    Disconnected(String),
    /// The question could not be put. Nothing is claimed and nothing settles;
    /// the next successful reconnect asks again.
    CouldNotAsk,
}

/// Read the server's answer about a side run.
///
/// Pure so the decision is reachable without a gateway — the same reason
/// `apply_history` is split out from `attach_session`.
///
/// # `Err` is two different things
///
/// [`CliError::Rpc`] means the gateway **answered**, and today its only answer
/// here is `Run not found`: this process has no record of the id. That covers a
/// core that restarted under the client and a run addressed by someone who may
/// not (deliberately the same response — see `caller_may_address_run`), and
/// neither is a verdict about the work. It is still a settlement, because
/// whatever became of that run, nothing is going to stream it to this client.
///
/// The note carries the server's own words rather than paraphrasing them. The
/// sentence around them has to stay true of *any* refusal this method might
/// grow, and "it did not report this run in flight" is the whole of what an
/// error response establishes — "it has no record of it" is one specific
/// reading, correct today and not this client's to assert.
///
/// Every other `CliError` is transport: the socket died again, the request timed
/// out, the reply would not parse. Those say nothing at all about the run, and
/// reading them as an ending is the "refusal read as absence" defect one layer
/// down.
fn side_run_verdict(answer: &CliResult<AgentRunStatusReport>) -> SideRunVerdict {
    let report = match answer {
        Ok(report) => report,
        Err(CliError::Rpc { message, .. }) => {
            return SideRunVerdict::Disconnected(format!(
                "Disconnected while this was being answered, and the gateway did not report it \
                 in flight (it said: {message}). Anything below arrived before the drop."
            ))
        }
        Err(_) => return SideRunVerdict::CouldNotAsk,
    };
    match report.phase() {
        RunPhase::Running => SideRunVerdict::StillAnswering,
        RunPhase::Failed => SideRunVerdict::Failed(
            report
                .error
                .clone()
                .filter(|reason| !reason.trim().is_empty())
                // A gateway that names the failure without saying why. Not
                // guessed at: "it failed" is the whole of what was said.
                .unwrap_or_else(|| "the gateway reported this run failed".to_string()),
        ),
        RunPhase::Completed => SideRunVerdict::Disconnected(
            "Disconnected while this was being answered. The gateway reports the run finished; \
             the text below is only what reached this client before the drop, and may not be \
             the whole answer."
                .to_string(),
        ),
        RunPhase::Cancelled => SideRunVerdict::Disconnected(
            "Disconnected while this was being answered. The gateway reports the run was \
             cancelled. Anything below arrived before the drop."
                .to_string(),
        ),
        // Not folded in with `Running`: an unknown word read as "still going"
        // is a spinner that never stops, and this client cannot recover from
        // that. See `RunPhase::Unrecognized`.
        RunPhase::Unrecognized => SideRunVerdict::Disconnected(
            "Disconnected while this was being answered, and the gateway reports a state this \
             client does not recognize. Anything below arrived before the drop."
                .to_string(),
        ),
    }
}

/// Ask the server what became of the side question this overlay is still
/// showing as unanswered, and stop the spinner unless it really is still going.
///
/// # Why the TUI needs its own path for this
///
/// A `/btw` run's terminal frame can be emitted while this client is offline,
/// and frames sent to a dead socket are simply gone — so the overlay waits for
/// a `RunComplete` that already happened, forever. The Panel repairs the
/// equivalent from `stream.running_set_changed` / `gateway.metrics.
/// run_concurrency`, but that set is keyed by **session** and is a Panel-only
/// surface (`frame_census::PANEL_ONLY_STREAM_METHODS`); it could not answer for
/// a side question anyway, because a side run executes on a derived session
/// whose key is hashed server-side and which this client therefore cannot name.
///
/// The run id is the one handle it does hold, and `agent.status` is the one
/// run-id-keyed read in the gateway. One round trip, and only when a side
/// question is actually in flight.
///
/// # The window this does not close
///
/// A run that finishes *during* this round trip settles as
/// [`BtwOutcome::Disconnected`], and the `RunComplete` that arrives a moment
/// later — carrying the full answer — finds no active question and does
/// nothing. Stated rather than hidden: the cost is the tail of one answer plus
/// a word that is still true, over a window of one RPC, on a run that had to be
/// alive at reconnect and dead a few milliseconds later. Closing it would mean
/// letting a late terminal frame overwrite a settled exchange, which is the
/// misattribution `for_active_run` exists to prevent.
pub(super) fn apply_side_question_status(
    state: &mut AppState,
    run_id: &str,
    answer: Result<AgentRunStatusReport, CliError>,
) {
    if state.btw.active_run_id() != Some(run_id) {
        return;
    }
    match side_run_verdict(&answer) {
        // Nothing is claimed on either: one is "it is still going", the other
        // is "I do not know". Both leave the overlay as it is.
        SideRunVerdict::StillAnswering | SideRunVerdict::CouldNotAsk => {}
        SideRunVerdict::Failed(reason) => state.btw.fail_active(run_id, reason),
        SideRunVerdict::Disconnected(note) => state.btw.settle_disconnected(run_id, note),
    }
}

/// The `chat.abort` params that stop the side question the overlay is
/// showing, or `None` when nothing is being answered.
///
/// Two things this gets right that the obvious version does not, which is why
/// it is a function with a test rather than two lines inline:
///
/// 1. **The run id is the overlay's, not the screen's.** `/stop`'s helper
///    aborts `state.current_run`, which during a side question is the *main*
///    run (or nothing at all). Reusing it would stop the wrong work, or
///    silently refuse — and this overlay is the only surface from which a
///    running side question can be aimed at.
/// 2. **`session_key` is deliberately omitted.** The server uses that field to
///    purge the addressed session's busy-queue backlog. The only key this
///    client holds is the main conversation's, so passing it would drop the
///    messages queued behind the main run — the opposite of "stop the side
///    question". The side session's key is not derivable here by design (see
///    `aleph_protocol::btw`), and the run id alone suffices: the server gates
///    it on its own (`caller_may_address_run`).
fn side_abort_params(state: &AppState) -> Option<Value> {
    let run_id = state.btw.active_run_id()?;
    Some(json!({ "run_id": run_id }))
}

/// Esc in the side-question overlay: stop the side run when one is answering,
/// close the overlay when none is.
///
/// One function, one decision. Splitting "is it answering?" from "abort it"
/// would let a caller reach one without the other, and the two answers have to
/// come from the same read of the same state.
pub(super) async fn btw_abort_or_close(state: &mut AppState, client: &AlephClient) {
    let Some(params) = side_abort_params(state) else {
        // Nothing on screen is waiting on a decision, so closing is safe for
        // the user. It is not the same as "nothing is running".
        //
        // A **superseded** question — one the user replaced by asking another
        // before the first finished — is filed with whatever text it had
        // while its run keeps going server-side. `side_abort_params` names
        // only `active_run_id()`, so from this client that run can no longer
        // be shown, aborted, or reached at all; it runs to completion and its
        // answer goes nowhere. That is a real cost, stated rather than
        // hidden. It is also strictly better than what it replaced, which
        // orphaned the run AND mixed its output into the next question's
        // answer.
        //
        // Not fixed here on purpose: aborting it would mean `begin` — a
        // synchronous method on the overlay, with no client and no async —
        // reaching the network, which is a different shape of change from a
        // key handler. A side question is read-only and short, so the leak is
        // bounded; the alternative worth considering is keeping the
        // superseded run addressable rather than firing an abort nobody asked
        // for.
        state.close_btw();
        return;
    };
    if let Err(e) = client.call::<_, Value>("chat.abort", Some(params)).await {
        state.add_system_message(format!("Side question abort error: {e}"));
    }
    // Settle either way: the abort was sent, and a run that refuses to die is
    // still not something this overlay can go on claiming to be answering.
    state.btw.abort_active();
}

/// Apply a local command; return read-only replay RPCs for the bounded UI queue.
pub(super) async fn execute_local_command(
    state: &mut AppState,
    _textarea: &TextArea<'_>,
    client: &AlephClient,
    cmd: LocalCommand,
) -> Option<(&'static str, Value)> {
    match cmd {
        LocalCommand::JumpUser => state.jump_to_last_user(),
        LocalCommand::Requests => {
            if let Some(owner) =
                state
                    .interaction
                    .hidden_owners()
                    .into_iter()
                    .find(|owner| match owner {
                        crate::tui::interaction::ModalOwner::Clarification {
                            session_key, ..
                        }
                        | crate::tui::interaction::ModalOwner::Approval { session_key, .. } => {
                            session_key == &state.session_key
                        }
                    })
            {
                state.interaction.reopen(&owner);
                state.sync_modal_focus();
            } else {
                state.add_system_message("No hidden requests for this conversation.".into());
            }
        }
        LocalCommand::ToolOutput { tool_call_id } => {
            if tool_call_id.is_empty() {
                state.add_system_message("Usage: /tool-output <tool_call_id>".into());
            } else {
                state.open_tool_output(tool_call_id);
            }
        }
        LocalCommand::Clear => {
            state.clear_screen();
        }
        LocalCommand::Detail { name } => {
            use shared_ui_logic::transcript::DetailLevel;
            match name.as_deref().and_then(DetailLevel::parse) {
                Some(level) => {
                    state.detail = level;
                    state.step_overrides.clear();
                    state.chat_line_cache = crate::tui::widgets::chat_area::LineCache::default();
                    match app::detail_setting::remember(level) {
                        Ok(_) => state.add_system_message(format!("Detail: {}", level.as_str())),
                        Err(e) => state.add_system_message(format!(
                            "Detail: {} (not saved: {e})",
                            level.as_str()
                        )),
                    }
                }
                None => state.add_system_message(format!(
                    "Detail: {} (usage: /detail brief|full)",
                    state.detail.as_str()
                )),
            }
        }
        LocalCommand::Verbose => {
            state.toggle_verbose();
            let mode = if state.verbose { "on" } else { "off" };
            state.add_system_message(format!("Verbose mode: {mode}"));
        }
        LocalCommand::Quit => {
            state.request_quit();
        }
        LocalCommand::Help => {
            let help_text = build_help_text(state);
            state.add_system_message(help_text);
        }
        LocalCommand::ReplayList => return Some(("trace.list", json!({"limit": 10}))),
        LocalCommand::ReplayShow { task_id } => {
            if state.current_run.is_some() {
                state.add_system_message(
                    "Wait for this run to settle before opening a replay.".into(),
                );
            } else {
                return Some(("trace.get", json!({"task_id": task_id})));
            }
        }
        LocalCommand::Usage => execute_usage(state, client).await,
        LocalCommand::Compress { instructions } => {
            execute_compress(state, client, &instructions).await;
        }
        LocalCommand::Stop => execute_stop(state, client).await,
        LocalCommand::Undo => {
            execute_undo(state, client).await;
        }
        LocalCommand::Retry => execute_retry(state, client).await,
        LocalCommand::Tools { mode } => execute_tools(state, mode),
        LocalCommand::Knob { knob, value } => execute_knob(state, client, knob, value).await,
        LocalCommand::Sessions => execute_sessions(state, client).await,
        LocalCommand::Providers { query } => execute_providers(state, client, query).await,
        LocalCommand::AgentPanel => {
            state.toggle_agent_panel();
            let mode = if state.agent_panel_visible {
                "shown"
            } else {
                "hidden"
            };
            state.add_system_message(format!("Agent panel: {mode}"));
        }
        LocalCommand::Agents => execute_agents(state, client).await,
        LocalCommand::Context => execute_context(state, client).await,
        LocalCommand::Todo => {
            state.tasks_panel_visible = !state.tasks_panel_visible;
            let notice = match (state.tasks_panel_visible, &state.plan) {
                (true, Some(plan)) if plan.has_content() => "Tasks panel shown.",
                (true, _) => "Tasks panel shown (it appears once this conversation has a plan).",
                (false, _) => "Tasks panel hidden. /todo to bring it back.",
            };
            state.add_system_message(notice.to_string());
        }
        LocalCommand::Theme { name } => execute_theme(state, name.as_deref()),
        LocalCommand::Browser { resource_id } => {
            execute_browser(state, client, &resource_id).await;
        }
    }
    None
}

/// Apply local display preferences without contacting the gateway.
fn execute_theme(state: &mut AppState, requested: Option<&str>) {
    use crate::tui::theme::{
        available_themes, current_theme_name, palette, remember_theme, select_theme, theme_file,
    };
    let requested = requested.map(str::trim);
    if requested.is_none() || requested == Some("list") {
        let (names, diagnostics) = available_themes();
        let depth = if palette().is_rgb() { "24-bit" } else { "ANSI" };
        let location = theme_file()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unavailable (no Aleph home)".to_string());
        let mut notice = format!(
            "Theme: {} · {depth}\nThemes: {}\n/theme <name> switches and remembers; /theme reload reloads the current file.\nPreference: {location}\nALEPH_TUI_THEME overrides the remembered choice at startup.",
            current_theme_name(), names.join(", ")
        );
        for diagnostic in diagnostics {
            notice.push_str(&format!("\nTheme skipped: {diagnostic}"));
        }
        state.add_system_message(notice);
        return;
    }
    let Some(requested) = requested else { return };
    let reload = requested == "reload";
    let name = if reload {
        current_theme_name()
    } else {
        requested.to_string()
    };
    match select_theme(&name) {
        Ok(()) => {
            // Styled markdown is cached; keep text and viewport identity while repainting.
            state.invalidate_rendered_lines();
            let current = current_theme_name();
            let notice = if reload {
                format!("Theme: {current} · reloaded")
            } else {
                match remember_theme(&current) {
                    Ok(path) => format!("Theme: {current} · remembered in {}", path.display()),
                    Err(e) => format!("Theme: {current} for this run · could not remember it: {e}"),
                }
            };
            state.add_system_message(notice);
        }
        Err(error) => state.add_system_message(format!(
            "Could not load theme {name:?}: {error}. Theme remains {}.",
            current_theme_name()
        )),
    }
}

/// Usage text for a blank `/browser` argument.
const BROWSER_USAGE: &str = "Usage: /browser <resource_id> - open a Gateway-published browser resource in your configured viewer.";

/// Shown when the viewer launcher is not enabled. Names the two environment
/// variables to set, never their values.
const BROWSER_DISABLED_NOTICE: &str = "Browser viewer is disabled by default. Set ALEPH_VIEWER_ENABLED=1 and ALEPH_VIEWER_TRUSTED_ORIGINS (comma-separated origins) and restart to open browser resources.";

/// One fixed text for every failed grant request. The RPC error is never
/// echoed: it can carry server-supplied text, including secrets.
const BROWSER_REQUEST_FAILED: &str = "Browser viewer request failed or was denied.";

/// Longest resource id the list will print.
const BROWSER_LIST_ID_MAX_LEN: usize = 128;

/// Why a `/browser <id>` request may or may not go to the Gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserResourceStatus {
    /// Blank argument: print usage.
    Empty,
    /// Not in the live catalog (never published, closed, or cleared on reconnect).
    Missing,
    /// In the catalog but past `expires_at_ms`.
    Expired,
    Available,
}

/// Classify `resource_id` against the live catalog. Exact-id lookup only: the
/// id is never derived from a URL, tab or profile, and the Gateway is not
/// consulted for anything the catalog does not already hold.
fn browser_resource_status(
    catalog: &app::BrowserResourceCatalog,
    resource_id: &str,
    now_ms: i64,
) -> BrowserResourceStatus {
    if resource_id.trim().is_empty() {
        BrowserResourceStatus::Empty
    } else if catalog.get_unexpired(resource_id, now_ms).is_some() {
        BrowserResourceStatus::Available
    } else if catalog.get(resource_id).is_some() {
        BrowserResourceStatus::Expired
    } else {
        BrowserResourceStatus::Missing
    }
}

/// Generic refusal text for a non-available status; never echoes the id.
fn browser_status_notice(status: BrowserResourceStatus) -> Option<&'static str> {
    match status {
        BrowserResourceStatus::Empty => Some(BROWSER_USAGE),
        BrowserResourceStatus::Missing => {
            Some("Browser resource not found. It may have closed or not been published yet.")
        }
        BrowserResourceStatus::Expired => Some("Browser resource has expired."),
        BrowserResourceStatus::Available => None,
    }
}

/// An id is printed only if every char is ASCII alphanumeric, `_` or `-`
/// (real ids are `res_...`). Ids come from the Gateway and are untrusted: one
/// with control characters could inject terminal escape sequences, and an
/// unsafe id cannot be offered as a copyable argument anyway.
fn is_displayable_browser_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= BROWSER_LIST_ID_MAX_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Usage plus the exact ids of the catalog's unexpired resources. Only ids are
/// shown (no title, URL or secret). Unexpired resources whose id is not safe to
/// print are counted, not shown and not rewritten.
fn format_browser_listing(catalog: &app::BrowserResourceCatalog, now_ms: i64) -> String {
    let mut ids = Vec::new();
    let mut omitted = 0usize;
    for resource in catalog.resources() {
        if now_ms >= resource.expires_at_ms {
            continue;
        }
        if is_displayable_browser_id(&resource.resource_id) {
            ids.push(resource.resource_id.as_str());
        } else {
            omitted += 1;
        }
    }
    let mut out = String::from(BROWSER_USAGE);
    if ids.is_empty() {
        out.push_str("\nNo browser resources are currently available.");
    } else {
        out.push_str("\nAvailable browser resources:");
        for id in ids {
            out.push_str("\n  ");
            out.push_str(id);
        }
    }
    if omitted > 0 {
        out.push_str(&format!(
            "\n({omitted} resource(s) with a non-displayable id omitted.)"
        ));
    }
    out
}

/// What `/browser <arg>` does before any Gateway call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BrowserPreflight {
    /// Show this text; do not contact the Gateway (no grant is minted).
    Show(String),
    /// The id is live and the viewer is enabled: go on to request a grant.
    Proceed,
}

/// Pure pre-issue decision. Order: blank argument lists resources; then the
/// exact catalog status/expiry guard; then the viewer-enabled gate, so a
/// disabled viewer never mints a grant nobody can use.
fn browser_preflight(
    catalog: &app::BrowserResourceCatalog,
    viewer_enabled: bool,
    resource_id: &str,
    now_ms: i64,
) -> BrowserPreflight {
    match browser_resource_status(catalog, resource_id, now_ms) {
        BrowserResourceStatus::Empty => {
            BrowserPreflight::Show(format_browser_listing(catalog, now_ms))
        }
        BrowserResourceStatus::Available if !viewer_enabled => {
            BrowserPreflight::Show(BROWSER_DISABLED_NOTICE.to_string())
        }
        BrowserResourceStatus::Available => BrowserPreflight::Proceed,
        refused => BrowserPreflight::Show(
            browser_status_notice(refused)
                .unwrap_or(BROWSER_USAGE)
                .to_string(),
        ),
    }
}

/// Text for a failed viewer launch. `LaunchError`'s Display never carries the
/// URL or nonce; `Disabled` additionally says how to opt in.
fn format_browser_launch_error(err: &crate::viewer_launcher::LaunchError) -> String {
    match err {
        crate::viewer_launcher::LaunchError::Disabled => {
            "Browser viewer launch is disabled by default. Enable and configure the viewer launcher to open browser resources."
                .to_string()
        }
        other => format!("Browser viewer launch failed: {other}"),
    }
}

/// `/browser`: with no argument, list the live resource ids. With an id:
/// request a read-only frames grant for that exact catalog resource (only
/// when the viewer launcher is enabled), then hand the one-time launch request to the (default
/// disabled) system launcher off the UI thread. Nothing secret is displayed.
async fn execute_browser(state: &mut AppState, client: &AlephClient, resource_id: &str) {
    execute_browser_with_launcher(state, client, resource_id, |config, request| {
        crate::viewer_launcher::ViewerLauncher::system(config).launch(&request)
    })
    .await;
}

/// [`execute_browser`] with the launch step injected, so tests can observe the
/// issue -> launch -> cleanup sequence without starting a real browser. The
/// only production caller passes the real system launcher; `launch` runs on
/// the blocking pool exactly like the closure it replaced.
async fn execute_browser_with_launcher<L>(
    state: &mut AppState,
    client: &AlephClient,
    resource_id: &str,
    launch: L,
) where
    L: FnOnce(
            crate::viewer_launcher::ViewerLauncherConfig,
            crate::viewer_launcher::ViewerLaunchRequest,
        ) -> Result<(), crate::viewer_launcher::LaunchError>
        + Send
        + 'static,
{
    let now_ms = i64::try_from(state.now_ms()).unwrap_or(i64::MAX);
    let preflight = browser_preflight(
        &state.browser_resources,
        state.viewer_launcher_config.enabled,
        resource_id,
        now_ms,
    );
    if let BrowserPreflight::Show(text) = preflight {
        state.add_system_message(text);
        return;
    }
    let issued = match crate::issue_browser_viewer_grant(client, resource_id).await {
        Ok(issued) => issued,
        Err(_) => {
            state.add_system_message(BROWSER_REQUEST_FAILED.to_string());
            return;
        }
    };
    let crate::IssuedBrowserViewerLaunch { request, grant_id } = issued;
    let config = state.viewer_launcher_config.clone();
    let outcome =
        browser_launch_outcome(tokio::task::spawn_blocking(move || launch(config, request)).await);
    let message = finish_browser_launch(&grant_id, outcome, |id| async move {
        crate::revoke_browser_viewer_grant(client, &id)
            .await
            .is_ok()
    })
    .await;
    state.add_system_message(message);
}

/// Result of handing an issued launch to the launcher thread.
#[derive(Debug)]
enum BrowserLaunchOutcome {
    Launched,
    Failed(crate::viewer_launcher::LaunchError),
    /// The blocking task itself panicked or was cancelled.
    TaskFailed,
}

fn browser_launch_outcome(
    joined: Result<Result<(), crate::viewer_launcher::LaunchError>, tokio::task::JoinError>,
) -> BrowserLaunchOutcome {
    match joined {
        Ok(Ok(())) => BrowserLaunchOutcome::Launched,
        Ok(Err(e)) => BrowserLaunchOutcome::Failed(e),
        Err(_) => BrowserLaunchOutcome::TaskFailed,
    }
}

/// Appended when a failed launch could not be cleaned up. Fixed text: the
/// revoke RPC error is never echoed (it may carry server-supplied text).
const BROWSER_GRANT_CLEANUP_FAILED: &str =
    " The unused viewer grant could not be revoked; it will expire on its own (TTL).";

/// Turn a launch outcome into the user-facing message. On any failure, calls
/// `revoke` once with the exact issued `grant_id`; on success never. The
/// original launch-failure text is always kept; a failed cleanup only appends
/// [`BROWSER_GRANT_CLEANUP_FAILED`].
async fn finish_browser_launch<R, Fut>(
    grant_id: &str,
    outcome: BrowserLaunchOutcome,
    revoke: R,
) -> String
where
    R: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let base = match &outcome {
        BrowserLaunchOutcome::Launched => return "Browser viewer launched.".to_string(),
        BrowserLaunchOutcome::Failed(e) => format_browser_launch_error(e),
        BrowserLaunchOutcome::TaskFailed => "Browser viewer launch task failed.".to_string(),
    };
    if revoke(grant_id.to_string()).await {
        base
    } else {
        format!("{base}{BROWSER_GRANT_CLEANUP_FAILED}")
    }
}

// ---------------------------------------------------------------------------
// Session picker (browse + switch), both RPCs already exist server-side.
// ---------------------------------------------------------------------------

/// Fetch the session list and open the picker overlay.
async fn execute_sessions(state: &mut AppState, client: &AlephClient) {
    match client.call::<_, Value>("sessions.list", None::<()>).await {
        Ok(result) => {
            // Accept both {"sessions": [...]} and a bare [...] top level.
            let rows = result
                .get("sessions")
                .and_then(Value::as_array)
                .or_else(|| result.as_array())
                .cloned()
                .unwrap_or_default();

            let entries: Vec<app::SessionEntry> =
                rows.iter().filter_map(session_entry_from_json).collect();

            if entries.is_empty() {
                state.add_system_message("No other sessions to switch to.".to_string());
            } else {
                state.open_session_picker(entries);
            }
        }
        Err(e) => state.add_system_message(format!("Sessions error: {e}")),
    }
}

/// Map one `sessions.list` row into a `SessionEntry`, or `None` if it has no key.
///
/// Parsed through [`SessionListRow`], the type the server constructs the row
/// from, rather than key by key. The hand-read version asked for `name` — a key
/// this row has never carried — so `topic` and `label` rode the wire unread and
/// every conversation in the picker was titled by its session key. A subset
/// reader can only ever prove it is a superset of whatever happens to arrive
/// (criterion #10); a shared type makes the same rename a compile error here.
fn session_entry_from_json(v: &Value) -> Option<app::SessionEntry> {
    let row: SessionListRow = serde_json::from_value(v.clone()).ok()?;
    if row.key.is_empty() {
        return None;
    }
    let title = row
        .topic
        .as_deref()
        .or(row.label.as_deref())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(row.key.as_str())
        .to_string();
    let mut label = format!("{title}  ({} msgs)", row.message_count);
    // The list face answers with a word and nothing else, so the mark is the
    // word — the counts belong to the attach face (`chat.history`), which is
    // where `apply_history` renders them.
    if let Some(mark) = row.last_run.as_ref().and_then(last_run_mark) {
        label.push_str(mark);
    }
    Some(app::SessionEntry {
        key: row.key,
        label,
    })
}

/// The picker's suffix for a conversation whose newest run needs attention, or
/// `None` when it does not.
///
/// One rule, shared with [`last_run_notice`]: "interrupted" is not the only
/// state worth marking. A log that holds dangling tool calls but no run marker
/// at all reduces to `never_ran`, and a log the reducer refused reduces to
/// `log_inconsistent` — both are runs whose outcome nobody can vouch for, and a
/// mark keyed on the word `interrupted` alone would leave them looking
/// finished.
fn last_run_mark(last_run: &LastRunState) -> Option<&'static str> {
    let dangling = last_run.dangling().is_some_and(|d| !d.is_empty());
    match last_run.disposition() {
        LastRunDisposition::Interrupted => Some("  [interrupted]"),
        LastRunDisposition::LogInconsistent => Some("  [log inconsistent]"),
        LastRunDisposition::Unrecognized => Some("  [unknown]"),
        LastRunDisposition::Unanswered => Some("  [unanswered]"),
        LastRunDisposition::Clean | LastRunDisposition::NeverRan if dangling => {
            Some("  [interrupted]")
        }
        LastRunDisposition::Clean | LastRunDisposition::NeverRan => None,
    }
}

// ---------------------------------------------------------------------------
// Agents overlay (`/agents`).
// ---------------------------------------------------------------------------

/// Refresh this session's sub-agent snapshot from `subagent.tree`, then open
/// the overlay. The snapshot is merged through the shared protocol
/// `apply_event` (a `Spawned` upsert per node — the Panel's cold-start rule),
/// so a spawn that raced ahead on the live topic survives.
async fn execute_agents(state: &mut AppState, client: &AlephClient) {
    refresh_agents(state, client).await;
    if state.agents.is_empty() {
        state.add_system_message(
            "No background sub-agents in this session yet. They appear here (and in the \
             docked panel) when the agent delegates work with the `subagent` tool."
                .to_string(),
        );
        return;
    }
    state.open_agents_overlay();
}

/// `/context`: fetch the measured layout of the last prompt and open the
/// overlay on it.
///
/// # Every failure here is "I don't know", never a breakdown of zeros
///
/// The server answers `RESOURCE_NOT_FOUND` when no turn of this session has
/// been measured — which includes a session that has never run AND one whose
/// measurements died with a daemon restart, because the registry is in
/// process memory. Both are unknowns, and the message says so rather than
/// opening an empty overlay that reads as "nothing is using your window"
/// (判据 §8).
async fn execute_context(state: &mut AppState, client: &AlephClient) {
    if state.session_key.is_empty() {
        state.add_system_message(
            "No session attached yet, so there is no prompt to break down.".to_string(),
        );
        return;
    }
    let params = json!({ "session_key": state.session_key });
    match client
        .call::<_, aleph_protocol::ContextBreakdown>("context.breakdown", Some(params))
        .await
    {
        Ok(breakdown) => state.open_context_overlay(&breakdown),
        // The error text is the server's own; it already distinguishes
        // "not measured yet" from a malformed key, and rewording it here
        // would be a second, weaker copy of that distinction.
        Err(e) => state.add_system_message(format!(
            "No measured prompt for this conversation yet: {e}\nIt appears once this session \
             takes a turn — measurements do not survive a gateway restart."
        )),
    }
}

/// One `subagent.tree` fetch, merged into `AppState.agents`.
pub(super) async fn refresh_agents(state: &mut AppState, client: &AlephClient) {
    if state.session_key.is_empty() {
        return;
    }
    let params = json!({ "root_session": state.session_key });
    match client.call::<_, Value>("subagent.tree", Some(params)).await {
        Ok(result) => {
            let nodes: Vec<aleph_protocol::subagent_tree::SubagentNode> = result
                .get("nodes")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            for node in nodes {
                aleph_protocol::subagent_tree::apply_event(
                    &mut state.agents,
                    aleph_protocol::subagent_tree::SubagentTreeEvent::Spawned { node },
                );
            }
        }
        Err(e) => state.add_system_message(format!("Sub-agent tree error: {e}")),
    }
}

/// Enter on an overlay row: open that agent's run view. The transcript is the
/// child's own persisted session (`SubagentNode.child_session`), served by the
/// same `chat.history` RPC as any conversation — no dedicated endpoint.
pub(super) async fn open_agent_detail(state: &mut AppState, client: &AlephClient) {
    let Some(node) = state.selected_agent().cloned() else {
        return;
    };
    let mut lines = agent_meta_lines(&node);
    match node.child_session.as_deref() {
        Some(child_key) => {
            let params = json!({ "session_key": child_key });
            match client.call::<_, Value>("chat.history", Some(params)).await {
                Ok(result) => {
                    lines.push(String::new());
                    lines.extend(transcript_lines(&result));
                }
                Err(e) => {
                    lines.push(String::new());
                    lines.push(format!("Transcript unavailable: {e}"));
                }
            }
        }
        None => {
            lines.push(String::new());
            lines.push(
                "No child transcript address for this agent (spawned before this build, \
                 or a sync fan-out child whose result was returned inline)."
                    .to_string(),
            );
        }
    }
    if let Some(overlay) = &mut state.agents_overlay {
        overlay.detail = Some(app::AgentDetailState {
            title: node.task.clone(),
            lines,
            scroll: 0,
        });
    }
}

/// Metadata header for the agent run view — everything the node itself knows.
fn agent_meta_lines(node: &aleph_protocol::subagent_tree::SubagentNode) -> Vec<String> {
    let mut lines = vec![format!("\u{2500}\u{2500} {}", node.task)];
    let mut meta = format!("  \u{00b7} {:?}", node.lifecycle).to_lowercase();
    if let Some(model) = node.model.as_deref() {
        meta.push_str(&format!(" \u{00b7} {model}"));
    }
    meta.push_str(&format!(" \u{00b7} {} tools", node.tool_count));
    if let Some(tokens) = node.total_tokens {
        meta.push_str(&format!(" \u{00b7} {tokens} tokens"));
    }
    lines.push(meta);
    if let Some(preview) = node.result_preview.as_deref() {
        lines.push(format!("  \u{00b7} result: {preview}"));
    }
    lines
}

/// Flatten a `chat.history` response into displayable lines: a `── role ──`
/// separator per row, then the content verbatim (the widget wraps).
fn transcript_lines(result: &Value) -> Vec<String> {
    let rows = result
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rows.is_empty() {
        return vec!["(the child transcript is empty)".to_string()];
    }
    let mut lines = Vec::new();
    for row in &rows {
        let role = row.get("role").and_then(Value::as_str).unwrap_or("?");
        let content = row.get("content").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("\u{2500}\u{2500} {role}"));
        lines.extend(content.lines().map(str::to_string));
        lines.push(String::new());
    }
    lines
}

// ---------------------------------------------------------------------------
// Provider / model picker.
// ---------------------------------------------------------------------------

/// Fetch the provider/model catalogue and open the picker overlay.
///
/// `view: all` on purpose. The narrower `configured` default hides every preset
/// the operator has not linked yet — which is exactly the row someone opens a
/// provider browser to find. Deciding what to show from what the server sent is
/// this client's whole job here; deciding which models exist is not (R4).
async fn execute_providers(state: &mut AppState, client: &AlephClient, query: String) {
    // Built from the contract type, not a `json!` literal. This crate cannot
    // depend on `alephcore`, so a hand-written request shape has nothing to
    // disagree with — which is how `agent.run` and `workspace create` both
    // shipped from here permanently broken, green tests and all.
    let params = CatalogParams::for_view(CatalogView::All);
    match client
        .call::<_, Value>("providers.catalog", Some(params))
        .await
    {
        Ok(result) => match catalog_entries(&result) {
            Ok(entries) if entries.is_empty() => state.add_system_message(
                "The gateway answered, and reports no chat providers at all.".to_string(),
            ),
            Ok(entries) => state.open_provider_picker(entries, query),
            Err(e) => state.add_system_message(format!(
                "Provider catalogue unreadable ({e}); this client and the gateway disagree \
                 about its shape."
            )),
        },
        // A refusal is not an empty catalogue: `providers.*` is operator-only,
        // so a member gets one here and must not be told there are no
        // providers. See `gateway_error`.
        Err(e) => state.add_system_message(gateway_error::explain(&e, "the provider catalogue")),
    }
}

/// Pull the catalogue rows out of the response envelope.
///
/// Deserialised straight into the shared [`CatalogEntry`], so a field this
/// client renders that the server stops sending is a parse failure the user is
/// told about, not a column that quietly prints a placeholder forever.
fn catalog_entries(result: &Value) -> Result<Vec<CatalogEntry>, serde_json::Error> {
    serde_json::from_value::<CatalogResult>(result.clone()).map(|r| r.items)
}

/// Ask the highlighted provider's vendor what it serves now.
///
/// The TUI could see a roster but never ask for a fresh one, so a provider
/// linked outside the terminal showed whatever the last sweep had cached — or
/// nothing at all, with no way to find out whether that was the vendor's answer
/// or simply an absence of one. `providers.modelsRefresh` is the same RPC the
/// Panel's per-row button and `aleph providers models --refresh` call.
///
/// Two round trips on purpose: the refresh writes the on-disk cache, and the
/// catalogue is what folds that cache into `roster`. Reporting the sweep
/// without re-reading would leave the list on screen contradicting the message
/// underneath it.
pub(super) async fn refresh_picker_provider(state: &mut AppState, client: &AlephClient) {
    let Some(id) = state.provider_picker_refresh_target() else {
        return;
    };

    let params = ModelsRefreshParams {
        provider: Some(id.clone()),
    };
    let sweep: ModelsRefreshResult = match client
        .call::<_, Value>("providers.modelsRefresh", Some(params))
        .await
    {
        Ok(result) => match serde_json::from_value(result) {
            Ok(parsed) => parsed,
            Err(e) => {
                state.add_system_message(format!(
                    "Model refresh answered in a shape this client cannot read ({e})."
                ));
                return;
            }
        },
        Err(e) => {
            state.add_system_message(gateway_error::explain(&e, "the model refresh"));
            return;
        }
    };

    // The sweep answers with rows, never with an RPC error — so "it returned"
    // is not "it worked", and the row is where the verdict is.
    state.add_system_message(match sweep.providers.iter().find(|r| r.provider == id) {
        Some(row) => refresh_summary(row),
        // The server answers about every named target since the round that
        // closed the silent skips, so this is a genuinely unexpected shape
        // rather than a state with a story.
        None => format!("The sweep ran and said nothing about '{id}'."),
    });

    match client
        .call::<_, Value>(
            "providers.catalog",
            Some(CatalogParams::for_view(CatalogView::All)),
        )
        .await
    {
        Ok(result) => match catalog_entries(&result) {
            Ok(entries) => state.replace_provider_catalog(entries),
            Err(e) => state.add_system_message(format!(
                "Refreshed, but the catalogue came back unreadable ({e}); \
                 the list above is the one from before."
            )),
        },
        Err(e) => state.add_system_message(gateway_error::explain(&e, "the provider catalogue")),
    }
}

/// One sentence per sweep outcome.
///
/// The verdict comes from [`ModelsRefreshRow::outcome`], not from a local
/// `match (ok, stale)`. This file, the CLI's status column and the Panel's
/// refresh badge each used to derive the tri-state themselves, which held only
/// while there were three states; the wording stays per-face (R4), the
/// classification does not.
///
/// A stale listing is neither of its neighbours: it carries models *and* a
/// failure, and collapsing it into "live" is what makes a dated answer look
/// authoritative. `NotApplicable` is likewise not a failure — nothing broke,
/// this vendor simply publishes no listing — so it must not read as one.
fn refresh_summary(row: &ModelsRefreshRow) -> String {
    let detail = row
        .error
        .as_deref()
        .map_or_else(String::new, |e| format!(" ({e})"));
    match row.outcome() {
        RefreshOutcome::Live => format!("{}: {} models, live.", row.provider, row.models.len()),
        RefreshOutcome::Stale => format!(
            "{}: {} models from the last good snapshot — the live fetch failed{detail}.",
            row.provider,
            row.models.len()
        ),
        RefreshOutcome::NotApplicable => format!(
            "{}: publishes no model list — send /model <id> yourself.",
            row.provider
        ),
        RefreshOutcome::Failed => format!("{}: no listing{detail}.", row.provider),
    }
}

/// Confirm the highlighted picker row.
///
/// A provider row descends into its roster; a model row pins the model. The pin
/// travels as `/model <id>` on the normal gateway path — `slash::parse_input`
/// classifies it as `Gateway`, so it reaches `select_model`, which stays the
/// single authority on a session's model (it writes both the process-global map
/// the run builder reads and the session row). The picker adds no second
/// writer, and in particular does not patch `identity_meta.custom`: a pin
/// written there alone is honoured after a restart and ignored before one.
pub(super) async fn confirm_provider_pick(state: &mut AppState, client: &AlephClient) {
    match state.selected_provider_pick() {
        Some(app::ProviderPick::Provider(index)) => state.enter_provider(index),
        Some(app::ProviderPick::Model(id)) => {
            state.close_overlay();
            let command = format!("/model {id}");
            state.add_user_message(command.clone(), vec![]);
            send_to_agent(state, client, &command, "Model select error").await;
        }
        // Nothing highlighted (the filter matched nothing) — closing is the
        // honest answer; guessing a row is not.
        None => state.close_overlay(),
    }
}

/// Whether an attach ADDS to what is on screen or REPLACES it.
///
/// Not cosmetic. Launch and `/session` both prepare the list themselves — the
/// first keeps the welcome banner and the startup notices, the second wipes it
/// in `switch_session` — so for them this call appends. A reconnect has no such
/// preparation step and must not get one: clearing before the fetch means a
/// `chat.history` that fails leaves the user staring at a blank screen on a
/// connection that just came back, which is a worse outcome than the stale
/// transcript it replaced.
///
/// `Replace` therefore clears **inside the success branch**, once the server's
/// copy is actually in hand. A failed reattach then costs one error line and
/// nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttachMode {
    /// Append to what the caller has already prepared.
    #[cfg(test)]
    Append,
    /// Swap in the server's copy, but only once it has arrived.
    Replace,
}

/// Load a conversation: its transcript **and** the settings that govern it.
///
/// The single attach path, used at launch, after a switch, and after a
/// reconnect. One call, because the transcript and the settings are one
/// snapshot — a second RPC would open a window in which the screen shows a
/// conversation while the status bar describes a different one's mode, tier
/// and token count.
///
/// This is also why launching the TUI on an existing key finally shows
/// anything: `chat.history` was previously reached *only* from the session
/// picker, so `aleph-tui --session <key>` opened a blank screen over a
/// transcript the server had all along.
///
/// Failure is reported and survivable: a conversation that cannot be loaded
/// leaves the client on the key it was given with no settings, which the status
/// bar renders as "unknown" rather than as the global defaults. `AttachMode`
/// exists so that survivability holds for the reconnect path too — see its doc.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct SessionAcquisition {
    pub session_key: String,
    pub generation: u64,
    pub history: Value,
    pub traces: Option<Value>,
}

/// RPC-only acquisition: safe to enqueue on a bounded background worker.
/// No mutable AppState is held over either await. A trace failure degrades
/// explicitly to history-only, never a fabricated complete replay.
pub(super) async fn acquire_session_snapshot(
    client: &AlephClient,
    key: String,
    generation: u64,
) -> Result<SessionAcquisition, String> {
    let history: Value = client
        .call("chat.history", Some(json!({ "session_key": key })))
        .await
        .map_err(|e| e.to_string())?;
    let active_run = active_run_from_history(&history)
        .flatten()
        .map(|active| active.run_id);
    let run_ids: Vec<String> = history
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|v| v.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|v| {
            v.get("run_id")
                .or_else(|| v.get("metadata").and_then(|m| m.get("run_id")))
                .and_then(Value::as_str)
        })
        .filter(|id| Some(*id) != active_run.as_deref())
        .map(str::to_owned)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let traces = if run_ids.is_empty() {
        Some(json!({ "runs": {} }))
    } else {
        // The server accepts at most 200 run IDs per request. Retain each
        // successful batch without claiming missing/expired runs are complete.
        let mut runs = serde_json::Map::new();
        let mut available = false;
        for batch in run_ids.chunks(200) {
            if let Ok(reply) = client
                .call::<_, Value>(
                    "trace.by_runs",
                    Some(json!({"session_key": key, "run_ids": batch})),
                )
                .await
            {
                if let Some(batch_runs) = reply.get("runs").and_then(Value::as_object) {
                    runs.extend(batch_runs.clone());
                    available = true;
                }
            }
        }
        available.then(|| json!({"runs": runs}))
    };
    Ok(SessionAcquisition {
        session_key: key,
        generation,
        history,
        traces,
    })
}

/// UI-only application, guarded against a session switch or a newer load.
/// Called by the main loop upon delivery of a tagged RPC job result.
pub(super) fn apply_acquired_session(
    state: &mut AppState,
    acquired: SessionAcquisition,
    mode: AttachMode,
) -> bool {
    if state.session_key != acquired.session_key
        || state.transcript_generation != acquired.generation
    {
        return false;
    }
    let active = active_run_from_history(&acquired.history);
    if state.current_run.is_some() {
        state.transcript_needs_resync = true;
        // Frames may have arrived during the two RPC calls. Without a
        // watermark the snapshot cannot be spliced into that live run.
        return false;
    }
    let rows = acquired
        .history
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let built = app::build_completed_history(
        acquired.session_key,
        acquired.generation,
        &rows,
        acquired.traces.as_ref(),
        active
            .as_ref()
            .and_then(|a| a.as_ref())
            .map(|a| a.run_id.as_str()),
    );
    if mode == AttachMode::Replace {
        if !state.apply_completed_hydration(built) {
            return false;
        }
    } else {
        for row in built.transcript.entries() {
            state.messages.import_legacy(row.clone());
        }
        state.invalidate_rendered_lines();
    }
    apply_history_settings(state, &acquired.history);
    true
}

/// Apply one `chat.history` response to this screen.
///
/// Split out from the call so the decision it carries is reachable without a
/// gateway — and the decision is WHEN, not just whether: `AttachMode::Replace`
/// drops the old transcript here, which is inside the caller's `Ok` arm by
/// construction. A reattach whose fetch failed therefore cannot blank the
/// screen, because this never runs.
#[cfg(test)]
fn apply_history(state: &mut AppState, result: &Value, mode: AttachMode) {
    let rows = result
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped: Vec<app::TranscriptEntry> = rows
        .iter()
        .enumerate()
        .filter_map(|(idx, v)| history_message_from_json(idx, v))
        .collect();

    // The server's copy is in hand, so it is now safe to drop what was on
    // screen. Doing this before the call — the obvious place — turns a
    // transient `chat.history` failure on a freshly-restored connection into a
    // blank screen.
    if mode == AttachMode::Replace {
        state.messages.clear();
        // The cache is keyed by positional index into `messages`, which is
        // about to be repopulated from scratch — a stale entry whose (kind,
        // len, width) happens to match new content at the same index must
        // not survive.
        state.chat_line_cache = crate::tui::widgets::chat_area::LineCache::default();
    }

    // Render the server transcript verbatim (no local dedup/store).
    for msg in mapped {
        state.messages.push(msg);
    }

    apply_history_settings(state, result);
}

fn apply_history_settings(state: &mut AppState, result: &Value) {
    // Restore the conversation's durable settings. Absent on an older
    // gateway — read as "I was not told", so the caption falls back to
    // the install default rather than to a value this client made up.
    match result.get("session").cloned() {
        Some(v) => match serde_json::from_value::<SessionSnapshot>(v) {
            Ok(snapshot) => state.apply_session_snapshot(snapshot),
            Err(e) => state.add_system_message(format!(
                "Session settings unreadable ({e}); showing install defaults."
            )),
        },
        None => state.add_system_message(
            "Gateway did not report session settings (older server); \
             showing install defaults."
                .to_string(),
        ),
    }

    // The conversation's execution list — the disk markdown is the truth the
    // model, the prompt layer and the stop guard all read, and this field is
    // its only cold-load carrier. A THREE-state read (the same one the Panel
    // does): absent = older gateway, learn nothing; `null` = asked and
    // answered, no plan; object = the plan. Collapsing absent into "no plan"
    // would clear a checklist on the say-so of a server that never spoke.
    match result.get("plan") {
        None => {}
        Some(Value::Null) => state.plan = None,
        Some(v) => {
            // Unreadable is "I have no answer", not "there is none" — the
            // failed parse keeps whatever this screen already knew.
            if let Ok(plan) =
                serde_json::from_value::<aleph_protocol::plan::PlanSnapshot>(v.clone())
            {
                state.plan = Some(plan);
            }
        }
    }

    // Which run — if any — is in flight on this session right now.
    //
    // The field is always emitted by a core that has it (`null` when
    // nothing is running), so its PRESENCE is the reconciliation
    // signal and its value is the run to join; an older gateway omits
    // it, and this screen then stays in the fail-open posture it has
    // always had. Deliberately read from the same response as the
    // transcript rather than through a second RPC: a client that holds
    // the transcript but not the run (or the reverse) renders either a
    // duplicated turn or a missing one.
    if let Some(active) = active_run_from_history(result) {
        state.adopt_active_run(active);
    }

    // What the conversation's PREVIOUS run did, once the live one (if any) has
    // been adopted — the two are different questions and this one is only
    // answerable from the snapshot: the transcript of a run that was cut off
    // looks exactly like the transcript of a run that finished.
    //
    // Read off the snapshot this screen just applied rather than off `result`
    // a second time, so the notice and the status bar can only ever describe
    // the same answer. Absent (an older gateway, or a core with no event store
    // to ask) says nothing at all — never "it was fine".
    if let Some(notice) = state
        .session_snapshot
        .as_ref()
        .and_then(|s| s.last_run.as_ref())
        .and_then(last_run_notice)
    {
        state.add_system_message(notice);
    }

    state.scroll_to_bottom();
}

/// The one sentence this client says about a conversation's newest run, or
/// `None` when there is nothing to say.
///
/// The numbers come from the reduction the server already did
/// (`RunProgressView` and the dangling list) — this screen never recounts them
/// from the transcript, because two derivations of "how far did it get" are two
/// answers and the wrong one is unfalsifiable from here.
///
/// `inspected == false` is the list face's answer: the word and nothing else.
/// The counts are withheld then rather than printed as zeroes, which would read
/// as "nothing was lost" off a face that never looked.
fn last_run_notice(last_run: &LastRunState) -> Option<String> {
    // One split, from the protocol's own count: the calls the log proves
    // never completed, and the rest, whose outcome nobody can vouch for. Both
    // halves are `None` together — the list face never looked.
    let counts = last_run
        .dangling()
        .zip(last_run.never_completed_count())
        .map(|(d, never_completed)| (d.len() - never_completed, never_completed));
    let parked_line = |m: usize| {
        format!("上一轮有 {m} 次工具调用未完成 — 服务停止时它们还在等审批 / 等回答，或已被拒绝")
    };
    let with_parked = |base: String, m: usize| {
        if m > 0 {
            format!("{base}；{}", parked_line(m))
        } else {
            base
        }
    };
    match last_run.disposition() {
        LastRunDisposition::LogInconsistent => {
            let tags = if last_run.contradictions.is_empty() {
                "未报告".to_string()
            } else {
                last_run.contradictions.join("、")
            };
            Some(format!(
                "会话日志不一致（{tags}）— 恢复已拒绝，请运行 al doctor"
            ))
        }
        LastRunDisposition::Interrupted => Some(match (last_run.progress, counts) {
            (Some(p), Some((unknown, never_completed))) => with_parked(
                format!(
                    "上一轮运行被中断 — {}/{} 次工具回执已落盘，{unknown} 次结果未知",
                    p.tool_calls_answered, p.tool_calls_dispatched
                ),
                never_completed,
            ),
            _ => "上一轮运行被中断".to_string(),
        }),
        LastRunDisposition::Unrecognized => Some(format!(
            "上一轮运行状态未知（{}）— 本客户端无法判断",
            last_run.disposition
        )),
        // No numbers: nothing ran, so there is nothing to count. The server
        // stopped between the seed and the run's own marker; recovery retries.
        LastRunDisposition::Unanswered => {
            Some("上一条消息没有得到回答 — 运行在开始前就中断了，恢复会重试".to_string())
        }
        // A log can hold dispatched calls that never came back and still carry
        // no run marker at all, which reduces to `never_ran`. Keying the notice
        // on the word alone would leave those calls produced by the server and
        // rendered by nobody (criterion #17).
        LastRunDisposition::Clean | LastRunDisposition::NeverRan => match counts {
            Some((unknown, never_completed)) if unknown > 0 => Some(with_parked(
                format!("上一轮留下 {unknown} 次未回执的工具调用 — 结果未知"),
                never_completed,
            )),
            Some((0, never_completed)) if never_completed > 0 => Some(parked_line(never_completed)),
            _ => None,
        },
    }
}

/// Which run `chat.history` reports in flight on this session — a THREE-way
/// answer, and the outer layer is the load-bearing one.
///
/// - `None` — the field is absent. A gateway older than it; this screen has
///   learned nothing and stays in its fail-open posture.
/// - `Some(None)` — asked and answered: nothing is running here. That is what
///   arms `AppState::session_reconciled`, and it is the common case.
/// - `Some(Some(join))` — a turn to join, with the age the server measured.
///
/// Collapsing the outer two (reading absent as "nothing running") would arm the
/// guard against a server that never told it anything, and this screen would
/// then drop every frame of every run it did not personally start.
fn active_run_from_history(result: &Value) -> Option<Option<app::ActiveRunJoin>> {
    let value = result.get("active_run")?;
    let Some(run_id) = value.as_str().filter(|s| !s.is_empty()).map(str::to_string) else {
        return Some(None);
    };
    // Taken from the SAME response, never re-derived by a second call: the
    // field is a duration the server measured at the instant it answered, so
    // asking again would be measuring a different instant against a transcript
    // this screen already holds. Absent (older gateway, or the run left the
    // engine's table between that handler's two lookups) means "not told",
    // which the join reads as a floor.
    let elapsed_ms = result.get("active_run_elapsed_ms").and_then(Value::as_u64);
    Some(Some(app::ActiveRunJoin { run_id, elapsed_ms }))
}

/// Map one `chat.history` row (`{role, content, timestamp}`) into a transcript
/// entry.
///
/// `idx` only supplies the entry id, which for a history row need only be
/// stable within the loaded batch — the ids that have to mean something across
/// clients belong to tool rows, and those come from the call id.
///
/// History carries no tool calls; `trace.by_runs` is the surface that replays
/// those, and it has no client yet (Phase A shipped it with none). A restored
/// transcript is therefore text-only, which is what this client showed before
/// as well.
#[cfg(test)]
fn history_message_from_json(idx: usize, v: &Value) -> Option<app::TranscriptEntry> {
    let role = v.get("role").and_then(Value::as_str).unwrap_or("");
    let content = v
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let id = format!("h{idx}");
    match role {
        "user" => Some(app::TranscriptEntry::UserText {
            id,
            text: content,
            at_ms: u64::try_from(
                app::row_timestamp(v.get("timestamp").and_then(Value::as_str)).timestamp_millis(),
            )
            .ok(),
            // History rows from before the field existed — and history rows
            // from after it too, since the wire does not yet carry per-row
            // attachments — render with no chips. The compose send path uses
            // `add_user_message`, which already wires the real list.
            attachments: Vec::new(),
        }),
        "assistant" => Some(app::TranscriptEntry::AssistantText {
            id,
            markdown: content,
            streaming: false,
        }),
        "system" => Some(app::TranscriptEntry::SystemNotice { id, text: content }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Control-panel command handlers
// ---------------------------------------------------------------------------

/// Response shape for `session.usage` RPC. Subset of the full reply.
#[derive(Debug, serde::Deserialize)]
struct UsageReply {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    messages: u64,
    #[serde(default)]
    cost_usd: Option<f64>,
}

async fn execute_usage(state: &mut AppState, client: &AlephClient) {
    let params = json!({ "session_key": state.session_key });
    match client
        .call::<_, UsageReply>("session.usage", Some(params))
        .await
    {
        Ok(usage) => state.add_system_message(format_usage(state, &usage)),
        Err(e) => state.add_system_message(format!("Usage error: {e}")),
    }
}

/// Render the `/usage` cost line from the daemon-computed figure. Pure so it
/// is unit-testable; the TUI no longer owns any pricing (R4).
fn cost_line(model: &str, cost_usd: Option<f64>) -> String {
    match cost_usd {
        Some(usd) => format!("Cost estimate ({model}): ${usd:.4}"),
        None => format!("Cost: n/a (no pricing entry for {model})"),
    }
}

fn format_usage(state: &AppState, u: &UsageReply) -> String {
    [
        format!(
            "Session usage — messages: {}  input: {}  output: {}  total: {}",
            u.messages, u.input_tokens, u.output_tokens, u.tokens
        ),
        cost_line(&state.model_name, u.cost_usd),
    ]
    .join("\n")
}

/// Response shape for `session.compact` RPC.
///
/// The server renders the human-readable line (it knows whether the run was a
/// no-op, and why), so this is a thin carrier — R4: the interface renders, it
/// does not re-derive.
#[derive(Debug, serde::Deserialize)]
struct CompactReply {
    #[serde(default)]
    message: String,
    #[serde(default)]
    summary: String,
}

/// How much of the summary to echo back in the TUI. The full text also lands in
/// the conversation as a system message, so this is a preview, not the payload.
const SUMMARY_PREVIEW_CHARS: usize = 400;

async fn execute_compress(state: &mut AppState, client: &AlephClient, args: &str) {
    if state.current_run.is_some() {
        state.add_system_message(
            "Wait for the current run to finish before compacting (/stop to abort)".to_string(),
        );
        return;
    }
    // The gateway routes `session.compact` by session_key; sending the call
    // with an empty key would either be rejected or land on whatever default
    // the server uses, neither of which matches the user's intent (the TUI
    // is unattached at this point — there is no conversation to compact).
    // Surface the situation in the panel instead of leaking a useless RPC.
    if state.session_key.is_empty() {
        state.add_system_message(
            "Cannot /compress: no session attached. Send a message first.".to_string(),
        );
        return;
    }
    // `/compress <instructions>` — the trailing free text steers what the
    // summary must preserve (codex / pi / kimi-cli parity), matching what the
    // Panel's `/compact <instructions>` sends through the tool path.
    let instructions = args.trim();
    let mut params = json!({ "session_key": state.session_key });
    if !instructions.is_empty() {
        params["instructions"] = json!(instructions);
    }
    match client
        .call::<_, CompactReply>("session.compact", Some(params))
        .await
    {
        Ok(r) => {
            state.add_system_message(r.message);
            if !r.summary.trim().is_empty() {
                // P7 UTF-8 safety: cut on a char boundary, never a byte one.
                let preview: String = r.summary.chars().take(SUMMARY_PREVIEW_CHARS).collect();
                let ellipsis = if r.summary.chars().count() > SUMMARY_PREVIEW_CHARS {
                    "…"
                } else {
                    ""
                };
                state.add_system_message(format!("{preview}{ellipsis}"));
            }
        }
        Err(e) => state.add_system_message(format!("Compact error: {e}")),
    }
}

async fn execute_stop(state: &mut AppState, client: &AlephClient) {
    let Some(run_id) = state.current_run.clone() else {
        state.add_system_message("No active run.".to_string());
        return;
    };
    // Scoped to the session, not just the run: cancelling frees the session
    // slot and the gateway's wait lane would otherwise fire whatever the user
    // had queued behind this run, one full turn at a time — the opposite of
    // what pressing stop means.
    let params = json!({ "run_id": run_id, "session_key": state.session_key });
    match client.call::<_, Value>("chat.abort", Some(params)).await {
        Ok(reply) => {
            state.current_run = None;
            state.run_started_at = None;
            let dropped = reply
                .get("dropped")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let backlog = match dropped {
                0 => String::new(),
                1 => " 1 queued message dropped.".to_string(),
                n => format!(" {n} queued messages dropped."),
            };
            state.add_system_message(format!("Run aborted ({run_id}).{backlog}"));
        }
        Err(e) => state.add_system_message(format!("Abort error: {e}")),
    }
}

/// Response shape for `session.truncate` RPC.
#[derive(Debug, serde::Deserialize)]
struct TruncateReply {
    #[serde(default)]
    messages_removed: u64,
    #[serde(default)]
    tokens_removed_estimate: u64,
}

/// Resolve `session.truncate`'s `keep_count` for "drop the last turn", in the
/// index space the SERVER counts in.
///
/// `keep_count` is an ordinal over the stored `messages` rows. This screen's
/// `state.messages` is not those rows: `history_message_from_json` maps every
/// `tool` row to `None`, and the live stream never appends one at all — so two
/// turns with two tool calls each are 8 stored rows and 4 rendered ones. A count
/// taken from the rendered list therefore names a boundary several turns earlier
/// than the user's last one, and `session.truncate` is irreversible. (Same shape
/// as the "storage form vs display form" criterion in CLAUDE.md §0.)
///
/// So the boundary is derived from a fresh `chat.history`: keep everything
/// strictly before the newest `user` row. `total` is what makes this exact when
/// the response is a tail WINDOW rather than the whole transcript — without it
/// the window's own length would masquerade as the transcript's.
fn keep_count_before_last_turn(result: &Value) -> Option<usize> {
    let rows = result.get("messages").and_then(Value::as_array)?;
    let last_user = rows
        .iter()
        .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"))?;
    // `total` absent (older gateway) ⇒ the window is all there is.
    let total = result
        .get("total")
        .and_then(Value::as_u64)
        .map_or(rows.len(), |t| t as usize);
    let dropped_from_end = rows.len() - last_user;
    Some(total.saturating_sub(dropped_from_end))
}

async fn execute_undo(state: &mut AppState, client: &AlephClient) -> bool {
    if state.current_run.is_some() {
        state.add_system_message("Stop the active run first (/stop), then /undo.".to_string());
        return false;
    }
    // Ask the server where the last turn starts. Counting locally would be
    // counting in the wrong index space — see `keep_count_before_last_turn`.
    let history = match client
        .call::<_, Value>(
            "chat.history",
            Some(json!({ "session_key": state.session_key })),
        )
        .await
    {
        Ok(h) => h,
        Err(e) => {
            state.add_system_message(format!("Undo error: {e}"));
            return false;
        }
    };
    let Some(keep_count) = keep_count_before_last_turn(&history) else {
        state.add_system_message("Nothing to undo.".to_string());
        return false;
    };
    let params = json!({
        "session_key": state.session_key,
        "keep_count": keep_count,
    });
    match client
        .call::<_, TruncateReply>("session.truncate", Some(params))
        .await
    {
        Ok(r) => {
            pop_last_turn_locally(state);
            state.add_system_message(format!(
                "Reverted last turn (-{} messages, ~{} tokens).",
                r.messages_removed, r.tokens_removed_estimate
            ));
            true
        }
        Err(e) => {
            state.add_system_message(format!("Undo error: {e}"));
            false
        }
    }
}

async fn execute_retry(state: &mut AppState, client: &AlephClient) {
    if state.current_run.is_some() {
        state.add_system_message("Stop the active run first (/stop), then /retry.".to_string());
        return;
    }
    let Some(last_user) = last_user_message(state) else {
        state.add_system_message("Nothing to retry.".to_string());
        return;
    };

    // Phase 1: undo the last turn — abort the retry if the revert didn't happen,
    // otherwise we'd re-send on top of the still-present old turn (duplicated turn).
    if !execute_undo(state, client).await {
        return;
    }

    // Phase 2: re-submit the captured user message via the shared send site
    state.add_user_message(last_user.clone(), vec![]);
    state.push_send_history(last_user.clone());
    send_to_agent(state, client, &last_user, "Retry send error").await;
}

fn execute_tools(state: &mut AppState, mode: Option<ToolProgressMode>) {
    match mode {
        Some(m) => {
            state.tool_progress_mode = m;
            state.add_system_message(format!("Tool progress: {m}"));
        }
        None => {
            state.add_system_message(format!(
                "Tool progress: {} (usage: /tools off|new|all|verbose)",
                state.tool_progress_mode
            ));
        }
    }
}

/// Set (or report) one of this conversation's persisted knobs via
/// `sessions.patch`.
///
/// One handler for the family. Each knob writes the same
/// `identity_meta.custom` bag the Panel pills and the run loop already use, so
/// a value set here survives a restart and is read back by the attach snapshot
/// — that contract is identical for all four. Writing a handler per knob is how
/// `/tier` came to exist while `/mode`, `/think` and `/memory` did not, even
/// though all three were already being enforced on every turn.
///
/// `default` clears the override: the server reads a JSON `null` as "follow
/// global", which is the only way back to the install-wide value.
///
/// Local state is updated ONLY after the server accepts the write. An
/// optimistic update on a refused patch would leave the status bar confidently
/// describing a setting the session does not have.
async fn execute_knob(
    state: &mut AppState,
    client: &AlephClient,
    knob: SlashKnob,
    value: Option<String>,
) {
    let Some(value) = value else {
        let current = current_knob_value(state, knob).map_or_else(
            || "follows the global default".to_string(),
            |v| format!("`{v}`"),
        );
        state.add_system_message(format!(
            "/{cmd} — {purpose}. Currently {current}. Usage: /{cmd} {choices}              (or /{cmd} default to follow the global policy).",
            cmd = knob.command(),
            purpose = knob.purpose(),
            choices = knob.choices(),
        ));
        return;
    };

    if state.session_key.is_empty() {
        state.add_system_message(format!(
            "/{}: this conversation has no session yet — send a message first, then set it.",
            knob.command()
        ));
        return;
    }

    // `default` clears the override. JSON null is how both stores spell "remove
    // this key"; omitting it would mean "leave it alone".
    let wire = if value == "default" {
        Value::Null
    } else {
        Value::String(value.clone())
    };
    let params = json!({
        "session_key": state.session_key,
        "metadata": { knob.metadata_key(): wire },
    });
    match client
        .call::<_, Value>("sessions.patch", Some(params))
        .await
    {
        Ok(_) => {
            let stored = (value != "default").then(|| value.clone());
            state.record_local_knob(knob, stored);
            let shown = if value == "default" {
                "the global default".to_string()
            } else {
                format!("`{value}`")
            };
            state.add_system_message(format!("/{} now follows {shown}.", knob.command()));
        }
        Err(e) => state.add_system_message(format!("/{} error: {e}", knob.command())),
    }
}

/// The knob's current value as the attach snapshot last reported it.
fn current_knob_value(state: &AppState, knob: SlashKnob) -> Option<String> {
    let knobs = state.session_knobs();
    match knob {
        SlashKnob::ExecTier => knobs.exec_tier,
        SlashKnob::Mode => knobs.mode,
        SlashKnob::Think => knobs.think_level,
        SlashKnob::Memory => knobs.memory_mode,
    }
    .map(str::to_string)
}

/// Return a clone of the last User message's content, if any.
fn last_user_message(state: &AppState) -> Option<String> {
    state.messages.iter().rev().find_map(|m| match m {
        app::TranscriptEntry::UserText { text, .. } => Some(text.clone()),
        _ => None,
    })
}

/// Locally pop the last user+assistant turn from chat history, after a
/// successful server-side truncate. Keeps the TUI in sync without a full
/// reload round-trip.
///
/// # Why this counts back to a user message instead of popping two entries
///
/// A turn is no longer two entries. Since tool rows and reasoning became peers
/// of the text, one assistant turn is however many entries it produced — text,
/// a tool row, more text — so "pop the assistant, pop the user" would strip a
/// fragment of the turn and leave the rest of it orphaned above a user message
/// that is no longer there.
///
/// Everything after the last `UserText` is that user message's turn, so the
/// boundary is found rather than counted.
fn pop_last_turn_locally(state: &mut AppState) {
    let last_user = state
        .messages
        .iter()
        .rposition(|m| matches!(m, app::TranscriptEntry::UserText { .. }));
    let Some(at) = last_user else {
        // No user message to unwind to: only trailing notices can be dropped,
        // and dropping the whole transcript would be a far bigger edit than
        // the undo the user asked for.
        while matches!(
            state.messages.last(),
            Some(app::TranscriptEntry::SystemNotice { .. })
        ) {
            state.messages.pop();
        }
        return;
    };
    state.messages.truncate(at);
}

pub(super) fn format_replay_list(page: &AgentTraceListPage) -> String {
    if page.traces.is_empty() {
        return "Recent replays:\n  (none)\n\nUse /replay <task_id> after traces are persisted."
            .to_string();
    }

    let mut lines = vec!["Recent replays:".to_string()];
    for task in &page.traces {
        lines.push(format!(
            "  {} [{}] {} traces  {}",
            task.task_id,
            task.status,
            task.event_count,
            truncate_text(&task.prompt_preview, 72)
        ));
    }
    lines.push(String::new());
    if page.next_cursor.is_some() {
        lines.push(format!("(showing {} — more exist)", page.traces.len()));
    }
    lines.push("Use /replay <task_id> to load one.".to_string());
    lines.join("\n")
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let end = text
        .char_indices()
        .nth(max_chars)
        .map_or(text.len(), |(idx, _)| idx);
    format!("{}...", &text[..end])
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the help text shown by /help.
fn build_help_text(state: &AppState) -> String {
    let mut lines = vec!["Local commands:".to_string()];
    for (name, desc) in slash::local_commands() {
        lines.push(format!("  {name:<14} {desc}"));
    }

    if !state.gateway_commands.is_empty() {
        lines.push(String::new());
        lines.push("Gateway commands (handled by server):".to_string());
        for entry in &state.gateway_commands {
            if entry.is_namespace {
                let param = entry
                    .param_hint
                    .as_deref()
                    .map(|p| format!(" {p}"))
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
                        .map(|p| format!(" {p}"))
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
                    .map(|p| format!(" {p}"))
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
    lines.push("  \\ + Enter      Insert newline (portable)".to_string());
    lines.push("  Ctrl+J         Insert newline (portable)".to_string());
    lines.push("  Shift+Enter    Insert newline (enhanced terminals)".to_string());
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
/// Local command words that would swallow a gateway command of the same name.
///
/// The TUI resolves local commands first and unconditionally, so a local word
/// that matches a gateway one does not "take precedence" — it makes the gateway
/// command **unreachable**, with no error anywhere. `/memory` was exactly that:
/// the gateway namespaces `memory_search` / `memory_browse` / `memory_explore`
/// under it, and a knob command claiming the bare word would have silently
/// deleted all of them (caught by a parser test, which is luck, not a guard).
///
/// Checked at runtime against the list the gateway actually publishes rather
/// than against a hardcoded set: the gateway's namespaces come from its live
/// tool catalog, which grows with every installed skill, MCP server and plugin
/// — a compile-time list here would describe the world on the day it was
/// written and go quietly out of date, which is the failure mode it would exist
/// to prevent.
#[must_use]
pub(super) fn shadowed_gateway_commands(gateway: &[command_tree::CommandEntry]) -> Vec<String> {
    let mut clashes: Vec<String> = slash::local_commands()
        .iter()
        .map(|(name, _)| name.trim_start_matches('/').to_string())
        .filter(|local| gateway.iter().any(|g| g.name == *local))
        .collect();
    clashes.sort_unstable();
    clashes.dedup();
    clashes
}

pub(super) async fn fetch_gateway_commands(
    client: &AlephClient,
) -> Vec<command_tree::CommandEntry> {
    client
        .call::<_, Value>("commands.list", Some(json!({"interface": "tui"})))
        .await
        .map_or_else(
            |_| {
                // Old Gateway or connection issue — graceful degradation
                Vec::new()
            },
            |result| command_tree::CommandEntry::parse_from_json(&result),
        )
}

/// This client's own principal id, or `None` when it cannot be established.
///
/// Needed to tell "somebody else typed this" from "I typed this" on
/// `stream.session_user_message`. Author identity is the discriminator rather
/// than run ownership because the frame can arrive BEFORE the `chat.send` that
/// would teach this screen its own run id — the two race, and the losing order
/// renders the sender's own message twice.
///
/// Every failure collapses to `None`, and the three that can happen say
/// different things — an older gateway with no `users.me`, a transport error,
/// and a loopback caller with no principal record at all. None of them may be
/// read as "the author is somebody else": `None` disables the echo entirely,
/// which is exactly the behaviour this client had before the frame existed.
/// A peer's message then still arrives, on the next attach, as it always did.
pub(super) async fn fetch_my_user_id(client: &AlephClient) -> Option<String> {
    client
        .call::<_, aleph_protocol::users::UserMeResult>("users.me", Some(json!({})))
        .await
        .ok()?
        .user
        .map(|u| u.user_id)
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_resource_status_classifies_blank_and_missing() {
        let catalog = app::BrowserResourceCatalog::new();
        assert_eq!(
            browser_resource_status(&catalog, "", 0),
            BrowserResourceStatus::Empty
        );
        assert_eq!(
            browser_resource_status(&catalog, "  \t", 0),
            BrowserResourceStatus::Empty
        );
        assert_eq!(
            browser_resource_status(&catalog, "res_a", 0),
            BrowserResourceStatus::Missing
        );
    }

    #[test]
    fn browser_status_notices_are_generic_and_only_available_proceeds() {
        assert_eq!(
            browser_status_notice(BrowserResourceStatus::Empty),
            Some(BROWSER_USAGE)
        );
        let missing = browser_status_notice(BrowserResourceStatus::Missing).unwrap();
        let expired = browser_status_notice(BrowserResourceStatus::Expired).unwrap();
        assert!(missing.contains("not found"));
        assert!(expired.contains("expired"));
        assert_eq!(
            browser_status_notice(BrowserResourceStatus::Available),
            None
        );
    }

    /// Same shape as the `browser_catalog` tests' `resource()` fixture.
    fn browser_fixture(
        id: &str,
        expires_at_ms: i64,
    ) -> aleph_protocol::browser_live::PreviewResource {
        use aleph_protocol::browser_live::{
            BrowserTargetBinding, ConversationBinding, DisplayPolicy, IssuerIdentity, PreviewKind,
            PreviewProvenance, PreviewResource, PreviewSource, RedactionStatus, ResourceBinding,
            ViewerBinding,
        };
        PreviewResource {
            resource_id: id.into(),
            revision: 1,
            binding: ResourceBinding::Browser {
                viewer: ViewerBinding {
                    principal: IssuerIdentity::User {
                        user_id: "u1".into(),
                    },
                    task: ConversationBinding {
                        session_key: "agent:main:main".into(),
                        run_id: "run-1".into(),
                    },
                    target: BrowserTargetBinding {
                        profile: "default".into(),
                        browser_session_id: "bs-1".into(),
                        target_id: "t1".into(),
                    },
                },
            },
            kind: PreviewKind::Page,
            display: DisplayPolicy::ExplicitOnly,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            expires_at_ms,
            redaction: RedactionStatus::Pending,
            companion_candidates: Vec::new(),
        }
    }

    fn catalog_with(entries: &[(&str, i64)]) -> app::BrowserResourceCatalog {
        let mut catalog = app::BrowserResourceCatalog::new();
        for (id, expires_at_ms) in entries {
            let data = json!({
                "type": "browser_live_resource",
                "resource": browser_fixture(id, *expires_at_ms),
            });
            catalog.apply_topic(aleph_protocol::browser_live::RESOURCE_TOPIC, &data);
            assert!(catalog.get(id).is_some(), "fixture not stored: {id:?}");
        }
        catalog
    }

    #[test]
    fn blank_browser_arg_lists_only_unexpired_exact_ids_and_never_proceeds() {
        let catalog = catalog_with(&[
            ("res_live_b", 2_000),
            ("res_live_a", 2_000),
            ("res_old", 500),
        ]);
        for arg in ["", "  \t"] {
            for enabled in [false, true] {
                let BrowserPreflight::Show(text) = browser_preflight(&catalog, enabled, arg, 1_000)
                else {
                    panic!("blank argument must not proceed");
                };
                assert!(text.contains(BROWSER_USAGE));
                assert!(text.contains("\n  res_live_a\n  res_live_b"), "{text}");
                assert!(!text.contains("res_old"), "expired id listed: {text}");
            }
        }
        // Expiry boundary: now == expires_at_ms is expired.
        let BrowserPreflight::Show(text) = browser_preflight(&catalog, true, "", 2_000) else {
            panic!("blank argument must not proceed");
        };
        assert!(!text.contains("res_live_a"));
        assert!(text.contains("No browser resources are currently available."));
    }

    #[test]
    fn browser_listing_with_empty_catalog_says_so_without_inventing_ids() {
        let text = format_browser_listing(&app::BrowserResourceCatalog::new(), 0);
        assert!(text.contains(BROWSER_USAGE));
        assert!(text.contains("No browser resources are currently available."));
        assert!(!text.contains("res_"));
    }

    #[test]
    fn browser_listing_skips_unsafe_ids_and_does_not_forge_a_copyable_one() {
        let long = format!("res_{}", "a".repeat(BROWSER_LIST_ID_MAX_LEN));
        let entries: Vec<(&str, i64)> = vec![
            ("res_\u{1b}[31mred", 2_000),
            ("res_a b", 2_000),
            ("res_\nline", 2_000),
            ("res_\u{e9}", 2_000),
            (long.as_str(), 2_000),
            ("res_ok-1", 2_000),
        ];
        let text = format_browser_listing(&catalog_with(&entries), 1_000);
        assert!(text.contains("\n  res_ok-1"));
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(!text.contains("res_a b") && !text.contains("res_\u{e9}") && !text.contains(&long));
        // Only the safe id is a list line; no sanitized variant of the others.
        assert_eq!(text.lines().filter(|l| l.starts_with("  ")).count(), 1);
        assert!(
            text.contains("(5 resource(s) with a non-displayable id omitted.)"),
            "{text}"
        );
    }

    #[test]
    fn browser_preflight_guards_status_before_the_disabled_gate() {
        let catalog = catalog_with(&[("res_live", 2_000), ("res_old", 500)]);
        let missing = browser_status_notice(BrowserResourceStatus::Missing).unwrap();
        let expired = browser_status_notice(BrowserResourceStatus::Expired).unwrap();
        for enabled in [false, true] {
            assert_eq!(
                browser_preflight(&catalog, enabled, "res_nope", 1_000),
                BrowserPreflight::Show(missing.to_string())
            );
            assert_eq!(
                browser_preflight(&catalog, enabled, "res_old", 1_000),
                BrowserPreflight::Show(expired.to_string())
            );
        }
    }

    #[test]
    fn browser_preflight_returns_before_issue_when_viewer_is_disabled() {
        let catalog = catalog_with(&[("res_live", 2_000)]);
        // Disabled: a live id stops here. The handler returns on `Show`, so
        // `issue_browser_viewer_launch` is never reached and no grant is minted.
        let BrowserPreflight::Show(text) = browser_preflight(&catalog, false, "res_live", 1_000)
        else {
            panic!("disabled viewer must not proceed to the issuer");
        };
        assert_eq!(text, BROWSER_DISABLED_NOTICE);
        assert!(text.contains("ALEPH_VIEWER_ENABLED=1"));
        assert!(text.contains("ALEPH_VIEWER_TRUSTED_ORIGINS"));
        assert!(!text.contains("res_live"));
        // Enabled: only now does it go on to the issuer.
        assert_eq!(
            browser_preflight(&catalog, true, "res_live", 1_000),
            BrowserPreflight::Proceed
        );
    }

    #[test]
    fn browser_request_failure_text_is_fixed_and_carries_no_rpc_text() {
        assert_eq!(
            BROWSER_REQUEST_FAILED,
            "Browser viewer request failed or was denied."
        );
    }

    /// Drive `finish_browser_launch` with a recording revoke; returns the
    /// message and the grant ids revoked.
    async fn run_finish(outcome: BrowserLaunchOutcome, revoke_ok: bool) -> (String, Vec<String>) {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = calls.clone();
        let message = finish_browser_launch("grant_issued_1", outcome, |id| async move {
            sink.lock().unwrap().push(id);
            revoke_ok
        })
        .await;
        let ids = calls.lock().unwrap().clone();
        (message, ids)
    }

    #[tokio::test]
    async fn browser_launch_success_never_revokes() {
        let (message, calls) = run_finish(BrowserLaunchOutcome::Launched, true).await;
        assert_eq!(message, "Browser viewer launched.");
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn browser_launch_failures_revoke_the_exact_grant_once() {
        use crate::viewer_launcher::LaunchError;
        let errors = [
            LaunchError::Disabled,
            LaunchError::InvalidUrl(crate::viewer_launcher::UrlRejection::UntrustedOrigin),
            LaunchError::InvalidExecutable,
            LaunchError::Spawn(std::io::ErrorKind::NotFound),
            LaunchError::NonZeroExit(Some(1)),
            LaunchError::NonZeroExit(None),
            LaunchError::ForbiddenArgument,
        ];
        for e in errors {
            let expected = format_browser_launch_error(&e);
            let (message, calls) = run_finish(BrowserLaunchOutcome::Failed(e), true).await;
            assert_eq!(message, expected);
            assert_eq!(calls, vec!["grant_issued_1".to_string()]);
        }
        let (message, calls) = run_finish(BrowserLaunchOutcome::TaskFailed, true).await;
        assert_eq!(message, "Browser viewer launch task failed.");
        assert_eq!(calls, vec!["grant_issued_1".to_string()]);
    }

    #[tokio::test]
    async fn browser_launch_join_error_revokes() {
        let joined =
            tokio::task::spawn_blocking(|| -> Result<(), crate::viewer_launcher::LaunchError> {
                panic!("simulated launcher panic")
            })
            .await;
        let outcome = browser_launch_outcome(joined);
        assert!(matches!(outcome, BrowserLaunchOutcome::TaskFailed));
        let (_, calls) = run_finish(outcome, true).await;
        assert_eq!(calls, vec!["grant_issued_1".to_string()]);
    }

    #[tokio::test]
    async fn browser_launch_cleanup_failure_keeps_launch_error_and_notes_ttl() {
        use crate::viewer_launcher::LaunchError;
        let e = LaunchError::NonZeroExit(Some(3));
        let original = format_browser_launch_error(&e);
        let (message, calls) = run_finish(BrowserLaunchOutcome::Failed(e), false).await;
        assert_eq!(message, format!("{original}{BROWSER_GRANT_CLEANUP_FAILED}"));
        assert!(message.contains("expire on its own (TTL)"));
        assert!(!message.contains("grant_issued_1"));
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn browser_launch_disabled_points_at_configuration_and_leaks_nothing() {
        use crate::viewer_launcher::LaunchError;
        let disabled = format_browser_launch_error(&LaunchError::Disabled);
        assert!(disabled.contains("disabled by default"));
        assert!(disabled.contains("Enable"));
        assert_eq!(
            format_browser_launch_error(&LaunchError::ForbiddenArgument),
            "Browser viewer launch failed: forbidden browser argument"
        );
    }

    /// `/undo`'s boundary must be an ordinal over the STORED rows.
    ///
    /// The rendered list drops every `tool` row, so a count taken from it names
    /// a boundary several turns earlier than the user's last one — and
    /// `session.truncate` deletes irreversibly and (since it now also retires
    /// the event log) takes the model's memory of those turns with it.
    #[test]
    fn undo_keeps_everything_before_the_last_user_row_counting_stored_rows() {
        let row = |role: &str| serde_json::json!({ "role": role, "content": "x" });
        // Two turns with two tool calls each: 8 stored rows, 4 of them rendered.
        let history = serde_json::json!({
            "messages": [
                row("user"), row("tool"), row("tool"), row("assistant"),
                row("user"), row("tool"), row("tool"), row("assistant"),
            ],
            "total": 8,
        });
        assert_eq!(
            keep_count_before_last_turn(&history),
            Some(4),
            "the boundary must be the newest `user` row's ordinal among STORED \
             rows — counting the rendered list would keep only 2 and silently \
             delete the first turn as well"
        );

        // A tail WINDOW: `total` is what keeps the ordinal absolute.
        let windowed = serde_json::json!({
            "messages": [row("assistant"), row("user"), row("assistant")],
            "total": 103,
        });
        assert_eq!(keep_count_before_last_turn(&windowed), Some(101));

        // Nothing to undo.
        assert_eq!(
            keep_count_before_last_turn(&serde_json::json!({ "messages": [], "total": 0 })),
            None
        );
    }

    #[test]
    fn cost_line_renders_amount_and_na() {
        assert_eq!(
            cost_line("claude-sonnet-4-6", Some(1.2345)),
            "Cost estimate (claude-sonnet-4-6): $1.2345"
        );
        assert!(cost_line("mystery-model", None).contains("n/a"));
    }

    /// Esc while a side question is answering must stop the SIDE run.
    ///
    /// The screen's `current_run` is the main run — reusing `/stop`'s abort
    /// would stop the wrong work — and `session_key` must not travel at all,
    /// because the only key this client holds is the main conversation's and
    /// the server would use it to purge that conversation's queue.
    #[test]
    fn the_side_abort_names_the_side_run_and_no_session() {
        let mut state = AppState::new("agent:main:main".into(), "m".into());
        state.current_run = Some("run-main".into());
        state.open_btw("why?".into());
        state.btw.claim_pending_run("run-side".into());

        let params = side_abort_params(&state).expect("a side question is answering");
        assert_eq!(params["run_id"], "run-side");
        assert_ne!(
            params["run_id"], "run-main",
            "aborting a side question must not stop the main run"
        );
        assert!(
            params.get("session_key").is_none(),
            "a session key here would purge the MAIN conversation's queue: {params}"
        );
    }

    /// Esc with nothing answering is a close, not an abort — and the way that
    /// is expressed is `None`, so the caller cannot send an abort naming
    /// nothing.
    #[test]
    fn there_is_nothing_to_abort_once_the_answer_has_settled() {
        let mut state = AppState::new("agent:main:main".into(), "m".into());
        assert!(side_abort_params(&state).is_none(), "nothing asked yet");

        state.open_btw("why?".into());
        assert!(
            side_abort_params(&state).is_none(),
            "no run id yet: agent.run has not replied, so there is nothing to abort"
        );

        state.btw.claim_pending_run("run-side".into());
        assert!(side_abort_params(&state).is_some());

        state.btw.finish_active("run-side", Some("because"));
        assert!(
            side_abort_params(&state).is_none(),
            "a settled answer is closed, not aborted"
        );
    }

    /// Is byte offset `at` inside a double-quoted literal on this one line?
    ///
    /// Deliberately per line: a Rust literal can span lines, but tracking that
    /// needs a lexer, and this crate has none. Resetting at each newline bounds
    /// a miscount to the line it happens on — the failure the unbounded version
    /// produces (an odd quote desynchronising and blanking everything after it)
    /// is silent, and this one is not.
    ///
    /// Whoever gives this crate a lexer should delete this function. Nothing
    /// is lost by that: the crate-boundary ruling this module carries lives on
    /// the census below, on its `#[test]`, not up here.
    fn inside_a_literal(line: &str, at: usize) -> bool {
        let mut quotes = 0usize;
        let mut escaped = false;
        for (i, c) in line.char_indices() {
            if i >= at {
                break;
            }
            if escaped {
                escaped = false;
                continue;
            }
            match c {
                '\\' => escaped = true,
                '"' => quotes += 1,
                _ => {}
            }
        }
        quotes % 2 == 1
    }

    /// This client asks "is this a side question?" in exactly one place.
    ///
    /// A source-level census, because the failure it guards against is not a
    /// wrong answer but a *second* answer: a prefix test written somewhere
    /// else would agree with the shared resolver today and drift from it the
    /// first time the server learns a spelling (`/BTW`, `/btw@bot`, the empty
    /// body that is deliberately NOT a side question). Runtime cannot tell the
    /// two apart — both say yes to `/btw x` — so only the source can.
    ///
    /// Comment lines are stripped first: the doc above this very test names
    /// the resolver, and a scanner that counted prose would be satisfied by a
    /// sentence rather than by a call. The `#[cfg(test)]` split is not
    /// anchored to line ends, because this repo's Windows checkout is CRLF and
    /// `"\n#[cfg(test)]\n"` matches nothing there — which would silently turn
    /// the whole file into "production" and let a test's own literals satisfy
    /// the census.
    ///
    /// # The cut is hand-rolled here, and what that costs
    ///
    /// `src.split("#[cfg(test)]").next()` is a PREFIX cut: it stops at the
    /// first `#[cfg(test)]` marker in the file, so production code below a
    /// gated `use`, helper `fn` or `mod` is never scanned. The failure is
    /// one-directional — a prefix cut can only ever under-scan — and therefore
    /// silent: a second `BtwTurn::resolve(` hidden down there leaves
    /// `hits.len() == 1` and this census reports the invariant intact.
    ///
    /// The single source for this question is
    /// `alephcore::utils::source_scan::production_prefix`, which walks gated
    /// ITEMS and lexes strings, char literals and block comments across lines.
    /// **`aleph-tui` cannot call it**: it is a separate workspace crate that
    /// does not depend on the server library `alephcore`, and the
    /// capability-wiring spec's non-goal 1 (不拆 crate — `alephcore` stays one
    /// crate this round) rules out moving the two functions somewhere both
    /// crates could reach. `aleph-panel` solved the same problem in-crate with
    /// `i18n_census::production_lines`; this crate has no second source-level
    /// guard to share such a helper with, so it carries the cut and the
    /// self-check below instead.
    ///
    /// The self-check is what makes the blindness loud rather than green: it
    /// scans the discarded remainder for the same needle and fails if one is
    /// hiding there. It cannot tell production from test code past the cut —
    /// nothing textual can — so it reports the line and asks for a human
    /// classification rather than guessing one.
    #[test]
    fn this_client_resolves_a_side_question_in_exactly_one_place() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits: Vec<String> = Vec::new();
        let mut unclassifiable: Vec<String> = Vec::new();
        let mut files_scanned = 0usize;

        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the crate's own src is readable") {
                let path = entry.expect("a readable dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("a readable .rs file");
                files_scanned += 1;
                // Production code only: a test may name the resolver freely.
                let source = source.replace('\r', "");
                let production = source.split("#[cfg(test)]").next().unwrap_or_default();
                for line in production.lines() {
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    if line.contains("BtwTurn::resolve(") {
                        hits.push(format!("{}: {}", path.display(), line.trim()));
                    }
                }

                // Everything the prefix cut threw away. A needle in here is
                // invisible to the census above, which would still say "exactly
                // one" — see this test's doc for why the cut is hand-rolled at
                // all. Occurrences inside a string literal are skipped: this
                // scanner's own `line.find(..)` argument and the failure message
                // below both name the needle, and a scan that flags its own text
                // is a scan nobody can keep green. That check is per line and
                // resets at each newline, so a miscount costs one line rather
                // than the rest of the file.
                let discarded = &source[production.len()..];
                for line in discarded.lines() {
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    let Some(at) = line.find("BtwTurn::resolve(") else {
                        continue;
                    };
                    if inside_a_literal(line, at) {
                        continue;
                    }
                    unclassifiable.push(format!("{}: {}", path.display(), line.trim()));
                }
            }
        }

        assert!(
            unclassifiable.is_empty(),
            "a `BtwTurn::resolve(` call sits past this file's first \
             `#[cfg(test)]` marker, where the prefix cut above cannot see it. \
             This scan cannot tell whether it is production code or a test's \
             own call, and guessing is how a census goes quietly green. \
             Classify it by hand: if it is production, this client now resolves \
             a side question in two places and the census's `== 1` was passing \
             because it was blind.\n  {}",
            unclassifiable.join("\n  ")
        );
        assert!(files_scanned > 5, "the scan found nothing to scan");
        assert_eq!(
            hits.len(),
            1,
            "exactly one production call site, found: {hits:#?}"
        );
        assert!(
            hits[0].contains("commands.rs"),
            "the one resolver call belongs at the send chokepoint, found: {}",
            hits[0]
        );
    }

    /// Hermetic flow tests for [`execute_browser_with_launcher`].
    ///
    /// Nothing here touches the real world: the Gateway is a loopback-only,
    /// in-process fake (hand-rolled RFC 6455, `tokio::net` only) that answers
    /// the three RPCs the flow uses; `AppState::new` is pure in-memory; the
    /// launcher is the real `ViewerLauncher` over a recording `CommandRunner`
    /// (Windows argv, never executed), so no `StdCommandRunner`, `rundll32` or
    /// browser is ever started. The launch URL lives only in memory; assertions
    /// on secret-bearing values use `assert!` on booleans so a failure cannot
    /// print the nonce. The fake is NOT the production Gateway.
    mod launch_flow {
        use super::*;
        use crate::viewer_launcher::{
            CommandExit, CommandRunner, LaunchCommand, LaunchError, Platform, ViewerLauncher,
            ViewerLauncherConfig,
        };
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        const FAR_FUTURE_MS: i64 = 4_102_444_800_000; // 2100-01-01
        const ISSUE: &str = "browser.viewer.grant.issue";
        const REVOKE: &str = "browser.viewer.grant.revoke";

        // ---- minimal crypto for the WebSocket accept key (test-only) ----

        fn sha1(data: &[u8]) -> [u8; 20] {
            let mut h: [u32; 5] = [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ];
            let mut msg = data.to_vec();
            let bit_len = (data.len() as u64) * 8;
            msg.push(0x80);
            while msg.len() % 64 != 56 {
                msg.push(0);
            }
            msg.extend_from_slice(&bit_len.to_be_bytes());
            for chunk in msg.chunks(64) {
                let mut w = [0u32; 80];
                for (i, word) in w.iter_mut().enumerate().take(16) {
                    *word = u32::from_be_bytes([
                        chunk[4 * i],
                        chunk[4 * i + 1],
                        chunk[4 * i + 2],
                        chunk[4 * i + 3],
                    ]);
                }
                for i in 16..80 {
                    w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
                }
                let [mut a, mut b, mut c, mut d, mut e] = h;
                for (i, wi) in w.iter().enumerate() {
                    let (f, k) = match i {
                        0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
                        20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                        40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                        _ => (b ^ c ^ d, 0xCA62_C1D6),
                    };
                    let t = a
                        .rotate_left(5)
                        .wrapping_add(f)
                        .wrapping_add(e)
                        .wrapping_add(k)
                        .wrapping_add(*wi);
                    e = d;
                    d = c;
                    c = b.rotate_left(30);
                    b = a;
                    a = t;
                }
                for (slot, v) in h.iter_mut().zip([a, b, c, d, e]) {
                    *slot = slot.wrapping_add(v);
                }
            }
            let mut out = [0u8; 20];
            for (i, word) in h.iter().enumerate() {
                out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
            }
            out
        }

        fn base64_std(data: &[u8]) -> String {
            const T: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for chunk in data.chunks(3) {
                let n = (u32::from(chunk[0]) << 16)
                    | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                    | u32::from(*chunk.get(2).unwrap_or(&0));
                out.push(T[((n >> 18) & 63) as usize] as char);
                out.push(T[((n >> 12) & 63) as usize] as char);
                out.push(if chunk.len() > 1 {
                    T[((n >> 6) & 63) as usize] as char
                } else {
                    '='
                });
                out.push(if chunk.len() > 2 {
                    T[(n & 63) as usize] as char
                } else {
                    '='
                });
            }
            out
        }

        fn ws_accept(key: &str) -> String {
            base64_std(&sha1(
                format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
            ))
        }

        #[test]
        fn fake_gateway_accept_key_matches_rfc6455_sample() {
            assert_eq!(
                ws_accept("dGhlIHNhbXBsZSBub25jZQ=="),
                "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
            );
        }

        // ---- fake gateway ----

        #[derive(Clone, Debug, PartialEq)]
        enum Event {
            Rpc { method: String, params: Value },
            Launch,
        }
        type Log = Arc<Mutex<Vec<Event>>>;

        fn push(log: &Log, event: Event) {
            log.lock().unwrap().push(event);
        }

        fn rpcs(log: &Log, method: &str) -> Vec<Value> {
            log.lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    Event::Rpc { method: m, params } if m == method => Some(params.clone()),
                    _ => None,
                })
                .collect()
        }

        fn launches(log: &Log) -> usize {
            log.lock()
                .unwrap()
                .iter()
                .filter(|e| matches!(e, Event::Launch))
                .count()
        }

        fn rpc_methods(log: &Log) -> Vec<String> {
            log.lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    Event::Rpc { method, .. } => Some(method.clone()),
                    Event::Launch => None,
                })
                .collect()
        }

        #[derive(Clone)]
        struct Script {
            /// `Some((grant_id, launch_url))` = issue succeeds; `None` = denied.
            issue: Option<(String, String)>,
            revoke_ok: bool,
        }

        impl Script {
            fn reply(&self, method: &str) -> Result<Value, &'static str> {
                match method {
                    "connect" => Ok(json!({ "role": "operator" })),
                    ISSUE => match &self.issue {
                        Some((grant_id, url)) => Ok(json!({
                            "launch_url": url,
                            "grant_id": grant_id,
                            "expires_at_ms": FAR_FUTURE_MS,
                        })),
                        None => Err("denied"),
                    },
                    REVOKE if self.revoke_ok => Ok(json!({ "revoked": true })),
                    REVOKE => Err("revoke unavailable"),
                    _ => Err("unexpected method"),
                }
            }
        }

        async fn read_frame(s: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
            let mut h = [0u8; 2];
            s.read_exact(&mut h).await.ok()?;
            let opcode = h[0] & 0x0f;
            let masked = h[1] & 0x80 != 0;
            let mut len = u64::from(h[1] & 0x7f);
            if len == 126 {
                let mut b = [0u8; 2];
                s.read_exact(&mut b).await.ok()?;
                len = u64::from(u16::from_be_bytes(b));
            } else if len == 127 {
                let mut b = [0u8; 8];
                s.read_exact(&mut b).await.ok()?;
                len = u64::from_be_bytes(b);
            }
            let mut mask = [0u8; 4];
            if masked {
                s.read_exact(&mut mask).await.ok()?;
            }
            let mut payload = vec![0u8; usize::try_from(len).ok()?];
            s.read_exact(&mut payload).await.ok()?;
            if masked {
                for (i, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[i % 4];
                }
            }
            Some((opcode, payload))
        }

        async fn write_frame(s: &mut TcpStream, opcode: u8, payload: &[u8]) -> bool {
            let mut frame = vec![0x80 | opcode];
            if payload.len() < 126 {
                frame.push(payload.len() as u8);
            } else {
                frame.push(126);
                frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            }
            frame.extend_from_slice(payload);
            s.write_all(&frame).await.is_ok()
        }

        async fn run_gateway(listener: TcpListener, script: Script, log: Log) {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                if buf.len() > 16 * 1024 || s.read_exact(&mut byte).await.is_err() {
                    return;
                }
                buf.push(byte[0]);
            }
            let request = String::from_utf8_lossy(&buf).into_owned();
            let key = request.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("sec-websocket-key")
                    .then(|| value.trim().to_string())
            });
            let Some(key) = key else { return };
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                 Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                ws_accept(&key)
            );
            if s.write_all(response.as_bytes()).await.is_err() {
                return;
            }
            while let Some((opcode, payload)) = read_frame(&mut s).await {
                match opcode {
                    8 => {
                        write_frame(&mut s, 8, &[]).await;
                        return;
                    }
                    9 => {
                        write_frame(&mut s, 0xA, &payload).await;
                    }
                    1 => {
                        let Ok(request) = serde_json::from_slice::<Value>(&payload) else {
                            continue;
                        };
                        let method = request
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let params = request.get("params").cloned().unwrap_or(Value::Null);
                        let id = request.get("id").cloned().unwrap_or(Value::Null);
                        push(
                            &log,
                            Event::Rpc {
                                method: method.clone(),
                                params,
                            },
                        );
                        let reply = match script.reply(&method) {
                            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                            Err(message) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32000, "message": message },
                            }),
                        };
                        if !write_frame(&mut s, 1, reply.to_string().as_bytes()).await {
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }

        /// Keeps the client's event receiver and the gateway task alive.
        struct Harness {
            client: AlephClient,
            log: Log,
            _events: tokio::sync::mpsc::Receiver<aleph_protocol::StreamEvent>,
            server: tokio::task::JoinHandle<()>,
        }

        impl Drop for Harness {
            fn drop(&mut self) {
                self.server.abort();
            }
        }

        async fn harness(script: Script) -> Harness {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let log: Log = Arc::default();
            let server = tokio::spawn(run_gateway(listener, script, log.clone()));
            let config = aleph_client::CliConfig {
                device_name: "launch-flow-test".to_string(),
                ..aleph_client::CliConfig::default()
            };
            let (client, events) = AlephClient::connect(&url, &config).await.unwrap();
            Harness {
                client,
                log,
                _events: events,
                server,
            }
        }

        // ---- fixtures ----

        const GRANT_ID: &str = "grant_flow_1";

        fn nonce() -> String {
            let nonce = format!("SECRETNONCE{}", "x".repeat(32));
            assert_eq!(nonce.len(), 43);
            nonce
        }

        fn launch_url() -> String {
            format!(
                "https://viewer.example.test/v#grant_id={GRANT_ID}&nonce={}",
                nonce()
            )
        }

        fn granting() -> Script {
            Script {
                issue: Some((GRANT_ID.to_string(), launch_url())),
                revoke_ok: true,
            }
        }

        fn state(enabled: bool, ids: &[(&str, i64)]) -> AppState {
            let mut state = AppState::new("agent:main:main".to_string(), "test-model".to_string());
            state.browser_resources = catalog_with(ids);
            state.viewer_launcher_config = ViewerLauncherConfig {
                enabled,
                ..ViewerLauncherConfig::default()
            };
            state
        }

        fn live_state() -> AppState {
            state(
                true,
                &[("res_live_1", FAR_FUTURE_MS), ("res_live_2", FAR_FUTURE_MS)],
            )
        }

        /// Records every command instead of running it.
        #[derive(Clone, Default)]
        struct RecordingRunner {
            commands: Arc<Mutex<Vec<LaunchCommand>>>,
            exit: Option<CommandExit>,
        }

        impl CommandRunner for RecordingRunner {
            fn run(&self, command: &LaunchCommand) -> std::io::Result<CommandExit> {
                self.commands.lock().unwrap().push(command.clone());
                Ok(self.exit.unwrap_or(CommandExit::Exited(Some(0))))
            }
        }

        fn notices(state: &AppState) -> Vec<String> {
            state
                .messages
                .iter()
                .filter_map(|m| match m {
                    app::TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        }

        fn assert_no_secret(state: &AppState) {
            let everything = format!("{:?}{:?}", state.messages, notices(state));
            assert!(
                !everything.contains("SECRETNONCE"),
                "nonce leaked into UI state"
            );
        }

        async fn bounded<F: std::future::Future>(fut: F) -> F::Output {
            tokio::time::timeout(std::time::Duration::from_secs(20), fut)
                .await
                .expect("flow test timed out")
        }

        // ---- tests ----

        #[tokio::test]
        async fn launches_only_after_real_issue_for_exact_id_and_never_revokes_on_success() {
            let gw = harness(granting()).await;
            let mut state = live_state();
            let runner = RecordingRunner::default();
            let commands = runner.commands.clone();
            let log = gw.log.clone();
            let config_seen = Arc::new(Mutex::new(None));
            let config_seen_in = config_seen.clone();

            bounded(execute_browser_with_launcher(
                &mut state,
                &gw.client,
                "res_live_1",
                move |config, request| {
                    push(&log, Event::Launch);
                    *config_seen_in.lock().unwrap() = Some(config.clone());
                    ViewerLauncher::new(config, runner, Platform::Windows).launch(&request)
                },
            ))
            .await;

            // Ordered log: handshake, exactly one issue, then the launch, nothing else.
            let log = gw.log.lock().unwrap().clone();
            assert_eq!(
                log.len(),
                3,
                "unexpected events: {:?}",
                rpc_methods(&gw.log)
            );
            assert!(matches!(&log[0], Event::Rpc { method, .. } if method == "connect"));
            assert_eq!(
                log[1],
                Event::Rpc {
                    method: ISSUE.to_string(),
                    params: json!({ "resource_id": "res_live_1", "permissions": ["frames"] }),
                }
            );
            assert_eq!(log[2], Event::Launch);
            assert!(rpcs(&gw.log, REVOKE).is_empty(), "success must not revoke");

            // The launcher got the state's config and the URL the Gateway issued.
            assert!(config_seen.lock().unwrap().as_ref().unwrap().enabled);
            let commands = commands.lock().unwrap();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].program, "rundll32.exe");
            assert!(
                commands[0].args[1] == launch_url(),
                "URL not passed through intact"
            );

            assert_eq!(
                notices(&state),
                vec!["Browser viewer launched.".to_string()]
            );
            assert_no_secret(&state);
        }

        #[tokio::test]
        async fn launcher_error_revokes_the_exact_grant_once_and_keeps_failure_text() {
            let gw = harness(granting()).await;
            let mut state = live_state();
            let log = gw.log.clone();

            bounded(execute_browser_with_launcher(
                &mut state,
                &gw.client,
                "res_live_2",
                move |_config, _request| {
                    push(&log, Event::Launch);
                    Err(LaunchError::Spawn(std::io::ErrorKind::NotFound))
                },
            ))
            .await;

            assert_eq!(
                rpc_methods(&gw.log),
                vec!["connect", ISSUE, REVOKE],
                "issue -> revoke, once each"
            );
            assert_eq!(launches(&gw.log), 1);
            assert_eq!(
                rpcs(&gw.log, ISSUE),
                vec![json!({ "resource_id": "res_live_2", "permissions": ["frames"] })]
            );
            assert_eq!(rpcs(&gw.log, REVOKE), vec![json!({ "grant_id": GRANT_ID })]);
            assert_eq!(
                notices(&state),
                vec![format_browser_launch_error(&LaunchError::Spawn(
                    std::io::ErrorKind::NotFound
                ))]
            );
            assert_no_secret(&state);
        }

        #[tokio::test]
        async fn real_launcher_rejecting_the_issued_url_is_cleaned_up_without_leaking() {
            // Issuer returns a URL the real launcher refuses (http, not https).
            let script = Script {
                issue: Some((
                    GRANT_ID.to_string(),
                    format!(
                        "http://viewer.example.test/v#grant_id={GRANT_ID}&nonce={}",
                        nonce()
                    ),
                )),
                revoke_ok: true,
            };
            let gw = harness(script).await;
            let mut state = live_state();
            let runner = RecordingRunner::default();
            let commands = runner.commands.clone();

            bounded(execute_browser_with_launcher(
                &mut state,
                &gw.client,
                "res_live_1",
                move |config, request| {
                    ViewerLauncher::new(config, runner, Platform::Windows).launch(&request)
                },
            ))
            .await;

            assert!(commands.lock().unwrap().is_empty(), "no process may start");
            assert_eq!(rpcs(&gw.log, REVOKE), vec![json!({ "grant_id": GRANT_ID })]);
            assert!(notices(&state)[0].starts_with("Browser viewer launch failed:"));
            assert_no_secret(&state);
        }

        #[tokio::test]
        async fn launcher_panic_is_reported_as_task_failure_and_revoked_once() {
            let gw = harness(granting()).await;
            let mut state = live_state();

            bounded(execute_browser_with_launcher(
                &mut state,
                &gw.client,
                "res_live_1",
                |_config, _request| -> Result<(), LaunchError> {
                    panic!("injected launcher panic")
                },
            ))
            .await;

            assert_eq!(rpcs(&gw.log, REVOKE), vec![json!({ "grant_id": GRANT_ID })]);
            assert_eq!(
                notices(&state),
                vec!["Browser viewer launch task failed.".to_string()]
            );
            assert_no_secret(&state);
        }

        #[tokio::test]
        async fn failed_cleanup_appends_ttl_notice_after_one_revoke_attempt() {
            let mut script = granting();
            script.revoke_ok = false;
            let gw = harness(script).await;
            let mut state = live_state();

            bounded(execute_browser_with_launcher(
                &mut state,
                &gw.client,
                "res_live_1",
                |_config, _request| Err(LaunchError::NonZeroExit(Some(1))),
            ))
            .await;

            assert_eq!(rpcs(&gw.log, REVOKE).len(), 1, "no retry loop");
            let message = notices(&state).remove(0);
            assert!(message.starts_with("Browser viewer launch failed:"));
            assert!(message.ends_with(BROWSER_GRANT_CLEANUP_FAILED));
            assert_no_secret(&state);
        }

        #[tokio::test]
        async fn denied_issue_never_reaches_the_launcher_or_revoke() {
            let gw = harness(Script {
                issue: None,
                revoke_ok: true,
            })
            .await;
            let mut state = live_state();
            let log = gw.log.clone();

            bounded(execute_browser_with_launcher(
                &mut state,
                &gw.client,
                "res_live_1",
                move |_config, _request| {
                    push(&log, Event::Launch);
                    Ok(())
                },
            ))
            .await;

            assert_eq!(rpc_methods(&gw.log), vec!["connect", ISSUE]);
            assert_eq!(launches(&gw.log), 0);
            assert_eq!(notices(&state), vec![BROWSER_REQUEST_FAILED.to_string()]);
        }

        #[tokio::test]
        async fn preflight_refusals_issue_nothing_and_never_launch() {
            // (viewer enabled, catalog, argument)
            let cases = vec![
                (false, vec![("res_live_1", FAR_FUTURE_MS)], "res_live_1"), // disabled
                (true, vec![("res_live_1", FAR_FUTURE_MS)], "res_unknown"), // not in catalog
                (true, vec![("res_live_1", FAR_FUTURE_MS)], "res_live"),    // prefix of an id
                (true, vec![("res_old", 1)], "res_old"),                    // expired
                (true, vec![("res_live_1", FAR_FUTURE_MS)], ""),            // blank
                (true, vec![("res_live_1", FAR_FUTURE_MS)], "  \t"),        // whitespace
            ];
            for (enabled, ids, arg) in cases {
                let gw = harness(granting()).await;
                let mut state = state(enabled, &ids);
                let log = gw.log.clone();
                bounded(execute_browser_with_launcher(
                    &mut state,
                    &gw.client,
                    arg,
                    move |_config, _request| {
                        push(&log, Event::Launch);
                        Ok(())
                    },
                ))
                .await;
                assert_eq!(
                    rpc_methods(&gw.log),
                    vec!["connect"],
                    "case enabled={enabled} arg={arg:?} reached the Gateway"
                );
                assert_eq!(
                    launches(&gw.log),
                    0,
                    "case arg={arg:?} reached the launcher"
                );
                assert_eq!(notices(&state).len(), 1, "case arg={arg:?}");
            }
        }
    }
    /// Real-Gateway acceptance of the `/browser` command path
    /// (`execute_browser_with_launcher`). Opt-in only: the test is
    /// `#[ignore]`d and additionally refuses to run unless the caller (see
    /// `qa/browser_live/tui-real-run.mjs`) has confirmed that the exact
    /// loopback Gateway is an owned process.
    ///
    /// What is REAL: the WebSocket, the device-token handshake
    /// (`AlephClient::connect_with_credential`), `events.subscribe`,
    /// `agent.run`, the Gateway-published `browser.live.resource` event
    /// (via `take_topic_events`), `browser.viewer.resource.decide`, grant
    /// issue / revoke, and the `browser_tabs` tool.
    /// What is INSTRUMENTED: the OS launcher. The callback builds a real
    /// `ViewerLauncher` (URL validation, trusted-origin check, Windows argv)
    /// but its `CommandRunner` only records the command; nothing is spawned.
    /// What is NOT the production TUI: the credential comes from an owned file
    /// (never the keyring / `--pair` / `--paired` store) and `AppState` is an
    /// in-memory test instance.
    mod real_gateway {
        use super::*;
        use crate::viewer_launcher::{
            CommandExit, CommandRunner, LaunchCommand, LaunchError, Platform, ViewerLauncher,
            ViewerLauncherConfig,
        };
        use aleph_client::DeviceCredential;
        use aleph_protocol::browser_live::{
            BrowserTargetBinding, RedactionStatus, ResourceBinding, RESOURCE_TOPIC,
        };
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        const GATEWAY_WS: &str = "ws://127.0.0.1:18790/ws";
        const TRUSTED_ORIGIN: &str = "http://127.0.0.1:18790";
        const ENV_AUTH_FILE: &str = "ALEPH_TUI_REAL_AUTH_FILE"; // PATH only, never the secret
        const ENV_GATEWAY: &str = "ALEPH_TUI_REAL_GATEWAY_WS";
        const ENV_FIXTURE: &str = "ALEPH_TUI_REAL_FIXTURE_URL";
        const ENV_OWNED: &str = "ALEPH_TUI_REAL_GATEWAY_OWNED"; // must be "1"
        const DEFAULT_FIXTURE: &str = "http://127.0.0.1:18791/page";
        const RESOURCE_WAIT: Duration = Duration::from_secs(90);
        const RPC_WAIT: Duration = Duration::from_secs(30);
        const SCENARIO_BUDGET: Duration = Duration::from_secs(120);
        const CLEANUP_BUDGET: Duration = Duration::from_secs(30);

        /// Records every command instead of running it; optionally refuses.
        #[derive(Clone, Default)]
        struct OwnedRecordingRunner {
            commands: Arc<Mutex<Vec<LaunchCommand>>>,
            refuse: bool,
        }

        impl CommandRunner for OwnedRecordingRunner {
            fn run(&self, command: &LaunchCommand) -> std::io::Result<CommandExit> {
                self.commands.lock().unwrap().push(command.clone());
                if self.refuse {
                    Err(std::io::Error::from(std::io::ErrorKind::NotFound))
                } else {
                    Ok(CommandExit::Exited(Some(0)))
                }
            }
        }

        fn system_notices(state: &AppState) -> Vec<String> {
            state
                .messages
                .iter()
                .filter_map(|m| match m {
                    app::TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        }

        /// `grant_id` out of the launch URL's fragment. The URL itself is
        /// secret-bearing and is never returned, logged or compared in an
        /// assertion that would print it.
        fn grant_id_of(url: &str) -> Option<String> {
            let fragment = url.split_once('#')?.1;
            fragment
                .split('&')
                .find_map(|kv| kv.strip_prefix("grant_id="))
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        }

        fn has_nonce(url: &str) -> bool {
            url.split_once('#')
                .map(|(_, f)| {
                    f.split('&')
                        .any(|kv| kv.strip_prefix("nonce=").is_some_and(|v| !v.is_empty()))
                })
                .unwrap_or(false)
        }

        fn find_tabs(v: &Value, depth: u8) -> Option<Vec<Value>> {
            if depth > 6 {
                return None;
            }
            match v {
                Value::String(s) => serde_json::from_str::<Value>(s)
                    .ok()
                    .and_then(|inner| find_tabs(&inner, depth + 1)),
                Value::Object(map) => {
                    if let Some(Value::Array(tabs)) = map.get("tabs") {
                        return Some(tabs.clone());
                    }
                    map.values().find_map(|c| find_tabs(c, depth + 1))
                }
                _ => None,
            }
        }

        fn find_success(v: &Value, depth: u8) -> Option<bool> {
            if depth > 6 {
                return None;
            }
            match v {
                Value::String(s) => serde_json::from_str::<Value>(s)
                    .ok()
                    .and_then(|inner| find_success(&inner, depth + 1)),
                Value::Object(map) => {
                    if let Some(Value::Bool(b)) = map.get("success") {
                        return Some(*b);
                    }
                    map.values().find_map(|c| find_success(c, depth + 1))
                }
                _ => None,
            }
        }

        fn check(cond: bool, what: &str) -> Result<(), String> {
            if cond {
                Ok(())
            } else {
                Err(format!("check failed: {what}"))
            }
        }

        fn clip(s: &str) -> String {
            s.chars().take(160).collect()
        }

        async fn rpc(client: &AlephClient, method: &str, params: Value) -> Result<Value, String> {
            tokio::time::timeout(RPC_WAIT, client.call::<_, Value>(method, Some(params)))
                .await
                .map_err(|_| format!("{method}: timed out"))?
                .map_err(|e| format!("{method}: {}", clip(&e.to_string())))
        }

        /// Everything the report may say: booleans, counts and fixed text only.
        #[derive(Default)]
        struct Report(serde_json::Map<String, Value>);
        impl Report {
            fn set(&mut self, key: &str, v: impl Into<Value>) {
                self.0.insert(key.to_string(), v.into());
            }
        }

        type TargetSlot = Arc<Mutex<Option<BrowserTargetBinding>>>;

        async fn scenario(
            client: &AlephClient,
            topics: &mut tokio::sync::mpsc::Receiver<aleph_client::TopicEvent>,
            state: &mut AppState,
            fixture: &str,
            target_slot: &TargetSlot,
            report: &mut Report,
        ) -> Result<(), String> {
            // ---- subscribe exactly like the TUI does, then publish a fresh run ----
            crate::tui::subscribe_browser_live_topics(client)
                .await
                .map_err(|e| format!("events.subscribe: {}", clip(&e.to_string())))?;

            let session_key = format!(
                "agent:main:tui-real-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            );
            let run = rpc(
                client,
                "agent.run",
                json!({
                    "input": format!("OPEN_FIXTURE:{fixture}"),
                    "session_key": session_key,
                    "stream": true,
                }),
            )
            .await?;
            let run_id = run
                .get("run_id")
                .and_then(Value::as_str)
                .ok_or("agent.run: no run_id")?
                .to_string();

            // ---- the real Gateway-published resource event for THIS run ----
            let deadline = tokio::time::Instant::now() + RESOURCE_WAIT;
            let mut foreign_events = 0usize;
            let published = loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let ev = tokio::time::timeout(remaining, topics.recv())
                    .await
                    .map_err(|_| "no browser.live.resource for this run in time".to_string())?
                    .ok_or("topic plane closed")?;
                if ev.topic != RESOURCE_TOPIC {
                    continue;
                }
                let mine = ev
                    .data
                    .pointer("/resource/binding/viewer/task/run_id")
                    .and_then(Value::as_str)
                    == Some(run_id.as_str());
                if mine {
                    break ev;
                }
                foreign_events += 1;
            };
            report.set("foreign_resource_events_ignored", foreign_events);

            // ---- through the TUI's own topic path into the catalog ----
            let resource_id = published
                .data
                .pointer("/resource/resource_id")
                .and_then(Value::as_str)
                .ok_or("event has no resource_id")?
                .to_string();
            let first_revision = published
                .data
                .pointer("/resource/revision")
                .and_then(Value::as_u64)
                .ok_or("event has no revision")?;
            state.handle_topic_event(&published.topic, published.data.clone());
            let stored = state
                .browser_resources
                .get(&resource_id)
                .ok_or("catalog did not store the real resource event")?;
            check(
                stored.revision == first_revision,
                "stored revision matches event",
            )?;
            let (target, bound_run, bound_session) = match &stored.binding {
                ResourceBinding::Browser { viewer } => (
                    viewer.target.clone(),
                    viewer.task.run_id.clone(),
                    viewer.task.session_key.clone(),
                ),
                _ => return Err("resource binding is not a browser binding".into()),
            };
            report.set(
                "initial_redaction_pending",
                stored.redaction == RedactionStatus::Pending,
            );
            // From here the bound tab must be closed no matter what happens.
            *target_slot.lock().unwrap() = Some(target);
            check(bound_run == run_id, "binding.run_id is this run")?;
            check(
                bound_session == session_key,
                "binding.session_key is this run",
            )?;
            report.set("initial_revision", first_revision);

            // ---- decide(safe) with the REAL revision; feed the REAL response back ----
            let decided = rpc(
                client,
                "browser.viewer.resource.decide",
                json!({
                    "resource_id": resource_id,
                    "expected_revision": first_revision,
                    "decision": "safe",
                }),
            )
            .await?;
            state.handle_topic_event(RESOURCE_TOPIC, decided);
            let after = state
                .browser_resources
                .get(&resource_id)
                .ok_or("resource vanished after decide")?;
            check(
                after.revision > first_revision,
                "decide produced a newer revision",
            )?;
            check(
                after.redaction == RedactionStatus::Safe,
                "catalog holds the safe revision",
            )?;
            report.set("decided_revision_advanced", true);
            report.set("decided_redaction_safe", true);
            let now_ms = i64::try_from(state.now_ms()).unwrap_or(i64::MAX);
            check(
                state
                    .browser_resources
                    .get_unexpired(&resource_id, now_ms)
                    .is_some(),
                "resource unexpired at launch time",
            )?;

            // ---- launcher config set directly on the state (no env mutation) ----
            state.viewer_launcher_config = ViewerLauncherConfig {
                enabled: true,
                trusted_origins: vec![TRUSTED_ORIGIN.to_string()],
                ..ViewerLauncherConfig::default()
            };

            // ================= success scenario =================
            let ok_runner = OwnedRecordingRunner::default();
            let ok_commands = ok_runner.commands.clone();
            let notices_before = system_notices(state).len();
            execute_browser_with_launcher(state, client, &resource_id, move |config, request| {
                ViewerLauncher::new(config, ok_runner, Platform::Windows).launch(&request)
            })
            .await;
            let notices_now = system_notices(state);
            check(
                notices_now.len() == notices_before + 1,
                "success: exactly one notice",
            )?;
            check(
                notices_now.last().map(String::as_str) == Some("Browser viewer launched."),
                "success notice is the launched text",
            )?;
            let ok_cmds = ok_commands.lock().unwrap().clone();
            check(
                ok_cmds.len() == 1,
                "success: exactly one instrumented launch",
            )?;
            check(
                ok_cmds[0].program == "rundll32.exe",
                "success: Windows default argv program",
            )?;
            check(ok_cmds[0].args.len() == 2, "success: argv shape")?;
            let ok_url = ok_cmds[0].args[1].clone();
            check(
                ok_url.starts_with(&format!("{TRUSTED_ORIGIN}/")),
                "success: URL on the trusted loopback origin",
            )?;
            check(has_nonce(&ok_url), "success: URL carries a nonce")?;
            let ok_grant = grant_id_of(&ok_url).ok_or("success: no grant_id in URL")?;
            let flat = format!("{:?}{:?}", state.messages, system_notices(state));
            check(
                !flat.contains(&ok_grant),
                "success: grant id not shown in UI state",
            )?;
            report.set("success_launch_count", 1);
            report.set("success_notice_launched", true);
            report.set("success_url_on_trusted_origin", true);

            // Dispose the unused success grant through the real revoke RPC.
            let disposed = rpc(
                client,
                "browser.viewer.grant.revoke",
                json!({ "grant_id": ok_grant }),
            )
            .await?;
            check(
                disposed.get("revoked").and_then(Value::as_bool) == Some(true),
                "success grant was still live (revoked:true on dispose, i.e. helper did not revoke it)",
            )?;
            report.set("success_grant_live_until_dispose", true);

            // ================= failure scenario =================
            let bad_runner = OwnedRecordingRunner {
                refuse: true,
                ..OwnedRecordingRunner::default()
            };
            let bad_commands = bad_runner.commands.clone();
            let before = system_notices(state).len();
            execute_browser_with_launcher(state, client, &resource_id, move |config, request| {
                // Real launcher; its runner refuses -> LaunchError::Spawn.
                let result =
                    ViewerLauncher::new(config, bad_runner, Platform::Windows).launch(&request);
                match &result {
                    Err(LaunchError::Spawn(_)) => result,
                    _ => Err(LaunchError::Spawn(std::io::ErrorKind::Other)),
                }
            })
            .await;
            let after_notices = system_notices(state);
            check(
                after_notices.len() == before + 1,
                "failure: exactly one notice",
            )?;
            let failure_text = after_notices.last().cloned().unwrap_or_default();
            check(
                failure_text.starts_with("Browser viewer launch failed:"),
                "failure: launch-failed notice",
            )?;
            check(
                !failure_text.contains(BROWSER_GRANT_CLEANUP_FAILED.trim()),
                "failure: helper's revoke succeeded (no cleanup-failed suffix)",
            )?;
            let bad_cmds = bad_commands.lock().unwrap().clone();
            check(
                bad_cmds.len() == 1,
                "failure: exactly one instrumented launch",
            )?;
            let bad_url = bad_cmds[0].args.get(1).cloned().unwrap_or_default();
            let bad_grant = grant_id_of(&bad_url).ok_or("failure: no grant_id in URL")?;
            check(bad_grant != ok_grant, "failure: a FRESH grant was issued")?;
            let flat = format!("{:?}{:?}", state.messages, system_notices(state));
            check(
                !flat.contains(&bad_grant),
                "failure: grant id not shown in UI state",
            )?;
            report.set("failure_launch_count", 1);
            report.set("failure_notice_launch_failed", true);
            report.set("failure_notice_cleanup_suffix_absent", true);

            // The helper already revoked this exact grant. The Gateway's revoke is
            // owner-scoped and non-oracular: a second revoke of a consumed grant is
            // the generic PERMISSION_DENIED / "denied" (see handlers/browser_viewer.rs
            // `issuer_rpc_owned_issue_and_revoke_enforce_identity_without_oracle`),
            // never `revoked:false`. Accept ONLY that exact code + message.
            let probe = tokio::time::timeout(
                RPC_WAIT,
                client.call::<_, Value>(
                    "browser.viewer.grant.revoke",
                    Some(json!({ "grant_id": bad_grant })),
                ),
            )
            .await
            .map_err(|_| "failure probe revoke: timed out".to_string())?;
            let already_revoked_denied = matches!(
                &probe,
                Err(CliError::Rpc { code, message })
                    if *code == aleph_protocol::jsonrpc::PERMISSION_DENIED
                        && message == "denied"
            );
            check(
                already_revoked_denied,
                "failure grant already revoked by the helper (second revoke is exactly PERMISSION_DENIED/denied)",
            )?;
            report.set("failure_probe_second_revoke_permission_denied", true);

            // Fixed, client-side RPC accounting (not server-observed).
            report.set(
                "rpc_direct_calls",
                json!({
                    "events.subscribe": 1, "agent.run": 1, "decide": 1,
                    "grant.revoke(dispose success)": 1, "grant.revoke(probe failure, denied)": 1,
                }),
            );
            report.set(
                "rpc_via_helper",
                json!({ "grant.issue": 2, "grant.revoke(failure cleanup)": 1 }),
            );
            Ok(())
        }

        /// Close ONLY the tab bound to this run's resource.
        async fn close_own_tab(
            client: &AlephClient,
            target: &BrowserTargetBinding,
            report: &mut Report,
        ) -> Result<(), String> {
            let listed = rpc(
                client,
                "tools.invoke",
                json!({
                    "tool_name": "browser_tabs",
                    "arguments": { "profile": target.profile, "action": "list" },
                }),
            )
            .await?;
            let tabs = find_tabs(&listed, 0).ok_or("browser_tabs list: no tabs array")?;
            let exact = tabs
                .iter()
                .filter(|t| t.get("id").and_then(Value::as_str) == Some(target.target_id.as_str()))
                .count();
            report.set("tab_list_exact_matches", exact);
            if exact != 1 {
                return Err("bound tab not uniquely identified; nothing closed".into());
            }
            let closed = rpc(
                client,
                "tools.invoke",
                json!({
                    "tool_name": "browser_tabs",
                    "arguments": {
                        "profile": target.profile,
                        "action": { "close": { "tab_id": target.target_id } },
                    },
                }),
            )
            .await?;
            let ok = find_success(&closed, 0);
            report.set(
                "tab_close_success",
                ok.map(Value::from).unwrap_or(Value::Null),
            );
            check(ok != Some(false), "browser_tabs close reported failure")
        }

        #[tokio::test]
        #[ignore = "requires an owned real Gateway on 127.0.0.1:18790 + device-token file; run via qa/browser_live/tui-real-run.mjs"]
        async fn real_gateway_browser_command_path() {
            // ---- opt-in gates: nothing below runs against anything unconfirmed ----
            assert!(
                std::env::var(ENV_OWNED).as_deref() == Ok("1"),
                "owned-Gateway confirmation missing"
            );
            let gateway = std::env::var(ENV_GATEWAY).unwrap_or_else(|_| GATEWAY_WS.to_string());
            assert!(
                gateway == GATEWAY_WS,
                "Gateway URL is not the exact loopback URL"
            );
            let fixture =
                std::env::var(ENV_FIXTURE).unwrap_or_else(|_| DEFAULT_FIXTURE.to_string());
            assert!(
                fixture.starts_with("http://127.0.0.1:"),
                "fixture URL is not loopback"
            );
            let auth_path = std::env::var(ENV_AUTH_FILE).expect("auth file path env missing");

            // ---- credential: read in-process from the owned file, never printed ----
            let credential = {
                let raw = std::fs::read_to_string(&auth_path).expect("auth file unreadable");
                let doc: Value = serde_json::from_str(&raw).expect("auth file is not JSON");
                let id = doc
                    .get("device_id")
                    .and_then(Value::as_str)
                    .expect("device_id");
                let token = doc
                    .get("device_token")
                    .and_then(Value::as_str)
                    .expect("device_token");
                DeviceCredential::device_token(id, token).expect("credential rejected")
            };
            let config = aleph_client::CliConfig {
                device_name: "tui-real-gateway-acceptance".to_string(),
                ..aleph_client::CliConfig::default()
            };

            let mut report = Report::default();
            report.set(
                "label",
                "real Gateway and TUI command path, instrumented OS runner",
            );
            report.set("os_launch_proved", false);

            let (client, mut stream_events) = tokio::time::timeout(
                Duration::from_secs(20),
                AlephClient::connect_with_credential(&gateway, &config, credential),
            )
            .await
            .expect("connect timed out")
            .unwrap_or_else(|e| panic!("connect_with_credential failed: {}", clip(&e.to_string())));
            report.set("connected_with_device_token", true);
            let drain = tokio::spawn(async move { while stream_events.recv().await.is_some() {} });
            let mut topics = client
                .take_topic_events()
                .expect("topic plane already claimed");
            let mut state = AppState::new("agent:main:main".to_string(), "test-model".to_string());
            let target_slot: TargetSlot = Arc::default();

            let outcome = tokio::time::timeout(
                SCENARIO_BUDGET,
                scenario(
                    &client,
                    &mut topics,
                    &mut state,
                    &fixture,
                    &target_slot,
                    &mut report,
                ),
            )
            .await
            .unwrap_or_else(|_| Err("scenario exceeded its time budget".to_string()));

            // ---- cleanup: close only our own tab, whatever the outcome ----
            let target = target_slot.lock().unwrap().clone();
            let cleanup = match &target {
                Some(t) => {
                    tokio::time::timeout(CLEANUP_BUDGET, close_own_tab(&client, t, &mut report))
                        .await
                        .unwrap_or_else(|_| Err("tab cleanup timed out".to_string()))
                }
                None => Ok(()),
            };
            report.set("own_tab_closed", target.is_some() && cleanup.is_ok());
            drain.abort();

            report.set("scenario_ok", outcome.is_ok());
            report.set("cleanup_ok", cleanup.is_ok());
            if let Err(e) = &outcome {
                report.set("scenario_error", clip(e));
            }
            if let Err(e) = &cleanup {
                report.set("cleanup_error", clip(e));
            }
            println!("REAL_GATEWAY_REPORT {}", Value::Object(report.0));
            assert!(outcome.is_ok(), "scenario failed (see REAL_GATEWAY_REPORT)");
            assert!(cleanup.is_ok(), "cleanup failed (see REAL_GATEWAY_REPORT)");
        }
    }
}

#[cfg(test)]
mod active_run_tests {
    use super::{active_run_from_history, app};
    use serde_json::json;

    /// "The server never told me" and "the server told me nothing is running"
    /// are different answers, and only the second may arm the cross-session
    /// guard. Reading the first as the second would make an old gateway look
    /// like a quiet one, and this screen would silently drop every frame of
    /// every run it did not start itself.
    #[test]
    fn an_absent_field_is_not_an_answer() {
        assert_eq!(active_run_from_history(&json!({ "messages": [] })), None);
    }

    #[test]
    fn null_means_nothing_is_running_here() {
        assert_eq!(
            active_run_from_history(&json!({ "active_run": serde_json::Value::Null })),
            Some(None)
        );
    }

    #[test]
    fn a_run_id_is_the_turn_to_join() {
        assert_eq!(
            active_run_from_history(&json!({ "active_run": "run-7" })),
            Some(Some(app::ActiveRunJoin {
                run_id: "run-7".to_string(),
                elapsed_ms: None,
            }))
        );
    }

    /// The age rides along on the same response. A client that asked for it
    /// separately would be timing a different instant than the one that
    /// produced the run id beside it.
    #[test]
    fn the_reported_age_rides_with_the_run_id() {
        assert_eq!(
            active_run_from_history(
                &json!({ "active_run": "run-7", "active_run_elapsed_ms": 240_000 })
            ),
            Some(Some(app::ActiveRunJoin {
                run_id: "run-7".to_string(),
                elapsed_ms: Some(240_000),
            }))
        );
    }

    /// An age with no run is not a turn to join. The field pair is read from
    /// the run id outwards, so a server that reported one without the other
    /// cannot produce a run this screen would try to settle.
    #[test]
    fn an_age_without_a_run_is_still_nothing_running() {
        assert_eq!(
            active_run_from_history(
                &json!({ "active_run": serde_json::Value::Null, "active_run_elapsed_ms": 9 })
            ),
            Some(None)
        );
    }

    /// An empty string is not a run id. It reaches `adopt_active_run` as "we
    /// asked, nothing is running" rather than as a run nothing can ever settle.
    #[test]
    fn an_empty_string_is_not_a_run() {
        assert_eq!(
            active_run_from_history(&json!({ "active_run": "" })),
            Some(None)
        );
    }
}

#[cfg(test)]
mod attach_mode_tests {
    use super::{apply_acquired_session, apply_history, AttachMode, SessionAcquisition};
    use crate::tui::app::{AppState, TranscriptEntry};
    use serde_json::json;

    fn user_rows(state: &AppState) -> Vec<&str> {
        state
            .messages
            .iter()
            .filter_map(|m| match m {
                TranscriptEntry::UserText { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn history(rows: &[&str]) -> serde_json::Value {
        json!({
            "messages": rows
                .iter()
                .map(|c| json!({ "role": "user", "content": c }))
                .collect::<Vec<_>>(),
            "active_run": serde_json::Value::Null,
        })
    }

    #[test]
    fn acquired_completed_steps_replace_atomically_and_reject_stale_generation() {
        use super::{apply_acquired_session, SessionAcquisition};
        let key = "agent:main:main:s1";
        let mut state = AppState::new(key.into(), "m".into());
        let snapshot = |generation| SessionAcquisition {
            session_key: key.into(),
            generation,
            history: json!({"messages": [
                {"role":"user", "content":"question"},
                {"role":"assistant", "content":"answer", "run_id":"r"}
            ], "active_run":null}),
            traces: Some(json!({"runs":{"r":[
                {"kind":"turn_started", "iteration":1},
                {"kind":"text_emitted", "iteration":1, "stream":"final", "text":"thinking"}
            ]}})),
        };
        let stale = snapshot(state.transcript_generation);
        state.transcript_generation += 1;
        assert!(!apply_acquired_session(
            &mut state,
            stale,
            AttachMode::Replace
        ));
        assert!(state.messages.is_empty());
        let fresh = snapshot(state.transcript_generation);
        assert!(apply_acquired_session(
            &mut state,
            fresh,
            AttachMode::Replace
        ));
        assert!(state
            .messages
            .iter()
            .any(|m| matches!(m, TranscriptEntry::Step(_))));
    }

    /// Launch and `/session` prepare the list themselves — the first keeps the
    /// welcome banner and the startup notices, the second wipes it in
    /// `switch_session`. Both then append.
    #[test]
    fn an_appending_attach_keeps_what_the_caller_prepared() {
        let mut state = AppState::new("agent:main:main:s1".into(), "m".into());
        state.messages.push(TranscriptEntry::UserText {
            id: "t1".into(),
            text: "prepared by the caller".into(),
            at_ms: None,
            attachments: Vec::new(),
        });

        apply_history(
            &mut state,
            &history(&["from the server"]),
            AttachMode::Append,
        );

        assert_eq!(
            user_rows(&state),
            vec!["prepared by the caller", "from the server"]
        );
    }

    /// A reattach swaps the screen for the server's copy — messages, tool rows
    /// and whole turns can have landed while this client was offline, and only
    /// the server's copy is complete.
    #[test]
    fn a_replacing_attach_swaps_in_the_servers_copy() {
        let mut state = AppState::new("agent:main:main:s1".into(), "m".into());
        state.messages.push(TranscriptEntry::UserText {
            id: "t1".into(),
            text: "stale, from before the drop".into(),
            at_ms: None,
            attachments: Vec::new(),
        });

        apply_history(
            &mut state,
            &history(&["what really happened"]),
            AttachMode::Replace,
        );

        assert_eq!(user_rows(&state), vec!["what really happened"]);
    }

    /// The swap happens HERE, which is inside `attach_session`'s `Ok` arm. That
    /// placement is the property: a reattach whose `chat.history` failed never
    /// reaches this function, so it cannot leave the user on a blank screen at
    /// the exact moment the connection came back.
    #[test]
    fn a_late_snapshot_cannot_replace_the_new_session() {
        let mut state = AppState::new("new".into(), "m".into());
        state.add_user_message("keep".into(), vec![]);
        let acquired = SessionAcquisition {
            session_key: "old".into(),
            generation: state.transcript_generation,
            history: json!({"messages": []}),
            traces: None,
        };
        assert!(!apply_acquired_session(
            &mut state,
            acquired,
            AttachMode::Replace
        ));
        assert!(state.messages.iter().any(
            |entry| matches!(entry, TranscriptEntry::UserText { text, .. } if text == "keep")
        ));
    }
}

#[cfg(test)]
mod side_run_verdict_tests {
    use super::{side_run_verdict, SideRunVerdict};
    use aleph_client::CliError;
    use aleph_protocol::AgentRunStatusReport;

    /// Build the report the way the server does — from the contract type, not
    /// from a JSON literal written here. A literal would only prove serde
    /// round-trips its own bytes.
    fn reported(status: &str, error: Option<&str>) -> Result<AgentRunStatusReport, CliError> {
        Ok(AgentRunStatusReport {
            run_id: "r-side".into(),
            session_key: "agent:main:main:s1".into(),
            status: status.into(),
            elapsed_ms: 4_200,
            error: error.map(str::to_string),
        })
    }

    /// A side question that outlives a blip must not be torn down by the
    /// repair. Its frames resume on the new socket, so settling it here would
    /// throw away an answer that is still on its way.
    #[test]
    fn a_side_question_still_in_flight_is_left_alone() {
        assert_eq!(
            side_run_verdict(&reported(AgentRunStatusReport::RUNNING, None)),
            SideRunVerdict::StillAnswering
        );
    }

    /// The defect this whole path exists to close: the run ended while the
    /// client was away, its terminal frame went to a dead socket, and the
    /// overlay spun forever.
    ///
    /// It settles as `Disconnected`, never as answered — the frames emitted
    /// during the outage are gone, so the text on file may be a prefix of the
    /// real answer with no way to tell.
    #[test]
    fn a_run_that_finished_during_the_outage_settles_without_claiming_the_answer() {
        let SideRunVerdict::Disconnected(note) =
            side_run_verdict(&reported(AgentRunStatusReport::COMPLETED, None))
        else {
            panic!("a finished run must stop the spinner");
        };
        assert!(
            note.contains("only what reached this client"),
            "the user must be told the text may be partial: {note}"
        );
    }

    /// A failure keeps the reason. The wire used to drop it, so a client told
    /// `failed` had nothing to show but the word.
    #[test]
    fn a_failure_reaches_the_overlay_with_its_reason() {
        assert_eq!(
            side_run_verdict(&reported(
                AgentRunStatusReport::FAILED,
                Some("provider 429")
            )),
            SideRunVerdict::Failed("provider 429".into())
        );
    }

    /// …and a gateway that names the failure without saying why gets a sentence
    /// that claims no more than it was told. Not "unknown error", which reads
    /// as a fact about the run rather than about what was said.
    #[test]
    fn a_reasonless_failure_does_not_invent_one() {
        for blank in [None, Some(""), Some("   ")] {
            let SideRunVerdict::Failed(reason) =
                side_run_verdict(&reported(AgentRunStatusReport::FAILED, blank))
            else {
                panic!("a failed run must settle as failed");
            };
            assert_eq!(reason, "the gateway reported this run failed");
        }
    }

    /// A cancel is not a failure and not an answer.
    #[test]
    fn a_cancelled_run_is_neither_failed_nor_answered() {
        let verdict = side_run_verdict(&reported(AgentRunStatusReport::CANCELLED, None));
        assert!(matches!(verdict, SideRunVerdict::Disconnected(ref n) if n.contains("cancelled")));
    }

    /// An older client against a newer gateway. Reading an unknown word as
    /// "still going" is the one unrecoverable direction — the spinner would
    /// never stop — so it settles.
    #[test]
    fn a_state_word_this_client_does_not_know_is_not_read_as_running() {
        for unknown in ["", "paused", "queued"] {
            let verdict = side_run_verdict(&reported(unknown, None));
            assert_ne!(
                verdict,
                SideRunVerdict::StillAnswering,
                "{unknown:?} must not hold the overlay open"
            );
            assert!(matches!(verdict, SideRunVerdict::Disconnected(_)));
        }
    }

    /// The gateway answered, and its answer is that it has never heard of this
    /// run — a core that restarted under the client. Not a verdict about the
    /// work, but nothing is going to stream it here either, so the spinner
    /// stops.
    ///
    /// The server's own words are carried through instead of paraphrased: the
    /// sentence around them has to stay true of any refusal this method might
    /// grow, and this client is not in a position to assert *why* a run is not
    /// being reported.
    #[test]
    fn a_gateway_with_no_record_of_the_run_stops_the_spinner() {
        let refused: Result<AgentRunStatusReport, CliError> = Err(CliError::Rpc {
            code: -32602,
            message: "Run not found".into(),
        });
        let SideRunVerdict::Disconnected(note) = side_run_verdict(&refused) else {
            panic!("an answered refusal must stop the spinner");
        };
        assert!(
            note.contains("Run not found"),
            "the gateway's own words are what the user can act on: {note}"
        );
    }

    /// "I could not ask" is not "it stopped".
    ///
    /// The socket can die again mid-repair, or the reply can time out. Settling
    /// on those would file a question whose run is very likely answering
    /// normally — and unlike the refusal above, asking again is free: the next
    /// successful reconnect runs this same repair.
    #[test]
    fn a_question_that_could_not_be_put_settles_nothing() {
        let transport = [
            CliError::Disconnected("Connection closed by peer".into()),
            CliError::Timeout("no response in 30s".into()),
            CliError::Connection("broken pipe".into()),
        ];
        for e in transport {
            let label = e.to_string();
            let answer: Result<AgentRunStatusReport, CliError> = Err(e);
            assert_eq!(
                side_run_verdict(&answer),
                SideRunVerdict::CouldNotAsk,
                "{label} says nothing about the run"
            );
        }
    }
}

#[cfg(test)]
mod last_run_face_tests {
    use super::{
        apply_history, last_run_mark, last_run_notice, session_entry_from_json, AttachMode,
    };
    use crate::tui::app::{AppState, TranscriptEntry};
    use aleph_protocol::{
        DanglingCallView, LastRunState, RunProgressView, SessionListRow, SessionSnapshot,
    };
    use serde_json::{json, Value};

    fn system_rows(state: &AppState) -> Vec<String> {
        state
            .messages
            .iter()
            .filter_map(|m| match m {
                TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Build the response the way the server does — by serialising the shared
    /// snapshot type. A hand-written literal here would only prove that serde
    /// round-trips bytes this test wrote itself (criterion #10).
    fn history_with(session: Option<Option<LastRunState>>) -> Value {
        let mut result = json!({ "messages": [], "active_run": Value::Null });
        if let Some(last_run) = session {
            result["session"] = serde_json::to_value(SessionSnapshot {
                session_key: "agent:main:main:s1".into(),
                last_run,
                ..SessionSnapshot::default()
            })
            .expect("the snapshot serialises");
        }
        result
    }

    fn interrupted() -> LastRunState {
        LastRunState {
            disposition: LastRunState::INTERRUPTED.into(),
            run_id: Some("run-9".into()),
            trailing_starts: 1,
            dangling: vec![DanglingCallView {
                call_id: "call-1".into(),
                tool_name: "shell".into(),
                provenance: DanglingCallView::THIS_RESTART.into(),
                denied: false,
                parked: None,
            }],
            progress: Some(RunProgressView {
                tool_calls_dispatched: 3,
                tool_calls_answered: 2,
                assistant_messages: 1,
                last_activity_ms: Some(1_750_000_000_000),
            }),
            contradictions: Vec::new(),
            inspected: true,
        }
    }

    fn notices(history: &Value) -> Vec<String> {
        let mut state = AppState::new("agent:main:main:s1".into(), "m".into());
        apply_history(&mut state, history, AttachMode::Replace);
        system_rows(&state)
            .into_iter()
            .filter(|r| r.contains("上一轮") || r.contains("会话日志不一致"))
            .collect()
    }

    /// The counts are the server's reduction, rendered — not recounted here.
    #[test]
    fn apply_history_emits_interrupted_line() {
        let lines = notices(&history_with(Some(Some(interrupted()))));
        assert_eq!(lines.len(), 1, "exactly one line about the previous run");
        assert!(
            lines[0].contains("2/3") && lines[0].contains("1 次结果未知"),
            "the landed/dispatched pair and the unknown count both ride: {}",
            lines[0]
        );
    }

    /// The three states of the field, and only the third says anything: absent
    /// is an older gateway, `null` is "asked, and there is nothing to report".
    /// Reading either as "the run was fine" is the failure the field exists to
    /// remove.
    #[test]
    fn an_absent_last_run_says_nothing() {
        assert!(
            notices(&history_with(None)).is_empty(),
            "no `session` at all — this client was told nothing"
        );
        assert!(
            notices(&history_with(Some(None))).is_empty(),
            "`session` without `last_run` — asked, not answered"
        );
    }

    /// A run that finished is not news.
    #[test]
    fn a_clean_last_run_says_nothing() {
        let clean = LastRunState {
            disposition: LastRunState::CLEAN.into(),
            inspected: true,
            ..LastRunState::default()
        };
        assert!(notices(&history_with(Some(Some(clean)))).is_empty());
    }

    /// A refused log is not a clean one, and the tag is what an operator takes
    /// to `aleph doctor`.
    #[test]
    fn a_refused_log_points_at_doctor() {
        let refused = LastRunState {
            disposition: LastRunState::LOG_INCONSISTENT.into(),
            contradictions: vec!["session-log-duplicate-dispatch".into()],
            inspected: true,
            ..LastRunState::default()
        };
        let lines = notices(&history_with(Some(Some(refused))));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("session-log-duplicate-dispatch") && lines[0].contains("doctor"),
            "the tag and where to take it: {}",
            lines[0]
        );
    }

    /// A log can carry dispatched calls that never came back and no run marker
    /// at all — that reduces to `never_ran`, not to `interrupted`. Keying the
    /// notice on the word alone would leave those calls rendered by nobody.
    #[test]
    fn dangling_calls_are_reported_even_when_the_word_is_not_interrupted() {
        let unmarked = LastRunState {
            disposition: LastRunState::NEVER_RAN.into(),
            dangling: vec![DanglingCallView {
                call_id: "call-2".into(),
                tool_name: "file_write".into(),
                provenance: DanglingCallView::EARLIER_RUN.into(),
                denied: false,
                parked: None,
            }],
            inspected: true,
            ..LastRunState::default()
        };
        let lines = notices(&history_with(Some(Some(unmarked))));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("1 次未回执"), "{}", lines[0]);
    }

    /// §6.1: a call the log shows parked at a gate never completed, and this
    /// screen must not count it among the outcomes nobody can vouch for. The
    /// split comes from the protocol's own predicate — the same bucket a
    /// denied call lands in, on every face.
    #[test]
    fn parked_calls_are_reported_as_never_completed() {
        let mut lr = interrupted();
        lr.dangling[0].parked = Some("approval".into());
        let lines = notices(&history_with(Some(Some(lr))));
        assert_eq!(lines.len(), 1, "still one line about the previous run");
        assert!(
            lines[0].contains("0 次结果未知") && lines[0].contains("1 次工具调用未完成"),
            "the parked call moves out of the unknown count: {}",
            lines[0]
        );
    }

    /// The picker's title is the row's own `topic`. It used to read `name` — a
    /// key `sessions.list` has never sent — so every row was titled by its key.
    #[test]
    fn session_entry_label_uses_topic_not_name() {
        let row = SessionListRow {
            key: "agent:main:main:s4".into(),
            topic: Some("Ship the resume receipt".into()),
            message_count: 12,
            ..SessionListRow::default()
        };
        let entry = session_entry_from_json(&serde_json::to_value(&row).expect("row serialises"))
            .expect("a keyed row makes an entry");
        assert_eq!(entry.key, "agent:main:main:s4");
        assert!(
            entry.label.starts_with("Ship the resume receipt") && entry.label.contains("12 msgs"),
            "{}",
            entry.label
        );
    }

    /// No topic, no label — the key is the honest fallback, and it is not a
    /// title this client invented.
    #[test]
    fn a_row_with_no_title_falls_back_to_its_key() {
        let row = SessionListRow {
            key: "agent:main:main:s5".into(),
            ..SessionListRow::default()
        };
        let entry = session_entry_from_json(&serde_json::to_value(&row).expect("row serialises"))
            .expect("a keyed row makes an entry");
        assert!(
            entry.label.starts_with("agent:main:main:s5"),
            "{}",
            entry.label
        );
    }

    /// The list face carries the word and nothing else, and the word is enough
    /// to mark the row.
    #[test]
    fn picker_marks_interrupted() {
        let row = SessionListRow {
            key: "agent:main:main:s6".into(),
            topic: Some("Crashed mid-tool".into()),
            last_run: Some(LastRunState::from_markers(
                LastRunState::INTERRUPTED,
                Some("run-3".into()),
                2,
            )),
            ..SessionListRow::default()
        };
        let entry = session_entry_from_json(&serde_json::to_value(&row).expect("row serialises"))
            .expect("a keyed row makes an entry");
        assert!(entry.label.contains("[interrupted]"), "{}", entry.label);
    }

    /// §5.2: the server's word for a user message no run ever answered says
    /// one sentence on attach. The picker-mark arm exists for exhaustiveness
    /// and the safe direction, but **a picker row cannot carry the word
    /// today**: `sessions.list` is fed by the marker-only list face, which by
    /// design never says `unanswered` (the message lives outside the
    /// markers). The mark assertion pins the arm, not a feature.
    #[test]
    fn the_unanswered_notice_names_the_fact_and_the_picker_mark_arm_is_pinned_though_unreachable() {
        let listed = LastRunState::from_markers(LastRunState::UNANSWERED, None, 0);
        assert_eq!(last_run_mark(&listed), Some("  [unanswered]"));
        let attached = LastRunState {
            disposition: LastRunState::UNANSWERED.into(),
            inspected: true,
            ..LastRunState::default()
        };
        let notice = last_run_notice(&attached).expect("an unanswered message is news");
        assert!(notice.contains("没有得到回答"), "{notice}");
    }

    /// A row the server said nothing about, and a row it said was clean, are
    /// both unmarked — a mark that appeared on every row would stop meaning
    /// anything.
    #[test]
    fn a_clean_or_silent_row_is_unmarked() {
        assert_eq!(
            last_run_mark(&LastRunState::from_markers(LastRunState::CLEAN, None, 0)),
            None
        );
        let row = SessionListRow {
            key: "agent:main:main:s7".into(),
            ..SessionListRow::default()
        };
        let entry = session_entry_from_json(&serde_json::to_value(&row).expect("row serialises"))
            .expect("a keyed row makes an entry");
        assert!(!entry.label.contains('['), "{}", entry.label);
    }

    /// The list face never looked for dangling calls, so its empty list is not
    /// evidence of anything — and `dangling()` refuses to hand it over. A mark
    /// derived from it would read "clean" off a face that never asked.
    #[test]
    fn the_list_face_marks_from_the_word_never_from_an_empty_dangling_list() {
        let listed = LastRunState::from_markers(LastRunState::INTERRUPTED, None, 1);
        assert!(listed.dangling().is_none(), "the list face withholds it");
        assert_eq!(last_run_mark(&listed), Some("  [interrupted]"));
    }
}
