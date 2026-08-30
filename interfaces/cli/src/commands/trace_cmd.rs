//! Trace replay inspection commands

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};
use aleph_protocol::{
    present_agent_trace_event_with_preset, AgentTraceListPage, AgentTracePresentationPreset,
    AgentTraceReplay,
};

/// Epoch **seconds** → a local-time stamp for the STARTED column.
///
/// The unit is not guessable from the wire (`started_at` is a bare integer), so
/// it lives in one function rather than at each render site; `AgentTraceListRow`
/// states it, and this is the only reader.
fn format_epoch_secs(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map_or_else(|| secs.to_string(), |dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// UTF-8-safe truncation — `&s[..n]` panics mid-codepoint on CJK prompts.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(max_chars)
        .map_or(text.len(), |(idx, _)| idx);
    format!("{}…", &text[..end])
}

/// List recent persisted trace replays.
pub async fn list(server_url: &str, config: &CliConfig, limit: usize, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client
        .call("trace.list", Some(serde_json::json!({ "limit": limit })))
        .await?;

    if json {
        output::print_json(&result);
    } else {
        // ⚠️ `AgentTraceListPage`, not `Vec<_>`: the server answers with an
        // ENVELOPE `{traces, next_cursor}`. Parsing the whole result as a
        // sequence is what made this command fail on every invocation with
        // `invalid type: map, expected a sequence` — see the type's own doc.
        let page: AgentTraceListPage = serde_json::from_value(result.clone())
            .map_err(|e| CliError::Other(format!("Failed to parse trace list: {e}")))?;

        if page.traces.is_empty() {
            println!("No traces found.");
        } else {
            let headers = &["TASK ID", "STARTED", "STATUS", "EVENTS", "PROMPT"];
            let rows: Vec<Vec<String>> = page
                .traces
                .iter()
                .map(|item| {
                    vec![
                        item.task_id.clone(),
                        item.started_at.map_or_else(|| "-".to_string(), format_epoch_secs),
                        item.status.clone(),
                        item.event_count.to_string(),
                        truncate_chars(&item.prompt_preview, 48),
                    ]
                })
                .collect();
            output::print_table(headers, &rows, false, &result);
            if page.next_cursor.is_some() {
                println!(
                    "\n(more available — raise --limit; this page shows {})",
                    page.traces.len()
                );
            }
        }
    }

    client.close().await?;
    Ok(())
}

/// Show a structured replay for a persisted task.
pub async fn show(
    server_url: &str,
    config: &CliConfig,
    task_id: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client
        .call("trace.get", Some(serde_json::json!({ "task_id": task_id })))
        .await?;

    if json {
        output::print_json(&result);
    } else {
        let replay: AgentTraceReplay = serde_json::from_value(result)?;

        // Print task summary header
        let task = &replay.task;
        println!("Task:    {}", task.task_id);
        println!("Agent:   {}", task.agent_id);
        println!("Session: {}", task.session_id);
        println!("Status:  {}", task.status);
        println!("Prompt:  {}", task.prompt_preview);
        println!("Events:  {}", task.trace_count);
        println!();

        // Print each trace event using the CliCompact preset
        for entry in &replay.traces {
            if let Some(presentation) = present_agent_trace_event_with_preset(
                &entry.event,
                AgentTracePresentationPreset::CliCompact,
            ) {
                println!(
                    "[{:>3}] {} {}",
                    entry.step, presentation.kind, presentation.content
                );
            }
        }
    }

    client.close().await?;
    Ok(())
}
