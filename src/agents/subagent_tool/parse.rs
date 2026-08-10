//! Input JSON → `SubagentAction` parsing.
//!
//! Supports an explicit `action` discriminator plus legacy heuristics
//! (`request_id` → `check_status`, task → run). Validates field shape but
//! does no execution.

use serde_json::Value;

/// Render a [`serde_json::Value`] as a short kind tag (for parser error
/// messages — 'object', 'array', 'string', 'number', 'bool', 'null').
fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

use super::types::{
    max_run_timeout_secs, BatchTask, RunArgs, SubagentAction, ACCEPTED_ARG_KEYS,
    DEFAULT_RUN_TIMEOUT_SECS, DEFAULT_WAIT_TIMEOUT_SECS, MAX_WAIT_TIMEOUT_SECS,
};

/// Reject top-level keys the tool does not accept.
///
/// The hand-rolled parser reads the keys it knows and ignored everything else,
/// so a near-miss (`agent` for `agent_type`, `prompt` for `task`,
/// `background` for `run_in_background`) ran with a *different* meaning than the
/// caller asked for — the default role instead of the requested one, the
/// parent's model instead of the pinned one — and reported success. Rejecting is
/// the honest answer: the model reads the accepted set and retries correctly on
/// the next turn. This is what `#[serde(deny_unknown_fields)]` gives codex's V2
/// handlers for free; here it is explicit, with a drift guard against the
/// advertised schema.
///
/// An explicit JSON `null` counts as absent — schema-completing providers emit
/// `"key": null` for properties they are not using, and those carry no intent
/// (same rule as the `name` / `team_name` rejection below).
fn reject_unknown_keys(input: &Value) -> Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Ok(());
    };
    let mut unknown: Vec<&str> = obj
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, _)| k.as_str())
        .filter(|k| !ACCEPTED_ARG_KEYS.contains(k))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort_unstable();
    Err(format!(
        "unknown argument(s): {}. Accepted arguments: {}",
        unknown.join(", "),
        ACCEPTED_ARG_KEYS.join(", ")
    ))
}

/// Parse the input JSON into a [`SubagentAction`].
pub(super) fn parse_args(input: &Value) -> Result<SubagentAction, String> {
    reject_unknown_keys(input)?;

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
        // "result" is a defensive alias: older announce prompts coached the
        // model into a non-existent 'result' action; it reads the same way as
        // check_status. Deliberately not advertised in the schema/error text.
        "check_status" | "result" => {
            let rid = input
                .get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "check_status requires 'request_id' field".to_string())?;
            return Ok(SubagentAction::CheckStatus(rid.to_string()));
        }
        "wait" => {
            // Accept a single `request_id` (string) or `request_ids` (array).
            // Many ids → wait for whichever finishes first.
            let mut request_ids: Vec<String> = input
                .get("request_ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if request_ids.is_empty() {
                if let Some(s) = input.get("request_id").and_then(|v| v.as_str()) {
                    let s = s.trim();
                    if !s.is_empty() {
                        request_ids.push(s.to_string());
                    }
                }
            }
            if request_ids.is_empty() {
                return Err(
                    "wait requires 'request_id' (string) or a non-empty 'request_ids' array"
                        .to_string(),
                );
            }
            // Bounded window: default when omitted, clamped to [1, MAX] so a
            // single wait can never hang the turn past the tool budget.
            let timeout_secs = input
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS)
                .clamp(1, MAX_WAIT_TIMEOUT_SECS);
            return Ok(SubagentAction::Wait {
                request_ids,
                timeout_secs,
            });
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
            return Err(format!("unknown action '{other}'. Expected one of: run, check_status, wait, cancel, list, send_message, read_inbox"));
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
    //
    // B3-01: every entry that is not an object with a non-empty string
    // `task` must error, not silently drop. `reject_unknown_keys` exists
    // precisely because a near-miss that runs with a different meaning than
    // the caller asked for, and reports success, is the worst possible
    // outcome — yet the old `filter_map` shape did exactly that on every
    // entry. The two failure modes a silent drop enables are unacceptable:
    //   1. Partial drop — a 5-row request runs 4 children; renumbering from 0
    //      hides the loss.
    //   2. Total collapse — every entry malformed collapses the fan-out
    //      into a single ordinary sub-agent via `task`, reported as success.
    let max_run_timeout = max_run_timeout_secs();
    let batch_tasks: Option<Vec<BatchTask>> = match input.get("batch_tasks") {
        Some(v) if !v.is_null() => {
            let arr = v.as_array().ok_or_else(|| {
                format!("batch_tasks must be an array of objects (got {})", value_kind(v))
            })?;
            let mut rows = Vec::with_capacity(arr.len());
            for (idx, item) in arr.iter().enumerate() {
                let obj = item.as_object().ok_or_else(|| {
                    format!(
                        "batch_tasks[{idx}] must be an object with a non-empty string 'task' (got {})",
                        value_kind(item)
                    )
                })?;
                let task = obj
                    .get("task")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        format!("batch_tasks[{idx}] requires a non-empty string 'task'")
                    })?
                    .to_string();
                if task.trim().is_empty() {
                    return Err(format!(
                        "batch_tasks[{idx}]: 'task' must not be empty"
                    ));
                }
                rows.push(BatchTask {
                    task,
                    agent_type: obj
                        .get("agent_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    model: obj
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    // Clamped like the top-level value below — a per-entry
                    // override must not escape the tool-budget ordering
                    // either.
                    timeout_secs: obj
                        .get("timeout_secs")
                        .and_then(|v| v.as_u64())
                        .map(|t| t.clamp(1, max_run_timeout)),
                });
            }
            Some(rows)
        }
        _ => None,
    };
    let has_batch = batch_tasks.as_ref().is_some_and(|v| !v.is_empty());

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

    // Bounded window: default when omitted, clamped to [1, MAX] so the child's
    // own wall-clock timeout always fires before the `subagent` tool budget
    // (and a `0` can never mean "die before the first turn").
    let timeout_secs = input
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_RUN_TIMEOUT_SECS)
        .clamp(1, max_run_timeout);

    let run_in_background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let context_summary = input
        .get("context_summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Honest-trim: spawned sub-agents are NOT addressable teammates. The old
    // `name`/`team_name` run options registered a roster row the child could
    // never read (the SubAgent-mode recursion guard denies it the tool hosting
    // `read_inbox`, and the name never reached the child) — a silent
    // dead-letter trap. Reject loudly instead of silently ignoring. An
    // explicit JSON `null` counts as absent — schema-completing providers
    // emit `"team_name": null` on every call.
    if input.get("name").is_some_and(|v| !v.is_null())
        || input.get("team_name").is_some_and(|v| !v.is_null())
    {
        return Err(
            "'name'/'team_name' are not supported when spawning: sub-agents are not \
             addressable teammates (their results return to you, and background \
             completions are announced to you). For durable, addressable members use \
             the teams tools instead."
                .to_string(),
        );
    }

    // batch_tasks honors `run_in_background` exactly as the user provides it:
    // - false (default / explicit): run all sub-tasks in parallel, await all,
    //   return aggregated results. This matches the natural Think→Act loop
    //   expectation that a tool call returns its result.
    // - true: fire-and-forget — spawn all sub-tasks in background and return
    //   a list of request_ids. The caller is then responsible for polling
    //   `check_status` on each one. (Useful for very long-running batches.)

    // Mixture-of-Agents (MoA) inputs.
    let proposer_models = input
        .get("proposer_models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    let synthesize = input
        .get("synthesize")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let aggregator_model = input
        .get("aggregator_model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let synthesis_instruction = input
        .get("synthesis_instruction")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // `proposer_models` replicates the top-level `task`, so that task must be
    // present. (Explicit `batch_tasks` carry their own per-entry tasks and
    // take precedence — proposer_models is the convenience shorthand.)
    if proposer_models.is_some() && !has_batch && task.trim().is_empty() {
        return Err(
            "'proposer_models' replicates the top-level 'task' across models — 'task' must be set"
                .to_string(),
        );
    }

    // Synthesis is a reduce over a foreground fan-out; it has nothing to fold
    // when the batch is fire-and-forget.
    if synthesize && run_in_background {
        return Err(
            "'synthesize' requires a foreground batch (set run_in_background=false) so the \
             aggregator has completed proposals to fold"
                .to_string(),
        );
    }

    Ok(SubagentAction::Run(RunArgs {
        task,
        agent_type,
        model,
        timeout_secs,
        run_in_background,
        context_summary,
        batch_tasks,
        proposer_models,
        synthesize,
        aggregator_model,
        synthesis_instruction,
    }))
}
