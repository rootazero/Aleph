// Local slash-command execution: the TUI-side handlers for commands that resolve
// to Gateway RPCs (or pure client state), plus the session picker plumbing and
// result formatters.
//
// Extracted from `mod.rs` so the orchestrator file keeps only `run()` +
// `main_loop`. Every `agent.run` invocation funnels through `send_to_agent`, the
// single shared send site.

use serde_json::{json, Value};
use tui_textarea::TextArea;

use aleph_protocol::providers::{CatalogEntry, CatalogParams, CatalogResult, CatalogView};
use aleph_protocol::{
    AgentRunAccepted, AgentRunRequest, AgentTraceReplay, AgentTraceTaskSummary, SessionSnapshot,
};

use aleph_client::AlephClient;

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
        Ok(accepted) => state.adopt_canonical_session_key(&accepted.session_key),
        Err(e) => state.add_system_message(format!("{err_label}: {e}")),
    }
}

/// Execute a local slash command (TUI-only, no Gateway RPC needed).
pub(super) async fn execute_local_command(
    state: &mut AppState,
    textarea: &TextArea<'_>,
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
            match client
                .call::<_, Vec<AgentTraceTaskSummary>>("trace.list", Some(params))
                .await
            {
                Ok(tasks) => state.add_system_message(format_replay_list(&tasks)),
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
    }

    // Ensure textarea still has focus hint after command execution
    let _ = textarea;
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
fn session_entry_from_json(v: &Value) -> Option<app::SessionEntry> {
    let key = v.get("key").and_then(Value::as_str)?.to_string();
    let name = v.get("name").and_then(Value::as_str).unwrap_or("");
    let count = v.get("message_count").and_then(Value::as_u64);
    let label = match (name.is_empty(), count) {
        (false, Some(c)) => format!("{name}  ({c} msgs)"),
        (false, None) => name.to_string(),
        (true, Some(c)) => format!("{key}  ({c} msgs)"),
        (true, None) => key.clone(),
    };
    Some(app::SessionEntry { key, label })
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
    attach_session(state, client, &key).await;
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

/// Load a conversation: its transcript **and** the settings that govern it.
///
/// The single attach path, used both at launch and after a switch. One call,
/// because the transcript and the settings are one snapshot — a second RPC
/// would open a window in which the screen shows a conversation while the
/// status bar describes a different one's mode, tier and token count.
///
/// This is also why launching the TUI on an existing key finally shows
/// anything: `chat.history` was previously reached *only* from the session
/// picker, so `aleph-tui --session <key>` opened a blank screen over a
/// transcript the server had all along.
///
/// Failure is reported and survivable: a conversation that cannot be loaded
/// leaves the client on the key it was given with no settings, which the status
/// bar renders as "unknown" rather than as the global defaults.
pub(super) async fn attach_session(state: &mut AppState, client: &AlephClient, key: &str) {
    let params = json!({ "session_key": key });
    match client.call::<_, Value>("chat.history", Some(params)).await {
        Ok(result) => {
            let rows = result
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mapped: Vec<app::ChatMessage> =
                rows.iter().filter_map(history_message_from_json).collect();

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
            state.scroll_to_bottom();
        }
        Err(e) => state.add_system_message(format!("History error: {e}")),
    }
}

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
            timestamp: parse_history_timestamp(v),
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

/// Best-effort RFC3339 timestamp parse, falling back to now.
fn parse_history_timestamp(v: &Value) -> chrono::DateTime<chrono::Utc> {
    v.get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or_else(chrono::Utc::now, |dt| dt.with_timezone(&chrono::Utc))
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

async fn execute_undo(state: &mut AppState, client: &AlephClient) -> bool {
    if state.current_run.is_some() {
        state.add_system_message("Stop the active run first (/stop), then /undo.".to_string());
        return false;
    }
    // Count non-system messages — we only undo the last user+assistant pair.
    let conversational_count = state
        .messages
        .iter()
        .filter(|m| !matches!(m, app::ChatMessage::System { .. }))
        .count();
    if conversational_count < 2 {
        state.add_system_message("Nothing to undo.".to_string());
        return false;
    }
    let keep_count = conversational_count.saturating_sub(2);
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
    state.send_history.push(last_user.clone());
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

fn format_replay_list(tasks: &[AgentTraceTaskSummary]) -> String {
    if tasks.is_empty() {
        return "Recent replays:\n  (none)\n\nUse /replay <task_id> after traces are persisted."
            .to_string();
    }

    let mut lines = vec!["Recent replays:".to_string()];
    for task in tasks {
        lines.push(format!(
            "  {} [{}] {} traces  {}",
            task.task_id,
            task.status,
            task.trace_count,
            truncate_text(&task.prompt_preview, 72)
        ));
    }
    lines.push(String::new());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_line_renders_amount_and_na() {
        assert_eq!(
            cost_line("claude-sonnet-4-6", Some(1.2345)),
            "Cost estimate (claude-sonnet-4-6): $1.2345"
        );
        assert!(cost_line("mystery-model", None).contains("n/a"));
    }
}
