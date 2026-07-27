//! Trace replay inspection commands

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};
use aleph_protocol::{
    present_agent_trace_event_with_preset, AgentTracePresentationPreset, AgentTraceReplay,
    AgentTraceReplayListItem,
};

/// List recent persisted trace replays.
pub async fn list(server_url: &str, config: &CliConfig, limit: usize, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client
        .call("trace.list", Some(serde_json::json!({ "limit": limit })))
        .await?;

    if json {
        output::print_json(&result);
    } else {
        let items: Vec<AgentTraceReplayListItem> = serde_json::from_value(result.clone())
            .map_err(|e| anyhow::anyhow!("Failed to parse trace list: {e}"))?;

        if items.is_empty() {
            println!("No traces found.");
        } else {
            let headers = &["TASK ID", "STARTED", "STATUS", "EVENTS"];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    vec![
                        item.task_id.clone(),
                        item.started_at.clone().unwrap_or_else(|| "-".to_string()),
                        item.status.clone(),
                        item.event_count.to_string(),
                    ]
                })
                .collect();
            output::print_table(headers, &rows, false, &result);
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
