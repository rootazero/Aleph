//! `LoopTool` trait implementation for `SubagentTool`.

use async_trait::async_trait;
use futures::FutureExt;
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;

use crate::agents::background_tracker::{
    CompletedOutcome, CompletedSnapshot, WaitAnyOutcome, WaitOutcome,
};
use crate::agents::progress::SubagentProgress;
use crate::agents::runtime::AgentRuntimeConfig;
use crate::agents::AgentDef;
use crate::teams::messages::router::SendRequest;
use crate::teams::messages::types::MessageType;
use crate::tools::runtime::{LoopTool, ToolResult};
use tokio_util::sync::CancellationToken;

use super::parse::parse_args;
use super::spawn::CancelGuard;
use super::types::{BatchTask, SubagentAction};
use super::SubagentTool;

#[async_trait]
impl LoopTool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Delegate tasks to autonomous sub-agents. For simple single tasks, use 'task'. \
         For complex goals that can be broken into independent sub-tasks, use 'batch_tasks' \
         to launch multiple sub-agents in parallel — the system automatically runs them \
         in background and returns request_ids. Background completions are announced \
         back to you proactively as a system message — no need to poll. When you must \
         block on a specific result before continuing, use 'wait' (request_id): it parks \
         until that sub-agent finishes or a bounded window elapses, costing ONE call \
         instead of repeated check_status polls. check_status/list remain available for \
         on-demand inspection. \
         For Mixture-of-Agents (best-quality answers to one hard question), set \
         'proposer_models' to a list of models and 'synthesize'=true: the same 'task' runs \
         on every model in parallel, then one aggregator sub-agent folds the proposals into \
         a single synthesized answer. 'synthesize' also works on an explicit 'batch_tasks' \
         fan-out to add a reduce/aggregation step over the parallel results."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run", "check_status", "wait", "cancel", "list", "send_message", "read_inbox"],
                    "description": "The action to perform. Defaults to 'run' (or 'check_status' if only request_id is provided). 'wait' blocks (event-driven, no busy-poll) until a background sub-agent finishes or the bounded 'timeout_secs' window elapses — pass 'request_id' to wait on one, or 'request_ids' to wait for whichever of a set finishes first. It returns the completed result in ONE call, or status 'still_running' so you can wait again. 'cancel' interrupts a still-running background sub-agent identified by request_id. 'list' enumerates every background sub-agent (running and recently-completed) with their request_ids — use it to recover a request_id you no longer hold."
                },
                "task": {
                    "type": "string",
                    "description": "A clear description of the task for the sub-agent to complete. Use this for single tasks."
                },
                "batch_tasks": {
                    "type": "array",
                    "description": "Array of independent sub-tasks to execute in parallel. Use this when the overall goal can be decomposed into multiple independent sub-tasks. Each item can specify its own task, agent_type, model, and timeout_secs. By default (run_in_background=false), all tasks run in parallel and the call awaits every one; the response carries the aggregated results array (no polling needed). Set run_in_background=true only if you want fire-and-forget request_ids to poll later via check_status.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "task": {
                                "type": "string",
                                "description": "Description of this sub-task."
                            },
                            "agent_type": {
                                "type": "string",
                                "description": "Agent type for this sub-task. Inherits from top-level agent_type if not set."
                            },
                            "model": {
                                "type": "string",
                                "description": "Model hint for this sub-task. Inherits from top-level model if not set."
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Timeout for this sub-task. Inherits from top-level timeout_secs if not set."
                            }
                        },
                        "required": ["task"]
                    }
                },
                "proposer_models": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Mixture-of-Agents shorthand: replicate the top-level 'task' across these models as parallel proposers (same prompt, different model). Pair with synthesize=true to fold their answers into one. Ignored when explicit 'batch_tasks' is provided."
                },
                "synthesize": {
                    "type": "boolean",
                    "description": "After a synchronous batch (batch_tasks or proposer_models) fans out and all proposers return, run ONE aggregator sub-agent that synthesizes the proposals into a single answer (Mixture-of-Agents reduce). Requires run_in_background=false. Returns status 'moa_completed' with a 'synthesis' field plus the raw 'results'.",
                    "default": false
                },
                "aggregator_model": {
                    "type": "string",
                    "description": "Model for the MoA aggregator run. Defaults to the top-level 'model'. Use a strong model here for best synthesis."
                },
                "synthesis_instruction": {
                    "type": "string",
                    "description": "Optional extra guidance for the MoA aggregator, added on top of the default merge-and-reconcile instruction."
                },
                "agent_type": {
                    "type": "string",
                    "description": "The type of agent to use (e.g., 'explore', 'coder', 'researcher', 'plan', 'verify'). Defaults to 'default'."
                },
                "model": {
                    "type": "string",
                    "description": "Model hint for the sub-agent (e.g., 'fast', 'deep')."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "For 'run': maximum seconds the sub-agent may run (default 120). For 'wait': the bounded blocking window in seconds (default 120, capped at 600) — on elapse you get 'still_running' and may wait again.",
                    "default": 120
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "If true, run the sub-agent in the background and return immediately with a request_id.",
                    "default": false
                },
                "context_summary": {
                    "type": "string",
                    "description": "A summary of the parent agent's context to pass to the sub-agent."
                },
                "request_id": {
                    "type": "string",
                    "description": "Identifies a background sub-agent for check_status / wait / cancel. Provide request_id without task to retrieve the result (check_status)."
                },
                "request_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "For 'wait' only: a set of background request_ids to wait on. 'wait' returns as soon as the FIRST of them finishes (reporting which), so you can react to a fan-out in completion order. Use 'request_id' instead to wait on a single sub-agent."
                },
                "name": {
                    "type": "string",
                    "description": "Optional name for the sub-agent, making it addressable by teammates."
                },
                "team_name": {
                    "type": "string",
                    "description": "Optional team name. Enables shared tasks and inter-agent messaging. Requires 'name' to be set."
                },
                "to": {
                    "type": "string",
                    "description": "Target agent name for send_message action."
                },
                "text": {
                    "type": "string",
                    "description": "Message text for send_message action."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value, cancel: CancellationToken) -> ToolResult {
        // Gap B follow-up — the harness Act phase forks a per-call child of
        // the run cancel and threads it here. `cancel_for_child_with(&cancel)`
        // merges it with the run-level `parent_cancel` so a spawned subagent
        // runtime stops on EITHER the run being cancelled OR this specific
        // tool call being cancelled (via the upcoming per-tool cancel RPC).
        //
        // `SubagentAction::Cancel` (the LLM-level cancel-by-request_id path)
        // continues to operate on the `BackgroundTracker`'s per-request
        // cancellation token — it does NOT consume the harness `cancel`.
        // 1. Parse arguments
        let action = match parse_args(&input) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::Error {
                    error: e,
                    retryable: false,
                }
            }
        };

        // Handle non-run actions
        let args = match action {
            SubagentAction::SendMessage {
                to,
                text,
                team_name,
            } => {
                let router = match &self.message_router {
                    Some(r) => r.clone(),
                    None => {
                        return ToolResult::Error {
                            error: "send_message requires a message router (not configured)"
                                .to_string(),
                            retryable: false,
                        };
                    }
                };

                // Resolve team_name to team_id via teammate_manager
                let resolved_team_id = if let Some(ref mgr) = self.teammate_manager {
                    match mgr.ensure_team(&team_name, &self.parent_agent_id).await {
                        Ok(id) => id,
                        Err(e) => {
                            return ToolResult::Error {
                                error: format!("Failed to resolve team '{team_name}': {e}"),
                                retryable: false,
                            };
                        }
                    }
                } else {
                    team_name.clone()
                };

                match router
                    .send(SendRequest {
                        team_id: resolved_team_id,
                        from_agent: self.parent_agent_id.clone(),
                        to: vec![to.clone()],
                        cc: vec![],
                        msg_type: MessageType::Message,
                        subject: format!("Message to {to}"),
                        content: text,
                        reply_to: None,
                        attachments: vec![],
                    })
                    .await
                {
                    Ok(sent) => {
                        return ToolResult::Success {
                            output: json!({
                                "status": "sent",
                                "message_id": sent.id,
                                "to": to,
                            }),
                        };
                    }
                    Err(e) => {
                        return ToolResult::Error {
                            error: format!("Failed to send message: {e}"),
                            retryable: false,
                        };
                    }
                }
            }
            SubagentAction::ReadInbox { team_name } => {
                let inbox = match &self.inbox {
                    Some(i) => i.clone(),
                    None => {
                        return ToolResult::Error {
                            error: "read_inbox requires an inbox (not configured)".to_string(),
                            retryable: false,
                        };
                    }
                };

                // Resolve team_name to team_id via teammate_manager
                let resolved_team_id = if let Some(ref mgr) = self.teammate_manager {
                    match mgr.ensure_team(&team_name, &self.parent_agent_id).await {
                        Ok(id) => id,
                        Err(e) => {
                            return ToolResult::Error {
                                error: format!("Failed to resolve team '{team_name}': {e}"),
                                retryable: false,
                            };
                        }
                    }
                } else {
                    team_name.clone()
                };

                match inbox
                    .read(&self.parent_agent_id, &resolved_team_id, None, true)
                    .await
                {
                    Ok(messages) => {
                        let summaries: Vec<Value> = messages
                            .iter()
                            .map(|m| {
                                json!({
                                    "id": m.id,
                                    "from": m.from_agent,
                                    "subject": m.subject,
                                    "content": m.content,
                                    "type": m.msg_type.as_str(),
                                })
                            })
                            .collect();
                        return ToolResult::Success {
                            output: json!(summaries),
                        };
                    }
                    Err(e) => {
                        return ToolResult::Error {
                            error: format!("Failed to read inbox: {e}"),
                            retryable: false,
                        };
                    }
                }
            }
            SubagentAction::CheckStatus(request_id) => {
                // Running? — surface elapsed time + a derived activity
                // summary alongside the recent progress events.
                if let Some(meta) = self.background_tracker.running_meta(&request_id) {
                    let progress = self.background_tracker.progress_snapshot(&request_id, 10);
                    return ToolResult::Success {
                        output: json!({
                            "status": "running",
                            "request_id": request_id,
                            "task": meta.task,
                            "elapsed_secs": meta.elapsed_secs,
                            "summary": summarize_progress(&progress),
                            "progress": progress,
                        }),
                    };
                }
                // Completed? — non-destructive read, so the parent may poll
                // the same request_id again later without it vanishing.
                match self.background_tracker.result_snapshot(&request_id) {
                    Some(snap) => {
                        // The parent has now seen the terminal result on-demand;
                        // mark it consumed so the proactive announce does not
                        // spend a fresh turn re-delivering it (dedup with wait).
                        self.background_tracker.mark_consumed(&request_id);
                        if let CompletedOutcome::Err(err) = &snap.outcome {
                            return ToolResult::Error {
                                error: error_with_trail(err, &snap.progress_tail),
                                retryable: false,
                            };
                        }
                        return ToolResult::Success {
                            output: completed_to_json(&request_id, "completed", &snap),
                        };
                    }
                    None => {
                        return ToolResult::Error {
                            error: format!(
                                "No background sub-agent found with request_id '{request_id}'"
                            ),
                            retryable: false,
                        };
                    }
                }
            }
            SubagentAction::Wait {
                request_ids,
                timeout_secs,
            } => {
                // Park on the tracker's completion notifier (no busy-poll, no
                // per-check LLM turn). The delivered result is marked consumed
                // inside the tracker so the announce won't re-deliver it.
                let dur = std::time::Duration::from_secs(timeout_secs);

                // Single id → the simple wait (nicer elapsed_secs on timeout).
                if request_ids.len() == 1 {
                    let request_id = &request_ids[0];
                    return match self.background_tracker.wait(request_id, dur).await {
                        // Identical shape to check_status so the model reads a
                        // finished agent the same way regardless of how it asked.
                        WaitOutcome::Completed(snap) => {
                            if let CompletedOutcome::Err(err) = &snap.outcome {
                                ToolResult::Error {
                                    error: error_with_trail(err, &snap.progress_tail),
                                    retryable: false,
                                }
                            } else {
                                ToolResult::Success {
                                    output: completed_to_json(request_id, "completed", &snap),
                                }
                            }
                        }
                        WaitOutcome::TimedOut { elapsed_secs } => ToolResult::Success {
                            output: json!({
                                "status": "still_running",
                                "request_id": request_id,
                                "elapsed_secs": elapsed_secs,
                                "waited_secs": timeout_secs,
                                "note": "Sub-agent still running when the wait window elapsed. Call 'wait' again with this request_id to keep blocking, or do other work and check back — its completion is also announced to you.",
                            }),
                        },
                        WaitOutcome::NotFound => ToolResult::Error {
                            error: format!(
                                "No background sub-agent found with request_id '{request_id}'"
                            ),
                            retryable: false,
                        },
                    };
                }

                // Many ids → wait for whichever finishes first (fan-out
                // first-completion). A failed first-completion is returned as a
                // Success carrying the `failed` report (not a ToolResult::Error)
                // so it does not trip the harness failure counter — the model
                // sees which child failed and can wait for the rest.
                return match self.background_tracker.wait_any(&request_ids, dur).await {
                    WaitAnyOutcome::Completed {
                        request_id,
                        snapshot,
                    } => ToolResult::Success {
                        output: completed_to_json(&request_id, "completed", &snapshot),
                    },
                    WaitAnyOutcome::TimedOut { still_running } => ToolResult::Success {
                        output: json!({
                            "status": "still_running",
                            "still_running": still_running,
                            "waited_secs": timeout_secs,
                            "note": "No sub-agent in the set finished within the wait window. Call 'wait' again with these request_ids to keep blocking, or do other work — completions are also announced to you.",
                        }),
                    },
                    WaitAnyOutcome::NotFound => ToolResult::Error {
                        error:
                            "None of the given request_ids matches a known background sub-agent \
                             (all unknown or expired)"
                                .to_string(),
                        retryable: false,
                    },
                };
            }
            SubagentAction::Cancel(request_id) => {
                let hit = self.background_tracker.cancel(&request_id);
                if hit {
                    return ToolResult::Success {
                        output: json!({
                            "status": "cancelling",
                            "request_id": request_id,
                            "note": "CancellationToken fired; running task will exit at its next await point.",
                        }),
                    };
                }
                // No running entry — surface the completed outcome (if any)
                // so the LLM gets a deterministic answer rather than a
                // misleading "not found". Non-destructive: a later
                // check_status still sees the same result.
                return match self.background_tracker.result_snapshot(&request_id) {
                    Some(snap) => {
                        if let CompletedOutcome::Err(err) = &snap.outcome {
                            return ToolResult::Error {
                                error: format!(
                                    "Sub-agent '{request_id}' already failed before cancel: {err}"
                                ),
                                retryable: false,
                            };
                        }
                        ToolResult::Success {
                            output: completed_to_json(&request_id, "already_completed", &snap),
                        }
                    }
                    None => ToolResult::Error {
                        error: format!(
                            "No running or completed sub-agent found with request_id '{request_id}'"
                        ),
                        retryable: false,
                    },
                };
            }
            SubagentAction::List => {
                let running: Vec<Value> = self
                    .background_tracker
                    .list_running()
                    .into_iter()
                    .map(|(id, task, elapsed_secs)| {
                        json!({
                            "request_id": id,
                            "task": task,
                            "elapsed_secs": elapsed_secs,
                        })
                    })
                    .collect();
                let completed: Vec<Value> = self
                    .background_tracker
                    .all_completed()
                    .iter()
                    .map(|(id, snap)| completed_to_json(id, "completed", snap))
                    .collect();
                return ToolResult::Success {
                    output: json!({
                        "running": running,
                        "running_count": running.len(),
                        "completed": completed,
                        "completed_count": completed.len(),
                    }),
                };
            }
            SubagentAction::Run(run_args) => run_args,
        };

        // MoA shorthand: `proposer_models` replicates the top-level `task`
        // across models as parallel proposers (same prompt, different model —
        // the classic Mixture-of-Agents shape). Explicit `batch_tasks` always
        // wins; proposer_models is only the convenience expansion.
        let effective_batch: Option<Vec<BatchTask>> = match args.batch_tasks {
            Some(ref b) if !b.is_empty() => Some(b.clone()),
            _ => args.proposer_models.as_ref().map(|models| {
                models
                    .iter()
                    .map(|m| BatchTask {
                        task: args.task.clone(),
                        agent_type: args.agent_type.clone(),
                        model: Some(m.clone()),
                        timeout_secs: None,
                    })
                    .collect()
            }),
        };

        if let Some(ref batch) = effective_batch {
            if !batch.is_empty() {
                let child_chain = match self.chain.child() {
                    Some(c) => c,
                    None => {
                        return ToolResult::Error {
                            error: format!(
                                "Maximum subagent nesting depth ({}) exceeded",
                                self.chain.max_depth
                            ),
                            retryable: false,
                        };
                    }
                };

                // Resolve agent_def + per-task overrides up-front so we can
                // share a single error path between the sync/async branches.
                // Per-run project overlay (R3 close): when this tool call is
                // scoped to a project root, `<project>/.aleph/agents/{id}.md`
                // shadows the globally registered agent. Looked up once per
                // batch so all rows share the same overlay snapshot.
                let project_root = crate::projects::current_project_root();
                let project_root_ref = project_root.as_deref();
                let mut prepared: Vec<(AgentDef, String, Option<String>, u64)> =
                    Vec::with_capacity(batch.len());
                for (idx, batch_task) in batch.iter().enumerate() {
                    let agent_def = if let Some(ref agent_type) = batch_task.agent_type {
                        match self.agent_registry.resolve(agent_type, project_root_ref) {
                            Some(def) => def,
                            None => {
                                let available = self.agent_registry.list_ids().join(", ");
                                return ToolResult::Error {
                                    error: format!(
                                        "batch task {idx}: Unknown agent_type '{agent_type}'. Available agents: {available}"
                                    ),
                                    retryable: false,
                                };
                            }
                        }
                    } else if let Some(ref agent_type) = args.agent_type {
                        match self.agent_registry.resolve(agent_type, project_root_ref) {
                            Some(def) => def,
                            None => {
                                let available = self.agent_registry.list_ids().join(", ");
                                return ToolResult::Error {
                                    error: format!(
                                        "batch task {idx}: Unknown agent_type '{agent_type}'. Available agents: {available}"
                                    ),
                                    retryable: false,
                                };
                            }
                        }
                    } else {
                        match self
                            .agent_registry
                            .lookup_with_overlay("default", project_root_ref)
                        {
                            Some(def) => def,
                            None => {
                                return ToolResult::Error {
                                    error: "No default agent registered in AgentRegistry"
                                        .to_string(),
                                    retryable: false,
                                };
                            }
                        }
                    };
                    let model = batch_task.model.clone().or_else(|| args.model.clone());
                    let timeout = batch_task.timeout_secs.unwrap_or(args.timeout_secs);
                    prepared.push((agent_def, batch_task.task.clone(), model, timeout));
                }

                if args.run_in_background {
                    // Async batch — spawn and return request_ids for later polling.
                    let mut request_ids = Vec::with_capacity(prepared.len());
                    for (agent_def, task, model, timeout) in prepared {
                        let rid = self.spawn_background(
                            agent_def,
                            task,
                            args.context_summary.clone(),
                            model,
                            timeout,
                            child_chain.clone(),
                            &cancel,
                        );
                        request_ids.push(rid);
                    }
                    return ToolResult::Success {
                        output: json!({
                            "status": "batch_running_in_background",
                            "request_ids": request_ids,
                            "count": request_ids.len(),
                            "message": format!(
                                "{} sub-agents started in background. Completions will be announced to you; check_status with a request_id retrieves a result on demand.",
                                request_ids.len()
                            )
                        }),
                    };
                }

                // Sync batch — fan out in parallel, await all, return aggregate.
                tracing::info!(
                    count = prepared.len(),
                    "subagent: starting batch (sync parallel)"
                );
                // Captured before `prepared` is consumed so the MoA aggregator
                // can label each proposal with the model that produced it.
                let proposer_models_by_idx: Vec<Option<String>> =
                    prepared.iter().map(|(_, _, m, _)| m.clone()).collect();
                let mut handles = Vec::with_capacity(prepared.len());
                for (idx, (agent_def, task, model, timeout)) in prepared.into_iter().enumerate() {
                    let runtime_config = AgentRuntimeConfig {
                        agent_def,
                        task,
                        context_summary: args.context_summary.clone(),
                        model,
                        timeout_secs: timeout,
                        strategy: None,
                    };

                    let batch_cancel = self.cancel_for_child_with(&cancel);
                    let runtime = self.build_runtime(child_chain.clone(), batch_cancel.clone());
                    handles.push(tokio::spawn(async move {
                        let _cancel_guard = CancelGuard::new(batch_cancel.clone());
                        let outcome = AssertUnwindSafe(runtime.run(runtime_config))
                            .catch_unwind()
                            .await;
                        // Terminate this proposal's cancel-bridge watcher.
                        batch_cancel.cancel();
                        (idx, outcome)
                    }));
                }

                let mut results: Vec<Value> = Vec::with_capacity(handles.len());
                // Successful proposals (index, model, text) folded by the MoA
                // aggregator when `synthesize` is set.
                let mut proposals: Vec<(usize, Option<String>, String)> = Vec::new();
                for (batch_idx, h) in handles.into_iter().enumerate() {
                    let item = match h.await {
                        Ok((idx, Ok(Ok(r)))) => {
                            let text = r.final_text.unwrap_or_else(|| "(no output)".to_string());
                            proposals.push((
                                idx,
                                proposer_models_by_idx.get(idx).cloned().flatten(),
                                text.clone(),
                            ));
                            json!({
                                "index": idx,
                                "status": "completed",
                                "result": text,
                                "iterations": r.iterations,
                                "tool_calls_made": r.tool_calls_made,
                                "total_tokens": r.total_tokens,
                            })
                        }
                        Ok((idx, Ok(Err(e)))) => json!({
                            "index": idx,
                            "status": "failed",
                            "error": e,
                        }),
                        Ok((idx, Err(_panic))) => json!({
                            "index": idx,
                            "status": "panicked",
                            "error": "sub-agent panicked",
                        }),
                        Err(join_err) => json!({
                            "index": batch_idx,
                            "status": "join_error",
                            "error": format!("Failed to join task {}: {}", batch_idx, join_err),
                        }),
                    };
                    results.push(item);
                }

                // Mixture-of-Agents reduce: fold the proposals into one answer
                // via a single aggregator sub-agent. The synthesis is performed
                // by an LLM (R7/R9 — intelligence lives in the model + prompt);
                // this tool only fans out and concatenates (R10 — harness stays
                // dumb). Skipped when no proposal succeeded — there is nothing
                // to fold, so the raw batch is returned untouched.
                if args.synthesize && !proposals.is_empty() {
                    let aggregator_def = if let Some(ref agent_type) = args.agent_type {
                        match self.agent_registry.resolve(agent_type, project_root_ref) {
                            Some(def) => def,
                            None => {
                                let available = self.agent_registry.list_ids().join(", ");
                                return ToolResult::Error {
                                    error: format!(
                                        "aggregator: Unknown agent_type '{agent_type}'. Available agents: {available}"
                                    ),
                                    retryable: false,
                                };
                            }
                        }
                    } else {
                        match self
                            .agent_registry
                            .lookup_with_overlay("default", project_root_ref)
                        {
                            Some(def) => def,
                            None => {
                                return ToolResult::Error {
                                    error:
                                        "aggregator: No default agent registered in AgentRegistry"
                                            .to_string(),
                                    retryable: false,
                                };
                            }
                        }
                    };

                    let goal = if args.task.trim().is_empty() {
                        "(see the individual proposal tasks below)"
                    } else {
                        args.task.as_str()
                    };
                    let synthesis_prompt = build_synthesis_prompt(
                        goal,
                        args.synthesis_instruction.as_deref(),
                        &proposals,
                    );
                    let proposer_count = proposals.len();

                    tracing::info!(
                        proposers = proposer_count,
                        "subagent: running MoA aggregator over proposals"
                    );

                    let runtime_config = AgentRuntimeConfig {
                        agent_def: aggregator_def,
                        task: synthesis_prompt,
                        context_summary: args.context_summary.clone(),
                        model: args.aggregator_model.clone().or_else(|| args.model.clone()),
                        timeout_secs: args.timeout_secs,
                        strategy: None,
                    };
                    let agg_cancel = self.cancel_for_child_with(&cancel);
                    let _agg_cancel_guard = CancelGuard::new(agg_cancel.clone());
                    let runtime = self.build_runtime(child_chain.clone(), agg_cancel.clone());

                    let agg_outcome = runtime.run(runtime_config).await;
                    agg_cancel.cancel();
                    return match agg_outcome {
                        Ok(r) => ToolResult::Success {
                            output: json!({
                                "status": "moa_completed",
                                "synthesis": r.final_text.unwrap_or_else(|| "(no output)".to_string()),
                                "proposer_count": proposer_count,
                                "results": results,
                            }),
                        },
                        // Synthesis failed — never discard the proposals; return
                        // them so the parent can fold them itself.
                        Err(e) => ToolResult::Success {
                            output: json!({
                                "status": "moa_synthesis_failed",
                                "error": e,
                                "proposer_count": proposer_count,
                                "results": results,
                            }),
                        },
                    };
                }

                return ToolResult::Success {
                    output: json!({
                        "status": "batch_completed",
                        "count": results.len(),
                        "results": results,
                    }),
                };
            }
        }

        tracing::info!(
            task = %args.task,
            agent_type = ?args.agent_type,
            timeout_secs = args.timeout_secs,
            background = args.run_in_background,
            "subagent: starting sub-task"
        );

        // 2. Resolve agent definition (per-run project overlay first).
        let project_root = crate::projects::current_project_root();
        let project_root_ref = project_root.as_deref();
        let agent_def = if let Some(ref agent_type) = args.agent_type {
            match self.agent_registry.resolve(agent_type, project_root_ref) {
                Some(def) => def,
                None => {
                    let available = self.agent_registry.list_ids().join(", ");
                    return ToolResult::Error {
                        error: format!(
                            "Unknown agent_type '{agent_type}'. Available agents: {available}"
                        ),
                        retryable: false,
                    };
                }
            }
        } else {
            match self
                .agent_registry
                .lookup_with_overlay("default", project_root_ref)
            {
                Some(def) => def,
                None => {
                    return ToolResult::Error {
                        error: "No default agent registered in AgentRegistry".to_string(),
                        retryable: false,
                    };
                }
            }
        };

        // 3. Check nesting depth
        let child_chain = match self.chain.child() {
            Some(c) => c,
            None => {
                return ToolResult::Error {
                    error: format!(
                        "Maximum subagent nesting depth ({}) exceeded",
                        self.chain.max_depth
                    ),
                    retryable: false,
                };
            }
        };

        // 4. Teammate registration (when name + team_name are both provided)
        if let (Some(ref name), Some(ref tname)) = (&args.name, &args.team_name) {
            if let Some(ref mgr) = self.teammate_manager {
                match mgr.ensure_team(tname, &self.parent_agent_id).await {
                    Ok(tid) => {
                        if let Err(e) = mgr.register_teammate(&tid, name, "worker").await {
                            return ToolResult::Error {
                                error: format!("Failed to register teammate '{name}': {e}"),
                                retryable: true,
                            };
                        }
                    }
                    Err(e) => {
                        return ToolResult::Error {
                            error: format!("Failed to create team '{tname}': {e}"),
                            retryable: false,
                        };
                    }
                }
            } else {
                tracing::warn!(
                    name = %name,
                    team = %tname,
                    "subagent: teammate_manager not configured, skipping team registration"
                );
            }
        }

        // 5. Foreground vs background execution
        if args.run_in_background {
            let request_id = self.spawn_background(
                agent_def,
                args.task.clone(),
                args.context_summary,
                args.model,
                args.timeout_secs,
                child_chain,
                &cancel,
            );

            ToolResult::Success {
                output: json!({
                    "status": "running_in_background",
                    "request_id": request_id,
                    "message": format!("Sub-agent started in background. Its completion will be announced to you; request_id '{}' checks status on demand.", request_id)
                }),
            }
        } else {
            // Foreground execution
            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task: args.task.clone(),
                context_summary: args.context_summary,
                model: args.model,
                timeout_secs: args.timeout_secs,
                strategy: None,
            };

            let child_cancel = self.cancel_for_child_with(&cancel);
            let _child_cancel_guard = CancelGuard::new(child_cancel.clone());
            let runtime = self.build_runtime(child_chain, child_cancel.clone());

            let run_outcome = runtime.run(runtime_config).await;
            // Fire the child token so the cancel-bridge watcher exits now the
            // run is done (no-op if it already propagated a cancel).
            child_cancel.cancel();
            match run_outcome {
                Ok(result) => {
                    tracing::info!(
                        iterations = result.iterations,
                        tool_calls = result.tool_calls_made,
                        tokens = result.total_tokens,
                        "subagent: sub-task completed"
                    );

                    ToolResult::Success {
                        output: json!({
                            "result": result.final_text.unwrap_or_else(|| "(no output)".to_string()),
                            "iterations": result.iterations,
                            "tool_calls_made": result.tool_calls_made,
                            "total_tokens": result.total_tokens
                        }),
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "subagent: sub-task failed");
                    ToolResult::Error {
                        error: e,
                        retryable: false,
                    }
                }
            }
        }
    }
}

/// Derive a compact activity summary from a running sub-agent's progress
/// window. `steps` is the highest iteration index observed (monotonic, so
/// it stays accurate even though the window is FIFO-capped at 50); the
/// `last_*` fields reflect the most recent event.
fn summarize_progress(progress: &[SubagentProgress]) -> Value {
    let steps = progress.iter().map(|p| p.step).max().unwrap_or(0);
    let last = progress.last();
    json!({
        "steps": steps,
        "last_activity": last.map(|p| p.kind),
        "last_tool": last.and_then(|p| p.tool_name.clone()),
    })
}

/// B18 — fold a failed background sub-agent's trajectory into the error string.
///
/// `ToolResult::Error` carries no structured payload, and it must stay that way
/// (the variant is exhaustively destructured inside the harness, which is over
/// its line budget), so the trail rides the message itself. This is error
/// *compaction*, not error recovery: the parent model sees what the dead child
/// actually tried and can retry differently instead of re-deriving it blind. The
/// shape stays `Error` on purpose — the harness's consecutive-failure counter
/// depends on a failure reading as a failure.
///
/// Bounded by construction: the tracker only retains `PROGRESS_TAIL_LEN` events,
/// and this renders at most `TRAIL_LINES` of them.
fn error_with_trail(err: &str, progress: &[SubagentProgress]) -> String {
    const TRAIL_LINES: usize = 5;
    let head = format!("Background sub-agent failed: {err}");
    if progress.is_empty() {
        return head;
    }
    let steps = progress.iter().map(|p| p.step).max().unwrap_or(0);
    let start = progress.len().saturating_sub(TRAIL_LINES);
    let mut out = format!("{head}\nTrajectory before failure ({steps} steps):");
    for p in progress
        .get(start..)
        .expect("invariant: start is within progress bounds")
    {
        let activity = crate::agents::background_tracker::progress_activity(p.kind);
        match &p.tool_name {
            Some(tool) => out.push_str(&format!("\n  step {}: {activity} {tool}", p.step)),
            None => out.push_str(&format!("\n  step {}: {activity}", p.step)),
        }
    }
    out
}

/// Build the Mixture-of-Agents aggregator prompt: the shared goal, every
/// successful proposal labelled with the model that produced it, and an
/// instruction to synthesize a single best answer. Mirrors the MoA paper
/// (Wang et al. 2406.04692) — the aggregator critiques and merges rather than
/// picking one winner. All reasoning happens in the aggregator model (R7/R9);
/// this function only assembles text.
fn build_synthesis_prompt(
    goal: &str,
    extra_instruction: Option<&str>,
    proposals: &[(usize, Option<String>, String)],
) -> String {
    let mut out = String::new();
    out.push_str(
        "You are the aggregator in a Mixture-of-Agents pipeline. Several agents \
         independently produced candidate responses to the same goal. Synthesize \
         them into a single, higher-quality answer: merge their strongest points, \
         reconcile contradictions in favour of the best-supported claim, drop \
         errors, and do not simply pick one response verbatim.\n\n",
    );
    out.push_str("## Goal\n");
    out.push_str(goal);
    out.push_str("\n\n## Candidate responses\n");
    for (idx, model, text) in proposals {
        match model {
            Some(m) => out.push_str(&format!("\n### Proposal {idx} (model: {m})\n")),
            None => out.push_str(&format!("\n### Proposal {idx}\n")),
        }
        out.push_str(text);
        out.push('\n');
    }
    if let Some(extra) = extra_instruction {
        if !extra.trim().is_empty() {
            out.push_str("\n## Additional synthesis guidance\n");
            out.push_str(extra);
            out.push('\n');
        }
    }
    out.push_str("\n## Your task\nReturn the single synthesized answer to the goal.");
    out
}

/// Render a finished background sub-agent as a JSON object. `ok_status` is
/// the `status` string for a success (`completed` / `already_completed`);
/// a failure always reports `failed`. Shared by the `check_status`,
/// `cancel`, and `list` actions so a finished agent reports identically
/// everywhere — at parity with the foreground spawn path's
/// `{result, iterations, tool_calls_made}` response shape.
///
/// B18 — both arms carry the retained progress tail (`progress` + `summary`),
/// the same pair a *running* agent reports, so `list` no longer goes blind the
/// moment a child finishes.
fn completed_to_json(request_id: &str, ok_status: &str, snap: &CompletedSnapshot) -> Value {
    let progress = &snap.progress_tail;
    match &snap.outcome {
        CompletedOutcome::Ok {
            final_text,
            iterations,
            tool_calls_made,
            total_tokens,
        } => json!({
            "status": ok_status,
            "request_id": request_id,
            "task": snap.task,
            "result": final_text,
            "iterations": iterations,
            "tool_calls_made": tool_calls_made,
            "total_tokens": total_tokens,
            "duration_secs": snap.duration_secs,
            "summary": summarize_progress(progress),
            "progress": progress,
        }),
        CompletedOutcome::Err(err) => json!({
            "status": "failed",
            "request_id": request_id,
            "task": snap.task,
            "error": err,
            "duration_secs": snap.duration_secs,
            "summary": summarize_progress(progress),
            "progress": progress,
        }),
    }
}
