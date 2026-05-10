//! LoopTool trait implementation for SubagentTool.

use async_trait::async_trait;
use futures::FutureExt;
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;
use tokio_util::sync::CancellationToken;

use crate::agents::runtime::{AgentRuntime, AgentRuntimeConfig};
use crate::agents::AgentDef;
use crate::sync_primitives::Arc;
use crate::teams::messages::router::SendRequest;
use crate::teams::messages::types::MessageType;
use crate::tools::runtime::{LoopTool, ToolResult};

use super::{parse_args, SubagentAction, SubagentTool};

#[async_trait]
impl LoopTool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Delegate tasks to autonomous sub-agents. For simple single tasks, use 'task'. \
         For complex goals that can be broken into independent sub-tasks, use 'batch_tasks' \
         to launch multiple sub-agents in parallel — the system automatically runs them \
         in background and returns request_ids for status polling."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run", "check_status", "send_message", "read_inbox"],
                    "description": "The action to perform. Defaults to 'run' (or 'check_status' if only request_id is provided)."
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
                    "description": "Maximum time in seconds for the sub-agent to run. Default: 120.",
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
                    "description": "Check status of a background sub-agent. Provide request_id without task to retrieve the result."
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

    async fn execute(&self, input: Value) -> ToolResult {
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
                                error: format!("Failed to resolve team '{}': {}", team_name, e),
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
                                error: format!("Failed to resolve team '{}': {}", team_name, e),
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
                // Check running first
                let running = self.background_tracker.list_running();
                if running.iter().any(|(id, _, _)| id == &request_id) {
                    let progress = self.background_tracker.progress_snapshot(&request_id, 10);
                    return ToolResult::Success {
                        output: json!({
                            "status": "running",
                            "request_id": request_id,
                            "progress": progress,
                        }),
                    };
                }
                // Check completed
                match self.background_tracker.take_result(&request_id) {
                    Some(Ok(result)) => {
                        return ToolResult::Success {
                            output: json!({
                                "status": "completed",
                                "request_id": request_id,
                                "result": result,
                            }),
                        };
                    }
                    Some(Err(err)) => {
                        return ToolResult::Error {
                            error: format!("Background sub-agent failed: {}", err),
                            retryable: false,
                        };
                    }
                    None => {
                        return ToolResult::Error {
                            error: format!(
                                "No background sub-agent found with request_id '{}'",
                                request_id
                            ),
                            retryable: false,
                        };
                    }
                }
            }
            SubagentAction::Run(run_args) => run_args,
        };

        if let Some(ref batch) = args.batch_tasks {
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
                let mut prepared: Vec<(AgentDef, String, Option<String>, u64)> =
                    Vec::with_capacity(batch.len());
                for (idx, batch_task) in batch.iter().enumerate() {
                    let agent_def = if let Some(ref agent_type) = batch_task.agent_type {
                        match self.agent_registry.get(agent_type) {
                            Some(def) => def,
                            None => {
                                let available = self.agent_registry.list_ids().join(", ");
                                return ToolResult::Error {
                                    error: format!(
                                        "batch task {}: Unknown agent_type '{}'. Available agents: {}",
                                        idx, agent_type, available
                                    ),
                                    retryable: false,
                                };
                            }
                        }
                    } else if let Some(ref agent_type) = args.agent_type {
                        match self.agent_registry.get(agent_type) {
                            Some(def) => def,
                            None => {
                                let available = self.agent_registry.list_ids().join(", ");
                                return ToolResult::Error {
                                    error: format!(
                                        "batch task {}: Unknown agent_type '{}'. Available agents: {}",
                                        idx, agent_type, available
                                    ),
                                    retryable: false,
                                };
                            }
                        }
                    } else {
                        match self.agent_registry.get("default") {
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
                        );
                        request_ids.push(rid);
                    }
                    return ToolResult::Success {
                        output: json!({
                            "status": "batch_running_in_background",
                            "request_ids": request_ids,
                            "count": request_ids.len(),
                            "message": format!(
                                "{} sub-agents started in background. Use check_status with each request_id to retrieve results.",
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
                let mut handles = Vec::with_capacity(prepared.len());
                for (idx, (agent_def, task, model, timeout)) in prepared.into_iter().enumerate() {
                    let runtime_config = AgentRuntimeConfig {
                        agent_def,
                        task,
                        context_summary: args.context_summary.clone(),
                        model,
                        timeout_secs: timeout,
                    };

                    let provider = self.provider.clone();
                    let session = self.session.clone();
                    let parent_tools = self.parent_tools.clone();
                    let sandbox = self.sandbox.clone();
                    let raw_memory_writer = self.raw_memory_writer.clone();
                    let capture_registry = self.capture_registry.clone();
                    let parent_agent_id = self.parent_agent_id.clone();
                    let parent_session_id = self.parent_session_id.clone();
                    let chain_for_task = child_chain.clone();

                    handles.push(tokio::spawn(async move {
                        let mut runtime = AgentRuntime::new(
                            provider,
                            chain_for_task,
                            CancellationToken::new(),
                            session,
                            parent_tools,
                            sandbox,
                        )
                        .with_parent_agent_id(parent_agent_id);
                        if let Some(w) = raw_memory_writer {
                            runtime = runtime.with_raw_memory_writer(w);
                        }
                        if let Some(reg) = capture_registry {
                            runtime = runtime.with_capture_registry(reg);
                        }
                        if let Some(sid) = parent_session_id {
                            runtime = runtime.with_parent_session_id(sid);
                        }
                        let outcome = AssertUnwindSafe(runtime.run(runtime_config))
                            .catch_unwind()
                            .await;
                        (idx, outcome)
                    }));
                }

                let mut results: Vec<Value> = Vec::with_capacity(handles.len());
                for h in handles {
                    let item = match h.await {
                        Ok((idx, Ok(Ok(r)))) => json!({
                            "index": idx,
                            "status": "completed",
                            "result": r.final_text.unwrap_or_else(|| "(no output)".to_string()),
                            "iterations": r.iterations,
                            "tool_calls_made": r.tool_calls_made,
                        }),
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
                            "status": "join_error",
                            "error": join_err.to_string(),
                        }),
                    };
                    results.push(item);
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

        // 2. Resolve agent definition
        let agent_def = if let Some(ref agent_type) = args.agent_type {
            match self.agent_registry.get(agent_type) {
                Some(def) => def,
                None => {
                    let available = self.agent_registry.list_ids().join(", ");
                    return ToolResult::Error {
                        error: format!(
                            "Unknown agent_type '{}'. Available agents: {}",
                            agent_type, available
                        ),
                        retryable: false,
                    };
                }
            }
        } else {
            match self.agent_registry.get("default") {
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
                                error: format!("Failed to register teammate '{}': {}", name, e),
                                retryable: true,
                            };
                        }
                    }
                    Err(e) => {
                        return ToolResult::Error {
                            error: format!("Failed to create team '{}': {}", tname, e),
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
            );

            ToolResult::Success {
                output: json!({
                    "status": "running_in_background",
                    "request_id": request_id,
                    "message": format!("Sub-agent started in background. Use request_id '{}' to check status.", request_id)
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
            };

            let mut runtime = AgentRuntime::new(
                self.provider.clone(),
                child_chain,
                CancellationToken::new(),
                self.session.clone(),
                self.parent_tools.clone(),
                self.sandbox.clone(),
            )
            .with_parent_agent_id(self.parent_agent_id.clone());
            if let Some(w) = self.raw_memory_writer.clone() {
                runtime = runtime.with_raw_memory_writer(w);
            }
            if let Some(reg) = self.capture_registry.clone() {
                runtime = runtime.with_capture_registry(reg);
            }
            if let Some(sid) = self.parent_session_id.clone() {
                runtime = runtime.with_parent_session_id(sid);
            }

            match runtime.run(runtime_config).await {
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
                            "tool_calls_made": result.tool_calls_made
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

