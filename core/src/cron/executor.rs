//! Production cron job executor.
//!
//! Bridges `JobSnapshot` → `ExecutionAdapter` + `AgentRegistry` → `ExecutionResult`.

use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::cron::config::{
    DeliveryStatus, ErrorReason, ExecutionResult, JobSnapshot, RunStatus, SessionTarget,
};
use crate::cron::service::timer::JobExecutorFn;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::gateway::router::SessionKey;
use crate::sync_primitives::Arc;

/// Build a `JobExecutorFn` closure that captures execution dependencies.
pub fn build_cron_executor_fn(
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
) -> JobExecutorFn {
    Arc::new(move |snapshot: JobSnapshot| {
        let adapter = Arc::clone(&execution_adapter);
        let registry = Arc::clone(&agent_registry);
        Box::pin(async move { execute_cron_job(adapter, registry, snapshot).await })
    })
}

async fn execute_cron_job(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    snapshot: JobSnapshot,
) -> ExecutionResult {
    let started_at = chrono::Utc::now().timestamp_millis();

    // Resolve agent_id, defaulting to "main"
    let agent_id = snapshot.agent_id.as_deref().unwrap_or("main");

    // Look up agent in registry
    let agent = match registry.get(agent_id).await {
        Some(a) => a,
        None => {
            warn!(job_id = %snapshot.id, agent_id, "cron job agent not found in registry");
            return make_error_result(
                started_at,
                format!("agent not found: {agent_id}"),
                ErrorReason::Permanent(format!("agent '{agent_id}' is not registered")),
            );
        }
    };

    // Build task_id: Main sessions share by job_id, Isolated sessions get a unique suffix
    let task_id = match snapshot.session_target {
        SessionTarget::Main => snapshot.id.clone(),
        SessionTarget::Isolated => format!("{}-{}", snapshot.id, started_at),
    };

    let session_key = SessionKey::task(agent_id, "cron", &task_id);

    // Build prompt with cron context injected
    let prompt = build_cron_prompt(&snapshot);

    // Build metadata for traceability
    let mut metadata = HashMap::new();
    metadata.insert("cron_job_id".to_string(), snapshot.id.clone());
    metadata.insert(
        "trigger_source".to_string(),
        snapshot.trigger_source.as_str().to_string(),
    );
    if let Some(ref ch) = snapshot.source_channel_id {
        metadata.insert("source_channel_id".to_string(), ch.clone());
        // Also set channel_id so ExecutionEngine populates SessionContext.channel,
        // which downstream tools (message, agent management) rely on.
        metadata.insert("channel_id".to_string(), ch.clone());
    }

    let timeout_secs = snapshot
        .timeout_ms
        .map(|ms| (ms / 1000).max(1) as u64);

    let request = RunRequest {
        run_id: Uuid::new_v4().to_string(),
        input: prompt,
        session_key,
        timeout_secs,
        metadata,
        attachments: Vec::new(),
        pending_media: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
        Arc::new(NoOpEventEmitter::new());

    info!(
        job_id = %snapshot.id,
        agent_id,
        trigger = snapshot.trigger_source.as_str(),
        "executing cron job"
    );

    match adapter.execute(request, agent, emitter).await {
        Ok(()) => {
            let ended_at = chrono::Utc::now().timestamp_millis();
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at - started_at,
                status: RunStatus::Ok,
                output: None,
                error: None,
                error_reason: None,
                delivery_status: Some(DeliveryStatus::NotDelivered),
                agent_used_messaging_tool: false,
            }
        }
        Err(ExecutionError::Timeout) => {
            error!(job_id = %snapshot.id, "cron job timed out");
            let ended_at = chrono::Utc::now().timestamp_millis();
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at - started_at,
                status: RunStatus::Timeout,
                output: None,
                error: Some("job execution timed out".to_string()),
                error_reason: Some(ErrorReason::Transient("timeout".to_string())),
                delivery_status: None,
                agent_used_messaging_tool: false,
            }
        }
        Err(ExecutionError::AgentBusy(msg)) => {
            warn!(job_id = %snapshot.id, %msg, "cron job skipped: agent busy");
            let ended_at = chrono::Utc::now().timestamp_millis();
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at - started_at,
                status: RunStatus::Skipped,
                output: None,
                error: Some(format!("agent busy: {msg}")),
                error_reason: Some(ErrorReason::Transient(msg)),
                delivery_status: None,
                agent_used_messaging_tool: false,
            }
        }
        Err(e) => {
            error!(job_id = %snapshot.id, error = %e, "cron job failed");
            make_error_result(
                started_at,
                e.to_string(),
                ErrorReason::Transient(e.to_string()),
            )
        }
    }
}

/// Build the final prompt string, injecting cron context header.
fn build_cron_prompt(snapshot: &JobSnapshot) -> String {
    let mut parts = Vec::new();

    parts.push(format!("[Cron Task: {}]", snapshot.id));

    if let Some(ref channel_id) = snapshot.source_channel_id {
        parts.push(format!(
            "You are executing a scheduled task. After completing the task, send the results to channel '{}' using the message tool.",
            channel_id
        ));
    }

    parts.push(String::new()); // blank line separator
    parts.push(snapshot.prompt.clone());

    parts.join("\n")
}

/// Build an error `ExecutionResult`.
fn make_error_result(started_at: i64, error: String, reason: ErrorReason) -> ExecutionResult {
    let ended_at = chrono::Utc::now().timestamp_millis();
    ExecutionResult {
        started_at,
        ended_at,
        duration_ms: ended_at - started_at,
        status: RunStatus::Error,
        output: None,
        error: Some(error),
        error_reason: Some(reason),
        delivery_status: None,
        agent_used_messaging_tool: false,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::{SessionTarget, TriggerSource};

    fn make_test_snapshot() -> JobSnapshot {
        JobSnapshot {
            id: "test-job-1".to_string(),
            agent_id: Some("main".to_string()),
            source_channel_id: Some("discord:general".to_string()),
            prompt: "Check the weather".to_string(),
            model: None,
            timeout_ms: Some(300_000),
            delivery: None,
            session_target: SessionTarget::Isolated,
            marked_at: 1_000_000,
            trigger_source: TriggerSource::Schedule,
        }
    }

    #[test]
    fn test_build_cron_prompt_with_channel() {
        let snapshot = make_test_snapshot();
        let prompt = build_cron_prompt(&snapshot);
        assert!(prompt.contains("[Cron Task: test-job-1]"));
        assert!(prompt.contains("discord:general"));
        assert!(prompt.contains("message tool"));
        assert!(prompt.contains("Check the weather"));
    }

    #[test]
    fn test_build_cron_prompt_without_channel() {
        let mut snapshot = make_test_snapshot();
        snapshot.source_channel_id = None;
        let prompt = build_cron_prompt(&snapshot);
        assert!(prompt.contains("[Cron Task: test-job-1]"));
        assert!(!prompt.contains("message tool"));
        assert!(prompt.contains("Check the weather"));
    }
}
