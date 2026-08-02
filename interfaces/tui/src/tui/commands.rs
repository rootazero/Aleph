// Local slash-command execution: the TUI-side handlers for commands that resolve
// to Gateway RPCs (or pure client state), plus the session picker plumbing and
// result formatters.
//
// Extracted from `mod.rs` so the orchestrator file keeps only `run()` +
// `main_loop`. Every `agent.run` invocation funnels through `send_to_agent`, the
// single shared send site.

use serde_json::{json, Value};
use tui_textarea::TextArea;

use aleph_protocol::{AgentTraceReplay, AgentTraceTaskSummary};

use aleph_client::AlephClient;

use super::app::{self, AppState};
use super::command_tree;
use super::slash::{self, LocalCommand, ToolProgressMode};

/// Fire a message to the agent via `agent.run`.
///
/// This is the single `agent.run` call site shared by every sender
/// (SendMessage, GatewayCommand, the palette-confirm gateway path, and
/// `/retry`). Callers keep their own transcript/history bookkeeping and delegate
/// only the RPC + failure notice here, so the request shape can never drift
/// across the four paths.
pub(super) async fn send_to_agent(
    state: &mut AppState,
    client: &AlephClient,
    message: &str,
    err_label: &str,
) {
    let params = json!({
        "session_key": state.session_key,
        "message": message,
    });
    if let Err(e) = client.call::<_, Value>("agent.run", Some(params)).await {
        state.add_system_message(format!("{err_label}: {e}"));
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
        LocalCommand::Tier { level } => execute_tier(state, client, level).await,
        LocalCommand::Sessions => execute_sessions(state, client).await,
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

            // switch_session clears transient state + adds a "Switched" banner;
            // then render the server transcript verbatim (no local dedup/store).
            state.switch_session(&key);
            for msg in mapped {
                state.messages.push(msg);
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

/// Set (or report usage for) the session's execution tier via `sessions.patch`.
///
/// The exec tier gates tool-approval prompts (Ask / Auto / Full). This is the
/// TUI analogue of the Panel composer pill — an explicit operator control, not
/// an LLM-mediated decision — and writes the same `metadata.exec_tier` key the
/// server validates. `default` clears the per-session override (follow global
/// policy) via a null write; an unrecognised arg parses to `None` and prints the
/// usage hint (mirrors `/tools`).
async fn execute_tier(state: &mut AppState, client: &AlephClient, level: Option<String>) {
    let Some(level) = level else {
        state.add_system_message(
            "Exec tier gates tool-approval prompts. Usage: /tier ask|auto|full \
             (or /tier default to follow the global policy)"
                .to_string(),
        );
        return;
    };

    // `default` clears the per-session override; the server treats a null
    // exec_tier metadata write as "follow global policy".
    let tier_value = if level == "default" {
        Value::Null
    } else {
        Value::String(level.clone())
    };
    let params = json!({
        "session_key": state.session_key,
        "metadata": { "exec_tier": tier_value },
    });
    match client
        .call::<_, Value>("sessions.patch", Some(params))
        .await
    {
        Ok(_) => {
            let shown = if level == "default" {
                "default (follow global policy)".to_string()
            } else {
                level
            };
            state.add_system_message(format!("Exec tier set to {shown}."));
        }
        Err(e) => state.add_system_message(format!("Tier error: {e}")),
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
