//! Subagent completion announce — proactive delivery of background results.
//!
//! Background subagent runs (`subagent` tool, `background: true`) used to
//! finish silently: `BackgroundAgentTracker::mark_completed` parked the result
//! and the only reader was the LLM-initiated `list`/`check_status` action. If the
//! parent's run had already ended, nobody would ever look — the user never
//! heard back (an R5 violation, and the gap openclaw closes with its
//! `subagent.task_completed` announce flow).
//!
//! This module is the consuming half of the wire: `subagent_tool::spawn`
//! broadcasts [`AlephEvent::SubAgentCompleted`] on the [`GlobalBus`] (scope =
//! parent session key), and the subscriber here drives ONE proactive turn on
//! the parent session via the [`ExecutionAdapter`]:
//!
//! - parent idle → a fresh run processes the result and reports to the user
//!   (the reply fans out to the session's origin channel, mirroring
//!   `handlers::agent`'s `OriginFanout` wiring);
//! - parent mid-run → `ExecutionEngine::execute`'s busy-input path injects the
//!   notice as steering, so the live run absorbs it at the next turn boundary
//!   (no extra run is spawned);
//! - parent busy elsewhere → bounded retries, then the result simply remains
//!   poll-able via the subagent tool (the pre-announce behaviour).
//!
//! No reasoning happens here (R10): the harness only delivers the event; what
//! to do with the result is decided by the parent agent's own turn.

use std::collections::HashMap;

use tracing::{debug, info, warn};

use crate::event::{AlephEvent, EventFilter, EventType, GlobalBus, GlobalEvent};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::event_emitter::{EventEmitter, GatewayEventEmitter};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;

/// Busy-retry schedule (seconds before each attempt). The first attempt is
/// immediate; later ones give a busy parent time to free its run slot.
const RETRY_DELAYS_SECS: [u64; 3] = [0, 30, 120];

/// Subscribe to `SubAgentCompleted` global events and announce each completed
/// background subagent into its parent session. Registration mirrors the
/// `TeamNotifier` pattern in `aleph-server` startup.
pub fn spawn_subagent_announce(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    event_bus: Arc<GatewayEventBus>,
) {
    tokio::spawn(async move {
        let _sub = GlobalBus::global()
            .subscribe_async(
                EventFilter::new(vec![EventType::SubAgentCompleted]),
                move |global_event| {
                    let adapter = adapter.clone();
                    let registry = registry.clone();
                    let event_bus = event_bus.clone();
                    tokio::spawn(async move {
                        announce_one(adapter, registry, event_bus, global_event).await;
                    });
                },
            )
            .await;
        info!("Subagent announce subscriber registered (SubAgentCompleted → parent session)");
    });
}

/// Deliver one completion into the parent session.
async fn announce_one(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    event_bus: Arc<GatewayEventBus>,
    global_event: GlobalEvent,
) {
    let AlephEvent::SubAgentCompleted(result) = &global_event.event else {
        return;
    };

    // `request_id` is optional on the wire type; the broadcaster always sets
    // it to the tracker's request id, with `child_session_id` (same value at
    // the spawn site) as the defensive fallback.
    let request_id = result
        .request_id
        .clone()
        .unwrap_or_else(|| result.child_session_id.clone());

    // Dedup with the on-demand paths: if the parent already saw this result via
    // a `wait` or a `check_status` tool call, skip the proactive announce rather
    // than spending a fresh parent turn re-delivering what the model has already
    // folded in. Pure data check against the process-global tracker (the same
    // instance the spawn/wait paths use) — no reasoning, R7/R10 clean.
    if crate::agents::background_tracker::BackgroundAgentTracker::global().is_consumed(&request_id)
    {
        debug!(
            request_id = %request_id,
            "subagent announce: result already consumed on-demand; skipping proactive delivery"
        );
        return;
    }

    let Some(session_key) = SessionKey::from_key_string(&global_event.source_session_id) else {
        debug!(
            session = %global_event.source_session_id,
            request_id = %request_id,
            "subagent announce: parent session key not parseable; result stays poll-only"
        );
        return;
    };

    let agent_id = session_key.agent_id().to_string();
    let Some(agent) = registry.get(&agent_id).await else {
        warn!(
            agent_id = %agent_id,
            request_id = %request_id,
            "subagent announce: parent agent not registered; result stays poll-only"
        );
        return;
    };

    let status = if result.success {
        "succeeded"
    } else {
        "failed"
    };
    let detail = if result.success {
        result.summary.clone()
    } else {
        result
            .error
            .clone()
            .unwrap_or_else(|| "(no error detail)".to_string())
    };
    let input = format!(
        "[system] Background subagent run {request_id} {status}.\n\
         Result summary:\n{detail}\n\n\
         Process this result now: report the outcome to the user in your \
         reply (or to your team leader via team messaging when you work as a \
         team member), and take any follow-up actions the original task \
         implies. Use the subagent tool's 'check_status' action with \
         request_id='{request_id}' if you need the full output.",
    );

    // Mirror handlers::agent — Panel stream as base, fan the final reply out
    // to the session's origin channel when one is bound.
    let base: Arc<dyn EventEmitter + Send + Sync> =
        Arc::new(GatewayEventEmitter::new(event_bus.clone()));
    let emitter: Arc<dyn EventEmitter + Send + Sync> = match (
        agent.origin_route(&session_key).await,
        crate::gateway::event_emitter::origin_fanout::channel_registry(),
    ) {
        (Some((origin_channel, origin_conversation)), Some(channel_registry)) => Arc::new(
            crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                base,
                channel_registry,
                origin_channel,
                origin_conversation,
            ),
        ),
        _ => base,
    };

    let mut metadata = HashMap::new();
    metadata.insert("subagent_announce".to_string(), request_id.clone());

    for delay_secs in RETRY_DELAYS_SECS {
        if delay_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        }

        let request = RunRequest {
            run_id: uuid::Uuid::new_v4().to_string(),
            input: input.clone(),
            session_key: session_key.clone(),
            timeout_secs: None,
            metadata: metadata.clone(),
            attachments: Vec::new(),
            pending_media: crate::gateway::media::PendingMedia::default(),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
        };

        match adapter
            .execute(request, agent.clone(), emitter.clone())
            .await
        {
            // Ok covers both "fresh announce run completed" and "absorbed by
            // the live run as steering" — either way the parent saw it.
            Ok(()) => {
                debug!(
                    request_id = %request_id,
                    session = %global_event.source_session_id,
                    "subagent announce delivered to parent session"
                );
                return;
            }
            Err(ExecutionError::AgentBusy(_)) => {
                continue;
            }
            Err(e) => {
                warn!(
                    request_id = %request_id,
                    session = %global_event.source_session_id,
                    error = %e,
                    "subagent announce run failed; result remains available via the subagent tool's list action"
                );
                return;
            }
        }
    }

    warn!(
        request_id = %request_id,
        session = %global_event.source_session_id,
        "subagent announce: parent stayed busy through all retries; result remains available via the subagent tool's list action"
    );
}
