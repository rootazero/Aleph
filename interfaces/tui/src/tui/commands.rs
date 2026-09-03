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
    AgentRunAccepted, AgentRunRequest, AgentRunStatusReport, AgentRunStatusRequest,
    AgentTraceListPage, AgentTraceReplay, LastRunDisposition, LastRunState, RunPhase,
    SessionListRow, SessionSnapshot,
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
            state.add_user_message(text.to_string());
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
pub(super) async fn reconcile_side_question(state: &mut AppState, client: &AlephClient) {
    let Some(run_id) = state.btw.active_run_id().map(str::to_string) else {
        return;
    };
    let answer = client
        .call::<_, AgentRunStatusReport>(
            "agent.status",
            Some(AgentRunStatusRequest {
                run_id: run_id.clone(),
            }),
        )
        .await;
    match side_run_verdict(&answer) {
        // Nothing is claimed on either: one is "it is still going", the other
        // is "I do not know". Both leave the overlay as it is.
        SideRunVerdict::StillAnswering | SideRunVerdict::CouldNotAsk => {}
        SideRunVerdict::Failed(reason) => state.btw.fail_active(&run_id, reason),
        SideRunVerdict::Disconnected(note) => state.btw.settle_disconnected(&run_id, note),
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

/// Execute a local slash command (TUI-only, no Gateway RPC needed).
pub(super) async fn execute_local_command(
    state: &mut AppState,
    _textarea: &TextArea<'_>,
    client: &AlephClient,
    cmd: LocalCommand,
) {
    match cmd {
        LocalCommand::Clear => {
            state.clear_screen();
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
        LocalCommand::ReplayList => {
            let params = json!({ "limit": 10 });
            // The server answers `{traces, next_cursor}`. This asked for
            // `Vec<AgentTraceTaskSummary>` — a THIRD shape, different from both
            // the server's and the CLI's — so `/replay list` had never once
            // rendered a row. One contract type now, shared by both clients.
            match client
                .call::<_, AgentTraceListPage>("trace.list", Some(params))
                .await
            {
                Ok(page) => state.add_system_message(format_replay_list(&page)),
                Err(e) => state.add_system_message(format!("Replay list error: {e}")),
            }
        }
        LocalCommand::ReplayShow { task_id } => {
            let params = json!({ "task_id": task_id });
            match client
                .call::<_, AgentTraceReplay>("trace.get", Some(params))
                .await
            {
                Ok(replay) => state.load_trace_replay(&replay),
                Err(e) => state.add_system_message(format!("Replay load error: {e}")),
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
        LocalCommand::Todo => {
            state.tasks_panel_visible = !state.tasks_panel_visible;
            let notice = match (state.tasks_panel_visible, &state.plan) {
                (true, Some(plan)) if plan.has_content() => "Tasks panel shown.",
                (true, _) => "Tasks panel shown (it appears once this conversation has a plan).",
                (false, _) => "Tasks panel hidden. /todo to bring it back.",
            };
            state.add_system_message(notice.to_string());
        }
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
        LastRunDisposition::Clean | LastRunDisposition::NeverRan if dangling => {
            Some("  [interrupted]")
        }
        LastRunDisposition::Clean | LastRunDisposition::NeverRan => None,
    }
}

/// Confirm the highlighted session: load its history and re-point the session.
pub(super) async fn confirm_session_switch(state: &mut AppState, client: &AlephClient) {
    let Some(key) = state.selected_session_key() else {
        state.close_overlay();
        return;
    };
    state.close_overlay();
    // switch_session clears transient state (including the token counter and
    // the settings of the conversation being left) and adds a "Switched"
    // banner; `attach_session` then restores the incoming conversation's own.
    state.switch_session(&key);
    attach_session(state, client, &key, AttachMode::Append).await;
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
            state.add_user_message(command.clone());
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
pub(super) async fn attach_session(
    state: &mut AppState,
    client: &AlephClient,
    key: &str,
    mode: AttachMode,
) {
    let params = json!({ "session_key": key });
    match client.call::<_, Value>("chat.history", Some(params)).await {
        Ok(result) => {
            apply_history(state, &result, mode);
            // The sub-agent tree is per-session state the transcript does not
            // carry; seed it from the tracker snapshot the same way the plan
            // was seeded from the history response above.
            refresh_agents(state, client).await;
        }
        Err(e) => state.add_system_message(format!("History error: {e}")),
    }
}

/// Apply one `chat.history` response to this screen.
///
/// Split out from the call so the decision it carries is reachable without a
/// gateway — and the decision is WHEN, not just whether: `AttachMode::Replace`
/// drops the old transcript here, which is inside the caller's `Ok` arm by
/// construction. A reattach whose fetch failed therefore cannot blank the
/// screen, because this never runs.
fn apply_history(state: &mut AppState, result: &Value, mode: AttachMode) {
    let rows = result
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped: Vec<app::ChatMessage> = rows.iter().filter_map(history_message_from_json).collect();

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
    let dangling = last_run.dangling().map(<[_]>::len);
    match last_run.disposition() {
        LastRunDisposition::LogInconsistent => {
            let tags = if last_run.contradictions.is_empty() {
                "未报告".to_string()
            } else {
                last_run.contradictions.join("、")
            };
            Some(format!(
                "会话日志不一致（{tags}）— 恢复已拒绝，请运行 aleph doctor"
            ))
        }
        LastRunDisposition::Interrupted => Some(match (last_run.progress, dangling) {
            (Some(p), Some(n)) => format!(
                "上一轮运行被中断 — {}/{} 次工具回执已落盘，{n} 次结果未知",
                p.tool_calls_answered, p.tool_calls_dispatched
            ),
            _ => "上一轮运行被中断".to_string(),
        }),
        LastRunDisposition::Unrecognized => Some(format!(
            "上一轮运行状态未知（{}）— 本客户端无法判断",
            last_run.disposition
        )),
        // A log can hold dispatched calls that never came back and still carry
        // no run marker at all, which reduces to `never_ran`. Keying the notice
        // on the word alone would leave those calls produced by the server and
        // rendered by nobody (criterion #17).
        LastRunDisposition::Clean | LastRunDisposition::NeverRan => match dangling {
            Some(n) if n > 0 => Some(format!("上一轮留下 {n} 次未回执的工具调用 — 结果未知")),
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

/// Map one `chat.history` row (`{role, content, timestamp}`) into a `ChatMessage`.
/// Map one `chat.history` row (`{role, content, timestamp}`) into a `ChatMessage`.
fn history_message_from_json(v: &Value) -> Option<app::ChatMessage> {
    let role = v.get("role").and_then(Value::as_str).unwrap_or("");
    let content = v
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match role {
        "user" => Some(app::ChatMessage::User {
            content,
            timestamp: app::row_timestamp(v.get("timestamp").and_then(Value::as_str)),
        }),
        "assistant" => Some(app::ChatMessage::Assistant {
            content,
            tools: Vec::new(),
            reasoning: None,
            is_streaming: false,
        }),
        "system" => Some(app::ChatMessage::System { content }),
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
    state.add_user_message(last_user.clone());
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
            state.record_local_knob(app_knob(knob), stored);
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

/// Map the parser's knob onto the state's.
///
/// Two enums because they answer to two owners — the parser's list is "what a
/// user may type", the state's is "what the status bar can show" — but the
/// mapping is total in this direction, so a knob added to the parser without a
/// state cell is a compile error here rather than a silently invisible setting.
const fn app_knob(knob: SlashKnob) -> app::SessionKnob {
    match knob {
        SlashKnob::ExecTier => app::SessionKnob::ExecTier,
        SlashKnob::Mode => app::SessionKnob::Mode,
        SlashKnob::Think => app::SessionKnob::ThinkLevel,
        SlashKnob::Memory => app::SessionKnob::MemoryMode,
    }
}

/// Return a clone of the last User message's content, if any.
fn last_user_message(state: &AppState) -> Option<String> {
    state.messages.iter().rev().find_map(|m| match m {
        app::ChatMessage::User { content, .. } => Some(content.clone()),
        _ => None,
    })
}

/// Locally pop the last user+assistant pair from chat history, after a successful
/// server-side truncate. Keeps the TUI in sync without a full reload round-trip.
fn pop_last_turn_locally(state: &mut AppState) {
    // Drop trailing system messages first (they may have been added after the turn).
    while matches!(state.messages.last(), Some(app::ChatMessage::System { .. })) {
        state.messages.pop();
    }
    // Drop the trailing assistant turn (if any)…
    if matches!(
        state.messages.last(),
        Some(app::ChatMessage::Assistant { .. })
    ) {
        state.messages.pop();
    }
    // …and the preceding user message.
    if matches!(state.messages.last(), Some(app::ChatMessage::User { .. })) {
        state.messages.pop();
    }
}

fn format_replay_list(page: &AgentTraceListPage) -> String {
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
    use super::{apply_history, AttachMode};
    use crate::tui::app::{AppState, ChatMessage};
    use serde_json::json;

    fn user_rows(state: &AppState) -> Vec<&str> {
        state
            .messages
            .iter()
            .filter_map(|m| match m {
                ChatMessage::User { content, .. } => Some(content.as_str()),
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

    /// Launch and `/session` prepare the list themselves — the first keeps the
    /// welcome banner and the startup notices, the second wipes it in
    /// `switch_session`. Both then append.
    #[test]
    fn an_appending_attach_keeps_what_the_caller_prepared() {
        let mut state = AppState::new("agent:main:main:s1".into(), "m".into());
        state.messages.push(ChatMessage::User {
            content: "prepared by the caller".into(),
            timestamp: crate::tui::app::row_timestamp(None),
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
        state.messages.push(ChatMessage::User {
            content: "stale, from before the drop".into(),
            timestamp: crate::tui::app::row_timestamp(None),
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
    fn nothing_is_thrown_away_until_the_servers_copy_is_in_hand() {
        let src = include_str!("commands.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or_default();
        let attach = production
            .split("pub(super) async fn attach_session")
            .nth(1)
            .expect("attach_session must exist");
        let body = attach
            .split("\nfn apply_history")
            .next()
            .unwrap_or_default();
        // Self-protection: a scanner that matched nothing would agree with
        // everything below. Pin that this really is the function's body.
        assert!(
            body.contains("chat.history"),
            "the scan found no attach_session body — the guard is blind, not clean"
        );
        assert!(
            !body.contains("messages.clear()"),
            "attach_session must not touch the transcript itself — the clear \
             belongs in `apply_history`, which only runs on a successful fetch"
        );
        // Normalize line endings first: `include_str!` preserves the checkout's
        // bytes, and a CRLF checkout would turn a `\n`-carrying needle into a
        // guard that can never match (the "guard only recognizes the shape it
        // was written on" defect, CRLF edition).
        let body = body.replace("\r\n", "\n");
        assert!(
            body.contains("Ok(result) => {\n            apply_history(state, &result, mode);"),
            "the applier must be reached only from the success arm"
        );
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
    use super::{apply_history, last_run_mark, session_entry_from_json, AttachMode};
    use crate::tui::app::{AppState, ChatMessage};
    use aleph_protocol::{
        DanglingCallView, LastRunState, RunProgressView, SessionListRow, SessionSnapshot,
    };
    use serde_json::{json, Value};

    fn system_rows(state: &AppState) -> Vec<String> {
        state
            .messages
            .iter()
            .filter_map(|m| match m {
                ChatMessage::System { content } => Some(content.clone()),
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
    fn an_unanswered_last_run_says_nothing() {
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
            }],
            inspected: true,
            ..LastRunState::default()
        };
        let lines = notices(&history_with(Some(Some(unmarked))));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("1 次未回执"), "{}", lines[0]);
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

    /// A row the server said nothing about, and a row it said was clean, are
    /// both unmarked — a mark that appeared on every row would stop meaning
    /// anything.
    #[test]
    fn a_clean_or_unanswered_row_is_unmarked() {
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
