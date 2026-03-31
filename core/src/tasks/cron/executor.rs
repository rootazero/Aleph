//! Production cron job executor.
//!
//! Bridges `JobSnapshot` → `ExecutionAdapter` + `AgentRegistry` → `ExecutionResult`.

use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::tasks::cron::config::{
    DeliveryStatus, ErrorReason, ExecutionResult, JobSnapshot, RunStatus, SessionTarget,
};
use crate::tasks::cron::service::timer::JobExecutorFn;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::channel::OutboundMessage;
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::event_emitter::CollectingEventEmitter;
use crate::gateway::event_emitter::StreamEvent;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::gateway::router::SessionKey;
use crate::sync_primitives::Arc;

/// Deferred channel registry reference — set after channels are initialized.
pub type ChannelRegistryCell = Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>>;

/// Build a `JobExecutorFn` closure that captures execution dependencies.
pub fn build_cron_executor_fn(
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    channel_registry_cell: ChannelRegistryCell,
) -> JobExecutorFn {
    Arc::new(move |snapshot: JobSnapshot| {
        let adapter = Arc::clone(&execution_adapter);
        let registry = Arc::clone(&agent_registry);
        let ch_cell = Arc::clone(&channel_registry_cell);
        Box::pin(async move { execute_cron_job(adapter, registry, ch_cell, snapshot).await })
    })
}

async fn execute_cron_job(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    channel_registry_cell: ChannelRegistryCell,
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
    if let Some(ref conv_id) = snapshot.source_conversation_id {
        metadata.insert("conversation_id".to_string(), conv_id.clone());
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

    let collector = Arc::new(CollectingEventEmitter::new());
    let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
        Arc::clone(&collector) as _;

    info!(
        job_id = %snapshot.id,
        agent_id,
        trigger = snapshot.trigger_source.as_str(),
        "executing cron job"
    );

    // Capture delivery targets before moving snapshot fields
    let deliver_channel = snapshot.source_channel_id.clone();
    let deliver_conversation = snapshot.source_conversation_id.clone();

    match adapter.execute(request, agent, emitter).await {
        Ok(()) => {
            let ended_at = chrono::Utc::now().timestamp_millis();

            // Extract final response from collected events
            let final_response = extract_final_response(&collector).await;

            // Deliver response to source channel if available
            let delivery_status = if let (Some(ref ch_id), Some(ref conv_id)) =
                (&deliver_channel, &deliver_conversation)
            {
                if let Some(ref response_text) = final_response {
                    deliver_to_channel(
                        &channel_registry_cell,
                        ch_id,
                        conv_id,
                        response_text,
                        &snapshot.id,
                    )
                    .await
                } else {
                    info!(job_id = %snapshot.id, "cron job produced no response text, skipping delivery");
                    DeliveryStatus::NotDelivered
                }
            } else {
                DeliveryStatus::NotDelivered
            };

            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at - started_at,
                status: RunStatus::Ok,
                output: final_response,
                error: None,
                error_reason: None,
                delivery_status: Some(delivery_status),
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

    if snapshot.source_channel_id.is_some() {
        parts.push(
            "You are executing a scheduled task. \
             Just produce the result directly — the system will automatically deliver your response \
             to the user who created this task. Do NOT try to use a message tool for delivery."
                .to_string(),
        );
    }

    parts.push(String::new()); // blank line separator
    parts.push(snapshot.prompt.clone());

    parts.join("\n")
}

/// Extract the final response text from collected events.
async fn extract_final_response(collector: &CollectingEventEmitter) -> Option<String> {
    let events = collector.events().await;

    // First try: find RunComplete with final_response in summary
    for event in events.iter().rev() {
        if let StreamEvent::RunComplete { ref summary, .. } = event {
            if let Some(ref text) = summary.final_response {
                if !text.is_empty() {
                    return Some(text.clone());
                }
            }
        }
    }

    // Fallback: concatenate all ResponseChunk deltas
    let mut full_text = String::new();
    for event in &events {
        if let StreamEvent::ResponseChunk { ref delta, .. } = event {
            full_text.push_str(delta);
        }
    }
    if full_text.is_empty() {
        None
    } else {
        Some(full_text)
    }
}

/// Deliver the cron job response to the source channel via ChannelRegistry.
async fn deliver_to_channel(
    cell: &ChannelRegistryCell,
    channel_id: &str,
    conversation_id: &str,
    text: &str,
    job_id: &str,
) -> DeliveryStatus {
    let registry = match cell.get() {
        Some(r) => r,
        None => {
            warn!(job_id, "cron delivery skipped: ChannelRegistry not yet initialized");
            return DeliveryStatus::NotDelivered;
        }
    };

    let ch_id = crate::gateway::channel::ChannelId::new(channel_id);
    let message = OutboundMessage::text(conversation_id.to_string(), text.to_string());

    match registry.send(&ch_id, message).await {
        Ok(_) => {
            info!(job_id, channel_id, conversation_id, "cron job result delivered");
            DeliveryStatus::Delivered
        }
        Err(e) => {
            error!(job_id, channel_id, conversation_id, error = %e, "cron delivery failed");
            DeliveryStatus::NotDelivered
        }
    }
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
    use crate::tasks::cron::config::{SessionTarget, TriggerSource};

    fn make_test_snapshot() -> JobSnapshot {
        JobSnapshot {
            id: "test-job-1".to_string(),
            agent_id: Some("main".to_string()),
            source_channel_id: Some("discord:general".to_string()),
            source_conversation_id: Some("123456".to_string()),
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
