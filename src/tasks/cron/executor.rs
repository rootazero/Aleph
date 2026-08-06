//! Production cron job executor.
//!
//! Bridges `JobSnapshot` → `ExecutionAdapter` + `AgentRegistry` → `ExecutionResult`.

use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::gateway::agent_instance::{AgentInstance, AgentRegistry};
use crate::gateway::channel::OutboundMessage;
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::event_emitter::CollectingEventEmitter;
use crate::gateway::event_emitter::StreamEvent;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::gateway::reply_emitter::extract_final_response;
use crate::gateway::router::SessionKey;
use crate::sync_primitives::Arc;
use crate::tasks::cron::config::{
    DeliveryStatus, ErrorReason, ExecutionResult, JobSnapshot, RunStatus, SessionTarget,
    TriggerSource,
};
use crate::tasks::cron::service::concurrency::PendingAlert;
use crate::tasks::cron::service::timer::{AlertDispatcherFn, JobExecutorFn};
use crate::tasks::shared::delivery::{
    DeliveryConfig, DeliveryEngine, DeliveryMode, DeliveryPayload,
};
use crate::tasks::shared::retry_hint::{classify, RetryHint};

/// Deferred channel registry reference — set after channels are initialized.
pub type ChannelRegistryCell = Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>>;

/// Build a `JobExecutorFn` closure that captures execution dependencies.
///
/// `default_max_iterations` is the cron-wide Think→Act cap (see
/// `CronConfig::default_max_iterations`) applied as
/// `RunRequest.max_iterations_override` so cron-driven runs are bounded
/// independently of the global `[execution] max_iterations` (default 1000).
/// `None` defers to the global default — useful for tests or deployments
/// that want no cron-specific tightening.
pub fn build_cron_executor_fn(
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    channel_registry_cell: ChannelRegistryCell,
    default_max_iterations: Option<u32>,
) -> JobExecutorFn {
    Arc::new(move |snapshot: JobSnapshot| {
        let adapter = Arc::clone(&execution_adapter);
        let registry = Arc::clone(&agent_registry);
        let ch_cell = Arc::clone(&channel_registry_cell);
        let max_iter = default_max_iterations;
        Box::pin(
            async move { execute_cron_job(adapter, registry, ch_cell, snapshot, max_iter).await },
        )
    })
}

/// Build the alert dispatcher used by `run_timer_loop` to actually deliver
/// `PendingAlert`s.
///
/// Routes every alert through the shared [`DeliveryEngine`] (the same engine
/// the heartbeat loop uses), so a job's `failure_alert.target` is honoured for
/// **all** target kinds — Gateway, Webhook, and Memory — instead of only
/// Gateway. Previously this hand-rolled a Gateway-only match and silently
/// dropped Webhook/Memory targets even though both `DeliveryTarget`
/// implementations already exist.
#[must_use]
pub fn build_cron_alert_dispatcher_fn(delivery_engine: Arc<DeliveryEngine>) -> AlertDispatcherFn {
    Arc::new(move |alerts: Vec<PendingAlert>| {
        let engine = Arc::clone(&delivery_engine);
        Box::pin(async move {
            for alert in alerts {
                let target = alert.target;
                let output = format!("⚠️ {}: {}", alert.job_name, alert.message);
                let payload = DeliveryPayload {
                    source_type: "cron".to_string(),
                    task_name: alert.job_name,
                    // rust-doctor-disable-next-line unnecessary-allocation
                    // Empty placeholder required by DeliveryPayload; String::new() has no heap allocation.
                    agent_id: String::new(),
                    // Gateway targets render `output` verbatim; keep the ⚠️
                    // prefix the previous dispatcher produced. Webhook targets
                    // additionally receive task_name / metadata as structured
                    // JSON fields.
                    output,
                    channel_id: None,
                    metadata: serde_json::json!({
                        "job_id": alert.job_id,
                        "kind": "failure_alert",
                    }),
                };
                let config = DeliveryConfig {
                    mode: DeliveryMode::Primary,
                    targets: vec![target],
                    fallback_target: None,
                };
                let outcomes = engine.deliver(&payload, &config).await;
                if outcomes.iter().any(|o| !o.success) {
                    warn!(
                        job_id = %alert.job_id,
                        ?outcomes,
                        "cron failure alert delivery reported failures"
                    );
                }
            }
        })
    })
}

/// Resolve which agent instance to run a cron job with. When `requested` is
/// missing from the registry, fall back to the registry's default agent
/// (the built-in "main", which cannot be deleted). Returns the resolved
/// instance, the id it resolved under (for session keying), and whether a
/// fallback occurred. Returns `None` only when even the default is absent
/// (should not happen in production — "main" is built-in).
async fn resolve_cron_agent(
    registry: &AgentRegistry,
    requested: &str,
) -> Option<(Arc<AgentInstance>, String, bool)> {
    if let Some(agent) = registry.get(requested).await {
        return Some((agent, requested.to_string(), false));
    }
    warn!(
        requested,
        "cron agent missing, falling back to default agent"
    );
    let agent = registry.get_default().await?;
    let used = agent.id().to_string();
    Some((agent, used, true))
}

async fn execute_cron_job(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    channel_registry_cell: ChannelRegistryCell,
    snapshot: JobSnapshot,
    max_iterations_override: Option<u32>,
) -> ExecutionResult {
    let started_at = chrono::Utc::now().timestamp_millis();

    // Resolve agent, defaulting to "main" when unset and gracefully falling
    // back to the built-in default when the bound agent was deleted.
    let requested_agent = snapshot.agent_id.as_deref().unwrap_or("main").to_string();
    let (agent, resolved_agent_id, fell_back) =
        match resolve_cron_agent(&registry, &requested_agent).await {
            Some(resolved) => resolved,
            None => {
                warn!(job_id = %snapshot.id, requested = %requested_agent,
                    "cron job: neither requested agent nor default 'main' is registered");
                return make_error_result(
                    started_at,
                    "built-in 'main' agent is not registered".to_string(),
                    ErrorReason::Permanent("built-in 'main' agent is not registered".to_string()),
                    RetryHint::permanent(),
                    snapshot.trigger_source,
                );
            }
        };
    let agent_id = resolved_agent_id.as_str();

    // Build task_id: Main sessions share by job_id, Isolated sessions get a unique suffix
    let task_id = match snapshot.session_target {
        SessionTarget::Main => snapshot.id.clone(),
        SessionTarget::Isolated => format!("{}-{}", snapshot.id, started_at),
    };

    let session_key = SessionKey::task(agent_id, "cron", &task_id);

    // Build prompt with cron context injected
    let prompt = build_cron_prompt(&snapshot);

    let metadata = build_cron_metadata(&snapshot);

    let timeout_secs = snapshot.timeout_ms.map(|ms| (ms / 1000).max(1) as u64);

    // System-initiated: cron has no parent run, so there is no project
    // context to inherit. Round-3 follow-up: add an optional
    // `project_root` field to the job snapshot so scheduled jobs can be
    // bound to a specific project folder.
    let request = RunRequest {
        run_id: Uuid::new_v4().to_string(),
        input: prompt,
        session_key,
        timeout_secs,
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override,
        model_override: None,
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

            // Extract final response from collected events (shared with the
            // group-chat broadcaster via `reply_emitter::extract_final_response`).
            let final_response = extract_final_response(&collector.events().await);

            // P3: when the harness reports
            // `BudgetExhaustedPartialResult`, persist the partial text
            // to the carry-over file so the next firing of this job
            // resumes from where we left off. We use the stable label
            // exposed via `RunSummary.terminate_reason` rather than the
            // typed enum because cron only sees the gateway-side wire
            // form. The label is single-source from
            // `TerminateReason::as_static_str()`.
            let mut wrote_carryover = false;
            if let Some((label, detail)) = extract_terminate_reason(&collector).await {
                if label == crate::orchestrator::dispatch::BUDGET_PARTIAL_RESULT_LABEL {
                    if let Some(ref text) = final_response {
                        // Prefer the granular cap label exposed via
                        // `terminate_detail` (`"hit_max_iterations"` /
                        // `"context_budget_exhausted"` /
                        // `"max_output_tokens_exhausted"`); fall back to
                        // the umbrella token when an older binary on the
                        // emitter side did not populate the detail field.
                        let reason = detail.unwrap_or_else(|| label.clone());
                        let record =
                            crate::tasks::cron::carryover::CarryOver::new(text.clone(), reason);
                        if let Err(e) = crate::tasks::cron::carryover::write(&snapshot.id, &record)
                        {
                            warn!(
                                job_id = %snapshot.id,
                                error = %e,
                                "failed to persist cron carryover; \
                                 next firing will start without resume context",
                            );
                        } else {
                            wrote_carryover = true;
                            info!(
                                job_id = %snapshot.id,
                                "cron job hit budget cap with partial result; \
                                 wrote carryover for next run",
                            );
                        }
                    }
                }
            }

            // If this run did not itself produce a fresh carryover, ensure any
            // prior one is gone. The read-time clear in `build_cron_prompt` is
            // best-effort; if it failed (transient FS error), this idempotent
            // post-run clear stops a stale partial from being re-injected into
            // a later, already-completed run. `clear` is a no-op when absent.
            if !wrote_carryover {
                if let Err(e) = crate::tasks::cron::carryover::clear(&snapshot.id) {
                    warn!(
                        job_id = %snapshot.id,
                        error = %e,
                        "failed to clear stale cron carryover post-run",
                    );
                }
            }

            // Honour ALEPH_* protocol tokens at the delivery boundary. The
            // Background-paradigm prompt (`ProtocolTokensLayer`) teaches the
            // model to answer with these sentinels when there is nothing worth
            // notifying; without this gate the literal token text would reach
            // the user's channel. History (`ExecutionResult.output`) keeps the
            // raw text for auditability — only delivery is filtered.
            let deliverable = final_response.as_deref().and_then(deliverable_text);

            // Deliver response to source channel if available
            let delivery_status = if let (Some(ref ch_id), Some(ref conv_id)) =
                (&deliver_channel, &deliver_conversation)
            {
                match deliverable {
                    Some(ref response_text) => {
                        deliver_to_channel(
                            &channel_registry_cell,
                            ch_id,
                            conv_id,
                            response_text,
                            &snapshot.id,
                        )
                        .await
                    }
                    None if final_response.is_some() => {
                        info!(job_id = %snapshot.id, "cron job replied with a silent protocol token, suppressing delivery");
                        DeliveryStatus::NotDelivered
                    }
                    None => {
                        info!(job_id = %snapshot.id, "cron job produced no response text, skipping delivery");
                        DeliveryStatus::NotDelivered
                    }
                }
            } else {
                DeliveryStatus::NotDelivered
            };

            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at.saturating_sub(started_at),
                status: RunStatus::Ok,
                output: if fell_back {
                    Some(prepend_fallback_note(final_response, &requested_agent))
                } else {
                    final_response
                },
                error: None,
                error_reason: None,
                delivery_status: Some(delivery_status),
                agent_used_messaging_tool: false,
                trigger_source: snapshot.trigger_source,
                retry_hint: None,
            }
        }
        Err(ExecutionError::Timeout) => {
            error!(job_id = %snapshot.id, "cron job timed out");
            let ended_at = chrono::Utc::now().timestamp_millis();
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at.saturating_sub(started_at),
                status: RunStatus::Timeout,
                output: None,
                error: Some("job execution timed out".to_string()),
                error_reason: Some(ErrorReason::Transient("timeout".to_string())),
                delivery_status: None,
                agent_used_messaging_tool: false,
                trigger_source: snapshot.trigger_source,
                retry_hint: Some(classify("timeout")),
            }
        }
        Err(ExecutionError::AgentBusy(msg)) => {
            warn!(job_id = %snapshot.id, %msg, "cron job skipped: agent busy");
            let ended_at = chrono::Utc::now().timestamp_millis();
            // Agent-busy is a temporary local-side condition, treat as transient
            // network-shaped backpressure regardless of the message text.
            let hint =
                RetryHint::transient(crate::tasks::shared::retry_hint::RetryCategory::Overloaded);
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at.saturating_sub(started_at),
                status: RunStatus::Skipped,
                output: None,
                error: Some(format!("agent busy: {msg}")),
                error_reason: Some(ErrorReason::Transient(msg)),
                delivery_status: None,
                agent_used_messaging_tool: false,
                trigger_source: snapshot.trigger_source,
                retry_hint: Some(hint),
            }
        }
        Err(e) => {
            error!(job_id = %snapshot.id, error = %e, "cron job failed");

            let err_text = e.to_string();
            let hint = classify(&err_text);
            let error_msg = format!(
                "❌ Cron job execution failed\n\nJob: {}\nError: {}",
                snapshot.id, err_text
            );
            if let (Some(ref ch_id), Some(ref conv_id)) = (
                &snapshot.source_channel_id,
                &snapshot.source_conversation_id,
            ) {
                let _ = deliver_to_channel(
                    &channel_registry_cell,
                    ch_id,
                    conv_id,
                    &error_msg,
                    &snapshot.id,
                )
                .await;
            }

            // Match historical behaviour: errors that look transient stay
            // `Transient`; otherwise mark `Permanent` so phase3 can short-circuit
            // retries instead of hammering with backoff.
            let error_reason = if hint.retryable {
                ErrorReason::Transient(err_text.clone())
            } else {
                ErrorReason::Permanent(err_text.clone())
            };
            make_error_result(
                started_at,
                err_text,
                error_reason,
                hint,
                snapshot.trigger_source,
            )
        }
    }
}

/// Run metadata for a cron job: traceability keys, the origin route when the job
/// has one, and the `unattended` fail-closed marker when it does not.
///
/// An approval prompt is deliverable only when BOTH halves of the origin route
/// survive into the run — `TurnContext::is_channel_routable` demands a non-empty
/// channel AND conversation before `FallbackApprovalRequester` takes the channel
/// path. Without both, a confirm-gated tool (the default `Auto` tier already asks
/// on `vault_*` / `*_delete` / destructive `file_ops`) publishes an approval card
/// into the void and parks the whole job on it for the 120 s approval timeout
/// before failing anyway — and a job with a shorter `timeout_ms` simply dies
/// there. The marker makes that gate fail CLOSED instead: an immediate deny, with
/// a hint the model sees and can work around in the same turn.
///
/// A job that DOES carry a full origin route is deliberately left unmarked: its
/// approval is genuinely deliverable (the bridge reaches the registered channel
/// and the user can `/approve` from Telegram), and the marker would auto-deny a
/// human-in-the-loop path that works today.
fn build_cron_metadata(snapshot: &JobSnapshot) -> HashMap<String, String> {
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
    let approval_is_routable =
        snapshot.source_channel_id.is_some() && snapshot.source_conversation_id.is_some();
    if !approval_is_routable {
        metadata.insert(
            crate::gateway::execution_engine::UNATTENDED_KEY.to_string(),
            "true".to_string(),
        );
    }
    // P1 data isolation: cron has no completing run to inherit metadata from
    // (this run IS the first), so rehydrate owner/scope from the job's
    // persisted fields — the same fail-closed reconstruction the goal wake
    // service uses for its own hook-less continuations. `from_persisted`
    // requires both columns coherent; a legacy (pre-P1) job with neither set
    // emits nothing here → the run stays unscoped, zero behavior change.
    if let Some(attr) = crate::scope::ScopeAttribution::from_persisted(
        snapshot.owner_user_id.as_deref(),
        snapshot.scope_id.as_deref(),
    ) {
        crate::scope::stamp_metadata(&mut metadata, &attr);
    }
    metadata
}

/// Build the final prompt string, injecting cron context header and
/// (if present) the previous run's `BudgetExhaustedPartialResult` carry-over.
///
/// The carry-over prefix is wrapped in `<carryover reason=...>` tags so
/// the LLM recognises it as harness-supplied resume context, not a fresh
/// instruction from the user.
fn build_cron_prompt(snapshot: &JobSnapshot) -> String {
    let mut parts = Vec::new();

    parts.push(format!("[Cron Task: {}]", snapshot.id));

    if snapshot.source_channel_id.is_some() {
        parts.push(
            "You are executing a scheduled task. Produce your final answer as plain text — \
             the runtime will deliver it to the user who created this task automatically. \
             Do NOT call any messaging tool to send the result."
                .to_string(),
        );
    }

    // P3: pick up partial work from the previous BudgetExhaustedPartialResult
    // run, if any. `read` returns Ok(None) for the common "no prior partial"
    // case; surface IO errors as warnings rather than blocking the run — a
    // broken carry-over file should not break the job.
    //
    // Do NOT clear the carry-over here. Doing so before `adapter.execute`
    // runs would discard the partial progress if the execution itself
    // fails (timeout / panic / permanent error), forcing the next firing
    // to start over from zero. The post-run branch in `execute_cron_job`
    // is the single point that either replaces the file with a fresh
    // partial (BudgetExhaustedPartialResult path) or deletes it
    // idempotently when the run completed cleanly.
    match crate::tasks::cron::carryover::read(&snapshot.id) {
        Ok(Some(record)) => {
            let prefix = crate::tasks::cron::carryover::render_prefix(&record);
            if !prefix.is_empty() {
                parts.push(prefix);
            }
        }
        Ok(None) => {}
        Err(e) => {
            warn!(
                job_id = %snapshot.id,
                error = %e,
                "failed to read cron carryover; proceeding without resume context",
            );
        }
    }

    parts.push(String::new()); // blank line separator
    parts.push(snapshot.prompt.clone());

    parts.join("\n")
}

/// Extract the final response text from collected events.
///
/// Sanitizes the output (strips `<completion-check>`, `<task-complete/>`, thinking
/// tags, etc.) and falls back to concatenated `ResponseChunk` deltas when the
/// `RunSummary.final_response` is empty after sanitization (e.g. when the last
/// LLM turn was purely a completion-protocol confirmation).
/// Extract the harness's terminate label + granular detail from the
/// collected events.
///
/// Returns `(label, detail)` where:
/// - `label` is `RunSummary.terminate_reason` — the umbrella static
///   token (`"completed"`, `"budget_exhausted_partial_result"`, ...).
///   Used to gate the carry-over write path.
/// - `detail` is `RunSummary.terminate_detail` — populated only when
///   the umbrella variant collapses a granular cap label. For
///   `BudgetExhaustedPartialResult` this is e.g. `"hit_max_iterations"`
///   so the carry-over file records *which* budget fired, not just
///   "some budget".
///
/// `None` when no `RunComplete` event was observed.
async fn extract_terminate_reason(
    collector: &CollectingEventEmitter,
) -> Option<(String, Option<String>)> {
    let events = collector.events().await;
    for event in events.into_iter().rev() {
        if let StreamEvent::RunComplete { summary, .. } = event {
            if let Some(label) = summary.terminate_reason {
                return Some((label, summary.terminate_detail));
            }
        }
    }
    None
}

/// Map a run's final text to what (if anything) should reach the channel.
///
/// ALEPH_* protocol tokens (taught by `ProtocolTokensLayer` on the
/// Background paradigm) are the model's way to opt out of user
/// notification: silent variants suppress delivery, `NEEDS_ATTENTION`
/// delivers just its payload. Normal text passes through unchanged.
fn deliverable_text(text: &str) -> Option<String> {
    use crate::thinker::protocol_tokens::ProtocolToken;
    match ProtocolToken::parse(text) {
        Some(ProtocolToken::NeedsAttention(msg)) => Some(msg),
        Some(_) => None,
        None => Some(text.to_string()),
    }
}

/// Deliver the cron job response to the source channel via `ChannelRegistry`.
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
            warn!(
                job_id,
                "cron delivery skipped: ChannelRegistry not yet initialized"
            );
            return DeliveryStatus::NotDelivered;
        }
    };

    let ch_id = crate::gateway::channel::ChannelId::new(channel_id);
    let message = OutboundMessage::text(conversation_id.to_string(), text.to_string());

    match registry.send(&ch_id, message).await {
        Ok(_) => {
            info!(
                job_id,
                channel_id, conversation_id, "cron job result delivered"
            );
            DeliveryStatus::Delivered
        }
        Err(e) => {
            error!(job_id, channel_id, conversation_id, error = %e, "cron delivery failed");
            DeliveryStatus::NotDelivered
        }
    }
}

/// Bilingual note prepended to a cron run's persisted output when the
/// requested agent was missing and the run fell back to the default agent.
/// Kept as a fixed bilingual string because the executor has no panel i18n
/// context.
fn fallback_note(requested: &str) -> String {
    format!(
        "原 agent '{requested}' 不存在，已回退到 main / \
         Agent '{requested}' not found, fell back to main"
    )
}

/// Prepend [`fallback_note`] to a run's output. When the run produced no text,
/// the note becomes the entire output so the fallback stays visible in run
/// history.
fn prepend_fallback_note(output: Option<String>, requested: &str) -> String {
    let note = fallback_note(requested);
    match output {
        Some(text) => format!("{note}\n{text}"),
        None => note,
    }
}

/// Build an error `ExecutionResult`.
fn make_error_result(
    started_at: i64,
    error: String,
    reason: ErrorReason,
    retry_hint: RetryHint,
    trigger_source: TriggerSource,
) -> ExecutionResult {
    let ended_at = chrono::Utc::now().timestamp_millis();
    ExecutionResult {
        started_at,
        ended_at,
        duration_ms: ended_at.saturating_sub(started_at),
        status: RunStatus::Error,
        output: None,
        error: Some(error),
        error_reason: Some(reason),
        delivery_status: None,
        agent_used_messaging_tool: false,
        trigger_source,
        retry_hint: Some(retry_hint),
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
            owner_user_id: None,
            scope_id: None,
        }
    }

    #[test]
    fn test_build_cron_prompt_with_channel() {
        let snapshot = make_test_snapshot();
        let prompt = build_cron_prompt(&snapshot);
        assert!(prompt.contains("[Cron Task: test-job-1]"));
        assert!(prompt.contains("scheduled task"));
        assert!(prompt.contains("Do NOT call any messaging tool"));
        assert!(prompt.contains("Check the weather"));
    }

    #[test]
    fn test_build_cron_prompt_without_channel() {
        let mut snapshot = make_test_snapshot();
        snapshot.source_channel_id = None;
        let prompt = build_cron_prompt(&snapshot);
        assert!(prompt.contains("[Cron Task: test-job-1]"));
        assert!(!prompt.contains("scheduled task"));
        assert!(prompt.contains("Check the weather"));
    }

    /// The wiring guard for the fail-closed marker. A clock-driven job with no
    /// origin channel has nobody to answer an approval card, so the run must be
    /// marked unattended — otherwise a confirm-gated tool parks the whole job on
    /// the 120 s approval timeout.
    #[test]
    fn a_channelless_cron_job_runs_unattended() {
        use crate::gateway::execution_engine::UNATTENDED_KEY;
        let mut snapshot = make_test_snapshot();
        snapshot.source_channel_id = None;
        snapshot.source_conversation_id = None;
        assert_eq!(
            build_cron_metadata(&snapshot)
                .get(UNATTENDED_KEY)
                .map(String::as_str),
            Some("true")
        );

        // Half a route is not a route: `is_channel_routable` needs both.
        let mut half = make_test_snapshot();
        half.source_conversation_id = None;
        assert!(build_cron_metadata(&half).contains_key(UNATTENDED_KEY));
    }

    /// The negative half: a job with a full origin route CAN reach the user
    /// (`/approve` from the channel), so marking it would auto-deny a working
    /// human-in-the-loop path.
    #[test]
    fn a_channel_bound_cron_job_keeps_its_approval_route() {
        use crate::gateway::execution_engine::UNATTENDED_KEY;
        let metadata = build_cron_metadata(&make_test_snapshot());
        assert!(!metadata.contains_key(UNATTENDED_KEY));
        assert_eq!(
            metadata.get("channel_id").map(String::as_str),
            Some("discord:general")
        );
        assert_eq!(
            metadata.get("conversation_id").map(String::as_str),
            Some("123456")
        );
    }

    /// `build_cron_metadata` rehydrates owner/scope from the job snapshot's
    /// persisted fields — the fire path has no completing run to inherit
    /// metadata from, so it must reconstruct attribution itself.
    #[test]
    fn owned_snapshot_emits_scope_metadata_keys() {
        let mut snapshot = make_test_snapshot();
        snapshot.owner_user_id = Some("u-alice".to_string());
        snapshot.scope_id = Some("personal:u-alice".to_string());
        let metadata = build_cron_metadata(&snapshot);
        assert_eq!(
            metadata
                .get(crate::scope::OWNER_META_KEY)
                .map(String::as_str),
            Some("u-alice")
        );
        assert_eq!(
            metadata
                .get(crate::scope::SCOPE_META_KEY)
                .map(String::as_str),
            Some("personal:u-alice")
        );
    }

    /// A legacy (pre-P1) job with no owner/scope columns emits neither key —
    /// the run stays unscoped, zero behavior change.
    #[test]
    fn legacy_unowned_snapshot_emits_no_scope_metadata() {
        let metadata = build_cron_metadata(&make_test_snapshot());
        assert!(!metadata.contains_key(crate::scope::OWNER_META_KEY));
        assert!(!metadata.contains_key(crate::scope::SCOPE_META_KEY));
    }

    /// Fail-closed: an owner present with an unparseable/incoherent scope_id
    /// must not emit a half-written attribution (mirrors
    /// `ScopeAttribution::from_persisted`'s own "never guess" contract).
    #[test]
    fn incoherent_snapshot_emits_no_scope_metadata() {
        let mut snapshot = make_test_snapshot();
        snapshot.owner_user_id = Some("u-alice".to_string());
        snapshot.scope_id = None;
        let metadata = build_cron_metadata(&snapshot);
        assert!(!metadata.contains_key(crate::scope::OWNER_META_KEY));
        assert!(!metadata.contains_key(crate::scope::SCOPE_META_KEY));
    }

    #[test]
    fn fallback_note_names_requested_agent() {
        let n = fallback_note("oldie");
        assert!(n.contains("oldie"), "note must name the missing agent");
        assert!(n.contains("main"), "note must mention the fallback target");
    }

    #[test]
    fn prepend_fallback_note_prefixes_existing_output() {
        let out = prepend_fallback_note(Some("done".to_string()), "oldie");
        let expected = format!("{}\n{}", fallback_note("oldie"), "done");
        assert_eq!(out, expected);
    }

    #[test]
    fn prepend_fallback_note_uses_note_as_output_when_output_is_none() {
        let out = prepend_fallback_note(None, "oldie");
        assert_eq!(out, fallback_note("oldie"));
    }

    #[test]
    fn deliverable_text_suppresses_silent_protocol_tokens() {
        assert_eq!(deliverable_text("ALEPH_SILENT_COMPLETE"), None);
        assert_eq!(deliverable_text("  ALEPH_NO_REPLY \n"), None);
        assert_eq!(deliverable_text("ALEPH_HEARTBEAT_OK"), None);
    }

    #[test]
    fn deliverable_text_unwraps_needs_attention_payload() {
        assert_eq!(
            deliverable_text("ALEPH_NEEDS_ATTENTION: disk at 95%").as_deref(),
            Some("disk at 95%")
        );
    }

    #[test]
    fn deliverable_text_passes_normal_text_through() {
        // Mixed content is a normal reply, not a token (tokens must be the
        // entire message) — it must reach the channel untouched.
        assert_eq!(
            deliverable_text("All good. ALEPH_HEARTBEAT_OK").as_deref(),
            Some("All good. ALEPH_HEARTBEAT_OK")
        );
        assert_eq!(
            deliverable_text("Weather: sunny, 22°C").as_deref(),
            Some("Weather: sunny, 22°C")
        );
    }

    async fn test_registry_with_main() -> (tempfile::TempDir, AgentRegistry) {
        use crate::gateway::agent_instance::AgentInstanceConfig;
        use crate::gateway::session_store::sqlite_backend::{
            SqliteSessionStore, SqliteSessionStoreConfig,
        };
        use crate::gateway::session_store::SessionStore;

        let temp = tempfile::tempdir().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            SqliteSessionStore::new(SqliteSessionStoreConfig {
                db_path: temp.path().join("s.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let registry = AgentRegistry::new(); // default_agent = "main"
        let main = AgentInstance::new(
            AgentInstanceConfig {
                agent_id: "main".to_string(),
                workspace: temp.path().join("ws"),
                agent_dir: temp.path().join("agents/main"),
                ..Default::default()
            },
            store,
        )
        .unwrap();
        registry.register(main).await;
        (temp, registry)
    }

    #[tokio::test]
    async fn resolve_uses_requested_agent_when_present() {
        let (_t, registry) = test_registry_with_main().await;
        let (inst, used, fell_back) = resolve_cron_agent(&registry, "main").await.unwrap();
        assert_eq!(inst.id(), "main");
        assert_eq!(used, "main");
        assert!(!fell_back);
    }

    #[tokio::test]
    async fn resolve_falls_back_to_default_when_missing() {
        let (_t, registry) = test_registry_with_main().await;
        let (inst, used, fell_back) = resolve_cron_agent(&registry, "ghost").await.unwrap();
        assert_eq!(inst.id(), "main");
        assert_eq!(used, "main");
        assert!(fell_back);
    }

    #[tokio::test]
    async fn resolve_returns_none_when_default_absent() {
        let registry = AgentRegistry::new(); // empty: no "main"
        assert!(resolve_cron_agent(&registry, "ghost").await.is_none());
    }
}
