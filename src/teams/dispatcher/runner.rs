//! Member task runner.
//!
//! Executes a single coordination task by launching its owner agent through
//! the execution adapter, bounded by a timeout with abort-on-expiry.
//!
//! Shared by `team_delegate` (synchronous, leader-driven delegation) and the
//! autonomous [`TeamDispatcher`](super::TeamDispatcher).

use std::collections::HashMap;

use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;
use crate::sync_primitives::Arc;

/// Terminal status of a member task run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRunStatus {
    /// The agent session finished cleanly.
    Completed,
    /// Execution failed (agent missing, adapter error, or panic).
    Failed,
    /// The run exceeded its timeout and was aborted.
    Timeout,
}

/// Outcome of running a member task. Never an `Err` — every failure mode is
/// mapped here so callers can record task state uniformly.
#[derive(Debug, Clone)]
pub struct MemberRunOutcome {
    pub status: MemberRunStatus,
    /// The agent's last assistant reply (present only on `Completed`).
    pub reply: Option<String>,
    /// Human-readable error (present on `Failed` / `Timeout`).
    pub error: Option<String>,
}

/// Execute `task_text` as agent `agent_id` (within `team_id`), scoped to a
/// task-specific session, bounded by `timeout_secs`.
///
/// The agent runs the full Orchestrator → Harness path via the execution
/// adapter. On timeout the spawned execution is aborted to free resources.
pub async fn execute_member_task(
    context: &GatewayContext,
    agent_id: &str,
    team_id: &str,
    task_id: &str,
    task_text: String,
    timeout_secs: u64,
) -> MemberRunOutcome {
    // Resolve the target agent up front — an unknown owner is an explicit
    // failure, never a silent no-op.
    let agent_registry = context.agent_registry();
    let target_agent = match agent_registry.get(agent_id).await {
        Some(a) => a,
        None => {
            return MemberRunOutcome {
                status: MemberRunStatus::Failed,
                reply: None,
                error: Some(format!("Agent '{agent_id}' not found in registry")),
            };
        }
    };

    let session_key = SessionKey::task(agent_id, "team", task_id);
    let run_id = uuid::Uuid::new_v4().to_string();
    let metadata = {
        let mut m = HashMap::new();
        m.insert("team_id".to_string(), team_id.to_string());
        m.insert("task_id".to_string(), task_id.to_string());
        m
    };

    let request = RunRequest {
        run_id,
        input: task_text,
        session_key: session_key.clone(),
        timeout_secs: Some(timeout_secs),
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };

    let execution_adapter = Arc::clone(context.execution_adapter());
    let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
        Arc::new(NoOpEventEmitter::new());

    // Spawn the execution so it can be aborted on timeout.
    let agent_for_exec = target_agent.clone();
    let handle = tokio::spawn(async move {
        execution_adapter
            .execute(request, agent_for_exec, emitter)
            .await
    });
    let abort_handle = handle.abort_handle();

    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    match tokio::time::timeout(timeout_duration, handle).await {
        Ok(Ok(Ok(()))) => {
            let reply = fetch_last_reply(&target_agent, &session_key).await;
            MemberRunOutcome {
                status: MemberRunStatus::Completed,
                reply: Some(reply.unwrap_or_else(|| "(no reply content)".to_string())),
                error: None,
            }
        }
        Ok(Ok(Err(e))) => MemberRunOutcome {
            status: MemberRunStatus::Failed,
            reply: None,
            error: Some(format!("Execution failed: {e}")),
        },
        Ok(Err(join_err)) => MemberRunOutcome {
            status: MemberRunStatus::Failed,
            reply: None,
            error: Some(format!("Task panicked: {join_err}")),
        },
        Err(_) => {
            // Timeout — abort the spawned task to free resources.
            abort_handle.abort();
            MemberRunOutcome {
                status: MemberRunStatus::Timeout,
                reply: None,
                error: Some(format!("Timed out after {timeout_secs} seconds")),
            }
        }
    }
}

/// Fetch the last assistant reply from an agent's session.
async fn fetch_last_reply(
    agent: &crate::gateway::agent_instance::AgentInstance,
    session_key: &SessionKey,
) -> Option<String> {
    let history = agent.get_history(session_key, Some(1)).await;
    history
        .last()
        .filter(|msg| {
            matches!(
                msg.role,
                crate::gateway::agent_instance::MessageRole::Assistant
            )
        })
        .map(|msg| msg.content.clone())
}
