//! Spawn a run onto its session's wait lane, for surfaces whose feedback
//! channel is the run event stream (Panel / CLI over JSON-RPC).
//!
//! The inbound router keeps its own copy of this shape because a chat channel
//! answers with an `OutboundMessage` through the channel registry, not with a
//! `StreamEvent`. What must **not** be duplicated is the rule for *who owes the
//! user a receipt* — that rule lives with [`DeliveryOutcome`], and this is its
//! one implementation for stream surfaces.

use super::{deliver_with_ticket, register_run, BusyQueueConfig, DeliveryOutcome};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
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
    let run_id = request.run_id.clone();
    let session_key = request.session_key.to_key_string();
    let agent_id = request.session_key.agent_id().to_string();
    // `register_run`, not `register`: the lane is keyed on the session this run
    // will EXECUTE on, which for a side question is not the one it was
    // addressed to. See that function for what keying it wrong costs.
    //
    // `session_key` above stays the session it was ADDRESSED to, and the two
    // uses below want that one: the log line reads better against the
    // conversation a person is looking at, and the `RunError` receipt for a run
    // that never reached the engine has to resolve on the client that is
    // attached to it — the derived session may have no row at all yet.
    let ticket = register_run(
        &request.session_key,
        &request.metadata,
        cfg.max_per_session,
        &run_id,
    );

    tokio::spawn(async move {
        let outcome = match ticket {
            None => DeliveryOutcome::Rejected,
            Some(ticket) => {
                let mut attempt =
                    || engine.execute(request.clone(), agent.clone(), emitter.clone());
                // The lane's own surface for "still waiting". `session_key`
                // is the ADDRESSED session, not the derived execution lane
                // — same reason the never-ran `RunError` below names it:
                // the client resolving this frame is attached to the
                // former, and this is the run's first frame, so nothing
                // has seeded the run→session index yet.
                let queued_emitter = emitter.clone();
                let queued_run_id = run_id.clone();
                let queued_session = session_key.clone();
                let mut report = move |ahead: u16| {
                    let emitter = queued_emitter.clone();
                    let run_id = queued_run_id.clone();
                    let session_key = queued_session.clone();
                    async move {
                        if let Err(e) = emitter
                            .emit(StreamEvent::RunQueued {
                                run_id,
                                session_key,
                                ahead,
                            })
                            .await
                        {
                            // Best-effort mirror: `chat.history.pending`
                            // is the authoritative half, so a dropped
                            // frame costs liveness, never correctness.
                            tracing::debug!("failed to emit RunQueued: {e}");
                        }
                    }
                };
                deliver_with_ticket(ticket, cfg, &mut attempt, &mut report).await
            }
        };

        // Which failure (if any) this surface still owes a terminal frame for.
        // `DeliveryOutcome::user_error` covers the two never-ran outcomes; the
        // extra `Purged` arm is surface-specific: a channel hears about a purge
        // once, in the `/stop` reply, but the Panel has an in-flight bubble
        // keyed to this `run_id` and nothing else will ever close it — without a
        // terminal frame the user is left with a spinner they just stopped.
        let pending_error = match outcome {
            DeliveryOutcome::Executed(Ok(())) => {
                tracing::info!(run_id = %run_id, "{label} completed successfully");
                return;
            }
            DeliveryOutcome::Executed(Err(ref e)) => {
                // Raw chain goes to server logs only; the engine already put a
                // sanitized receipt on the wire (shared single source with
                // `ExecutionError::user_receipt`).
                tracing::error!(run_id = %run_id, error = %e, "{label} failed");
                None
            }
            DeliveryOutcome::Purged => {
                tracing::info!(run_id = %run_id, session = %session_key,
                    "{label} dropped: an explicit stop abandoned it while it was queued");
                Some(ExecutionError::Cancelled)
            }
            DeliveryOutcome::Rejected | DeliveryOutcome::TimedOut => outcome.user_error(&agent_id),
        };

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
