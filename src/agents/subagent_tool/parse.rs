//! Input JSON → `SubagentAction` parsing.
//!
//! Supports an explicit `action` discriminator plus legacy heuristics
//! (request_id → check_status, task → run). Validates field shape but
//! does no execution.

use serde_json::Value;

use super::types::{BatchTask, RunArgs, SubagentAction};

/// Parse the input JSON into a [`SubagentAction`].
pub(super) fn parse_args(input: &Value) -> Result<SubagentAction, String> {
    // Determine action from explicit field, falling back to legacy heuristics.
    let action = match input.get("action") {
        Some(v) => match v.as_str() {
            Some(s) => s,
            None => return Err("'action' must be a string".to_string()),
        },
        None => "",
    };

    match action {
        "send_message" => {
            let to = input
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "send_message requires 'to' field".to_string())?;
            let text = input
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "send_message requires 'text' field".to_string())?;
            let team_name = input
                .get("team_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "send_message requires 'team_name' field".to_string())?;
            return Ok(SubagentAction::SendMessage {
                to: to.to_string(),
                text: text.to_string(),
                team_name: team_name.to_string(),
            });
        }
        "read_inbox" => {
            let team_name = input
                .get("team_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "read_inbox requires 'team_name' field".to_string())?;
            return Ok(SubagentAction::ReadInbox {
                team_name: team_name.to_string(),
            });
        }
        "check_status" => {
            let rid = input
                .get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "check_status requires 'request_id' field".to_string())?;
            return Ok(SubagentAction::CheckStatus(rid.to_string()));
        }
        "cancel" => {
            let rid = input
                .get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "cancel requires 'request_id' field".to_string())?;
            return Ok(SubagentAction::Cancel(rid.to_string()));
        }
        "list" => {
            return Ok(SubagentAction::List);
        }
        // "run" or "" (default) — fall through to legacy run/check_status logic
        "run" | "" => {}
        other => {
            return Err(format!("unknown action '{other}'. Expected one of: run, check_status, cancel, list, send_message, read_inbox"));
        }
    }

    // Legacy heuristic: request_id without task → check_status
    let request_id = input
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if let Some(rid) = request_id {
        if task.is_none() || task.as_ref().is_some_and(|t| t.trim().is_empty()) {
            return Ok(SubagentAction::CheckStatus(rid));
        }
    }

    // Parse batch_tasks early — when present, top-level `task` is optional
    // since each sub-task carries its own.
    let batch_tasks = input
        .get("batch_tasks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let task = item.get("task")?.as_str()?.to_string();
                    if task.trim().is_empty() {
                        return None;
                    }
                    Some(BatchTask {
                        task,
                        agent_type: item
                            .get("agent_type")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        model: item
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        timeout_secs: item.get("timeout_secs").and_then(|v| v.as_u64()),
                    })
                })
                .collect::<Vec<_>>()
        });
    let has_batch = batch_tasks.as_ref().map(|v| !v.is_empty()).unwrap_or(false);

    // Run action — top-level `task` is required UNLESS batch_tasks supplies
    // the actual sub-task descriptions.
    let task =
        match task {
            Some(t) if !t.trim().is_empty() => t,
            Some(_) if has_batch => String::new(),
            Some(_) => return Err("task must not be empty".to_string()),
            None if has_batch => String::new(),
            None => return Err(
                "missing required field: task (or provide request_id to check background status)"
                    .to_string(),
            ),
        };

    let agent_type = input
        .get("agent_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let model = input
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let timeout_secs = input
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);

    let run_in_background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let context_summary = input
        .get("context_summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let team_name = input
        .get("team_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Validate: team_name without name is an error
    if team_name.is_some() && name.is_none() {
        return Err("team_name requires 'name' to be set (agent must be addressable)".to_string());
    }

    // Named teammates always run in background — override explicitly at parse time
    let run_in_background = if name.is_some() {
        if !run_in_background {
            tracing::info!(
                "Named teammates always run in background — overriding run_in_background to true"
            );
        }
        true
    } else {
        run_in_background
    };

    // batch_tasks honors `run_in_background` exactly as the user provides it:
    // - false (default / explicit): run all sub-tasks in parallel, await all,
    //   return aggregated results. This matches the natural Think→Act loop
    //   expectation that a tool call returns its result.
    // - true: fire-and-forget — spawn all sub-tasks in background and return
    //   a list of request_ids. The caller is then responsible for polling
    //   `check_status` on each one. (Useful for very long-running batches.)

    Ok(SubagentAction::Run(RunArgs {
        task,
        agent_type,
        model,
        timeout_secs,
        run_in_background,
        context_summary,
        name,
        team_name,
        batch_tasks,
    }))
}
