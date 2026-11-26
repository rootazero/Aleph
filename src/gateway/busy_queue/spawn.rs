//! Spawn a run onto its session's wait lane, for surfaces whose feedback
//! channel is the run event stream (Panel / CLI over JSON-RPC).
//!
//! The inbound router keeps its own copy of this shape because a chat channel
//! answers with an `OutboundMessage` through the channel registry, not with a
//! `StreamEvent`. What must **not** be duplicated is the rule for *who owes the
//! user a receipt* — that rule lives with [`DeliveryOutcome`], and this is its
//! one implementation for stream surfaces.

use super::{
    deliver_with_ticket, durable, register_run, BusyQueueConfig, DeliveryOutcome, TicketGuard,
};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::gateway::handlers::agent::{AgentRunManager, QueuedChatRun};
use crate::gateway::i18n::Locale;
use crate::gateway::{EventEmitter, ExecutionEngine, StreamEvent};
use crate::sync_primitives::Arc;

/// Spawn a Panel / CLI run onto the shared per-session busy wait lane.
///
/// Both RPC entry points (`agent.run` and `chat.send`) route through here so the
/// local surfaces get the same follow-up lane the inbound router gives external
/// channels. Before this, a Panel message that could not be delivered mid-run —
/// an attachment-bearing steer (which `try_inject_steering` deliberately defers,
/// because a steering event has no media seam) or a steering burst at
/// `max_pending_steering` — surfaced as an `AgentBusy` error bubble and the
/// message was simply lost. Now it waits its turn like every other queued
/// message.
///
/// The ticket is taken **synchronously, before the spawn**: registering inside
/// the task would make lane order follow task scheduling instead of arrival
/// order.
///
/// Error reporting is split by whether the attempt actually ran: the engine
/// already emits `RunError` for anything that fails *inside* `execute`, so
/// re-emitting here would show the Panel two error bubbles for one failure. Only
/// the queue-stage outcomes are this function's to report — see the `Purged`
/// arm for the one surface-specific addition.
pub fn spawn_queued_run<P, R, E>(
    engine: Arc<ExecutionEngine<P, R>>,
    request: RunRequest,
    agent: Arc<AgentInstance>,
    emitter: Arc<E>,
    cfg: BusyQueueConfig,
    locale: Locale,
    label: &'static str,
) where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
    E: EventEmitter + Send + Sync + 'static,
{
    let ticket = register_queue_ticket(&request, cfg.max_per_session);
    // Only registered ordinary runs are eligible for queue recovery. A
    // rejected ticket has no payload to recover.
    if ticket.is_some() && durable::is_armed() {
        if let Some(payload) = durable::QueuedRunPayload::from_request(&request, locale) {
            durable::record_enqueued(payload);
        }
    }
    spawn_registered_run(
        engine, request, agent, emitter, cfg, locale, label, ticket, None,
    );
}

/// Submit an exact-cancellable Chat receipt through the same live wait lane.
/// Do not journal it for queue recovery: the receipt's authority and cancellation
/// decision live in this process and cannot be reconstructed during boot replay.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_queued_chat_run<P, R, E>(
    engine: Arc<ExecutionEngine<P, R>>,
    request: RunRequest,
    agent: Arc<AgentInstance>,
    emitter: Arc<E>,
    cfg: BusyQueueConfig,
    locale: Locale,
    label: &'static str,
    manager: &AgentRunManager,
) where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
    E: EventEmitter + Send + Sync + 'static,
{
    // Keep arrival order: take the ticket before waiting on the manager's run map.
    let ticket = register_queue_ticket(&request, cfg.max_per_session);
    let managed = manager.bind_queued_chat_run(&request).await;
    spawn_registered_run(
        engine, request, agent, emitter, cfg, locale, label, ticket, managed,
    );
}

fn register_queue_ticket(request: &RunRequest, max_per_session: usize) -> Option<TicketGuard> {
    if request.owns_worktree() {
        return None;
    }
    // A side question waits on the derived execution session, not the addressed session.
    register_run(
        &request.session_key,
        &request.metadata,
        max_per_session,
        &request.run_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_registered_run<P, R, E>(
    engine: Arc<ExecutionEngine<P, R>>,
    request: RunRequest,
    agent: Arc<AgentInstance>,
    emitter: Arc<E>,
    cfg: BusyQueueConfig,
    locale: Locale,
    label: &'static str,
    ticket: Option<TicketGuard>,
    managed: Option<QueuedChatRun>,
) where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
    E: EventEmitter + Send + Sync + 'static,
{
    let run_id = request.run_id.clone();
    // The queue may be on a derived /btw session; receipts name the addressed session.
    let session_key = request.session_key.to_key_string();
    let agent_id = request.session_key.agent_id().to_string();
    if let Some(managed) = &managed {
        if ticket.is_none() {
            let _ = managed.cancellation.begin_terminal_commit();
        }
    }

    tokio::spawn(async move {
        let outcome = match ticket {
            None => DeliveryOutcome::Rejected,
            Some(ticket) => {
                // The lane's own surface for "still waiting" — the shared
                // constructor carries the addressed-vs-derived session rule
                // and the failure policy; see `run_queued_reporter`.
                let report_emitter: Arc<dyn EventEmitter + Send + Sync> = emitter.clone();
                let mut report =
                    super::run_queued_reporter(report_emitter, run_id.clone(), session_key.clone());
                if let Some(run) = &managed {
                    let token = run.cancellation.token();
                    let mut attempt = || {
                        engine.execute_with_chat_cancellation(
                            request.clone(),
                            agent.clone(),
                            emitter.clone(),
                            Some(&run.cancellation),
                        )
                    };
                    super::deliver::deliver_with_cancellation(
                        ticket,
                        cfg,
                        &mut attempt,
                        &mut report,
                        Some(&token),
                    )
                    .await
                } else {
                    let mut attempt =
                        || engine.execute(request.clone(), agent.clone(), emitter.clone());
                    deliver_with_ticket(ticket, cfg, &mut attempt, &mut report).await
                }
            }
        };

        // Executed attempts settle at their own outcome boundary. Only a
        // never-ran receipt belongs to the queue, before the durable settle.
        let outcome = if managed.as_ref().is_some_and(|run| {
            !matches!(outcome, DeliveryOutcome::Executed(_))
                && run.cancellation.begin_terminal_commit().is_err()
        }) {
            DeliveryOutcome::Purged
        } else {
            outcome
        };

        // Which failure (if any) this surface still owes a terminal frame for.
        // `DeliveryOutcome::user_error` covers the two never-ran outcomes; the
        // extra `Purged` arm is surface-specific: a channel hears about a purge
        // once, in the `/stop` reply, but the Panel has an in-flight bubble
        // keyed to this `run_id` and nothing else will ever close it — without a
        // terminal frame the user is left with a spinner they just stopped.
        let mut pending_error = match &outcome {
            DeliveryOutcome::Executed(Ok(())) => {
                tracing::info!(run_id = %run_id, "{label} completed successfully");
                durable::record_settled(&run_id, durable::SettleReason::AttemptConcluded);
                None
            }
            DeliveryOutcome::Executed(Err(e)) => {
                // Raw chain goes to server logs only; the engine already put a
                // sanitized receipt on the wire (shared single source with
                // `ExecutionError::user_receipt`).
                tracing::error!(run_id = %run_id, error = %e, "{label} failed");
                durable::record_settled(&run_id, durable::SettleReason::AttemptConcluded);
                None
            }
            DeliveryOutcome::Purged => {
                tracing::info!(run_id = %run_id, session = %session_key,
                    "{label} dropped: an explicit stop abandoned it while it was queued");
                durable::record_settled(&run_id, durable::SettleReason::Purged);
                Some(ExecutionError::Cancelled)
            }
            DeliveryOutcome::Rejected | DeliveryOutcome::TimedOut => {
                if matches!(outcome, DeliveryOutcome::TimedOut) {
                    durable::record_settled(&run_id, durable::SettleReason::TimedOut);
                }
                outcome.user_error(&agent_id)
            }
        };

        if let Some(managed) = managed {
            match outcome {
                DeliveryOutcome::Executed(result) => managed.settle(&result).await,
                _ => {
                    let result = pending_error.take().map_or(Ok(()), Err);
                    managed.settle(&result).await;
                    pending_error = result.err();
                }
            }
        }

        let Some(e) = pending_error else {
            return;
        };
        tracing::warn!(run_id = %run_id, session = %session_key, error = %e,
            "{label} never reached the agent");
        let (error_code, error_message) = e.user_receipt(locale);
        let seq = emitter.next_seq();
        if let Err(emit_err) = emitter
            .emit(StreamEvent::RunError {
                run_id: run_id.clone(),
                seq,
                error: error_message,
                error_code: Some(error_code.to_string()),
                // This is the one `RunError` producer whose run never reached
                // the engine, so no `RunAccepted` ever seeded the delivery
                // filter's run→session index and `ByRunId` resolution fails
                // closed. Naming the session makes the frame seed itself
                // (`EventVisibilityIndex::note_frame`). Without it every
                // receipt below this line is built, emitted, and then dropped
                // by the gateway before any client sees it — which is what was
                // happening to all three never-ran outcomes.
                session_key: Some(session_key.clone()),
            })
            .await
        {
            tracing::warn!("Failed to emit run error event: {}", emit_err);
        }
    });
}
