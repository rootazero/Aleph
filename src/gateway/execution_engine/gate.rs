//! Admission gate for [`ExecutionEngine::execute`]: reserve the agent's
//! Idle→Running slot, apply the per-channel busy-input policy
//! (Interrupt / Queue / Steer) when the slot is already held, and — once the
//! slot is held — enforce the concurrent-run cap and register the run in
//! `active_runs`.
//!
//! Verbatim extraction from `execute()` (Task 2, behavior-preserving split)
//! so Task 6's gate rewrite has a small, isolated landing zone.

use super::{ActiveRun, ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::{AgentInstance, AgentState};
use crate::sync_primitives::Arc;
use tokio::sync::mpsc;
use tracing::info;

/// Outcome of [`ExecutionEngine::admit_run`] — tells `execute()` whether to
/// proceed with the run or return immediately.
pub(super) enum GateOutcome {
    /// Slot reserved and the run registered in `active_runs` — proceed with
    /// the run.
    Admitted,
    /// Already fully handled inline (a busy sibling on this session was
    /// steered via mid-loop injection) — `execute()` must return `Ok(())`
    /// now, without running anything.
    HandledInline,
}

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    /// Admission gate for a run. Verbatim extraction of the original
    /// `execute()` body (`execute.rs:123-247`) — see Task 2 (behavior-
    /// preserving split). `cancel_tx` is consumed: stored into the
    /// registered [`ActiveRun`] on [`GateOutcome::Admitted`], otherwise
    /// simply dropped (identical to the original inline control flow).
    pub(super) async fn admit_run(
        &self,
        request: &RunRequest,
        run_id: &str,
        agent: &Arc<AgentInstance>,
        cancel_tx: mpsc::Sender<()>,
    ) -> Result<GateOutcome, ExecutionError> {
        // Reserve the agent's run slot FIRST. The per-agent Idle→Running gate in
        // `try_start_run` is the single source of truth for concurrency.
        // Registering the run in `active_runs` before reserving the slot (the
        // previous order) inserted a transient `Running` row that inflated the
        // concurrent-run count and could spuriously reject a sibling run.
        // `simple.rs` already uses this ordering.
        if !agent.try_start_run(run_id).await {
            // Agent busy. The per-channel busy-input policy (stamped into
            // metadata by the inbound router; absent → Steer) decides what
            // happens to this message. The run was never registered, so there is
            // nothing to undo on either path.
            match super::BusyInputMode::from_metadata(&request.metadata) {
                super::BusyInputMode::Interrupt => {
                    // Cancel the running sibling on THIS session, then let the
                    // inbound router's FIFO busy queue restart this message as
                    // a fresh run once the slot frees — the new run reads the
                    // interrupted task's full context from the session log
                    // plus this instruction. Reuses `cancel` + the `AgentBusy`
                    // delivery path; no new dispatch machinery (R10). If no
                    // same-session sibling is running (e.g. cross-session
                    // busy), fall through to plain busy-queue waiting without
                    // cancelling anything.
                    //
                    // A content-less request (resume-style continuation, or a
                    // synthetic run that lost the `try_start_run` race and only
                    // inherited an `Interrupt` busy-input key) must never cancel
                    // a healthy sibling: there is nothing new to contribute, so
                    // tearing down in-flight work is pure loss. Symmetric with
                    // the `Steer` empty-content guard, and the same intrinsic
                    // protection Hermes gives `internal` events. Past this gate
                    // the message just waits its turn in the busy queue.
                    let target = if super::steering::has_steering_content(request) {
                        let runs = self.active_runs.read().await;
                        super::steering::find_steering_target_id(
                            &runs,
                            run_id,
                            &request.session_key,
                        )
                    } else {
                        None
                    };
                    if let Some(target_id) = target {
                        let _ = self.cancel(&target_id).await;
                        // No interruption marker is persisted here: the
                        // harness bridge emits `RunFinished{Cancelled}` when
                        // the cancelled loop tears down, and the prompt
                        // builder replays that as a `<system-reminder>`
                        // interruption note (covers /stop, Panel chat.abort
                        // and this Interrupt mode alike) — single source,
                        // nothing stored twice.
                        info!(
                            session = %request.session_key.to_key_string(),
                            target_run = %target_id,
                            "busy-input interrupt: cancelled running sibling; message will restart as a fresh run via the busy queue",
                        );
                    }
                    return Err(ExecutionError::AgentBusy(agent.id().to_string()));
                }
                super::BusyInputMode::Queue => {
                    // Follow-up lane (openclaw `followup` / hermes `queue` /
                    // Pi `followUp` / OpenSquilla `followup` parity): leave the
                    // running task untouched — no mid-loop injection, no
                    // cancellation — and let the inbound router's FIFO busy
                    // queue deliver this message as a fresh run once the
                    // current one finishes.
                    return Err(ExecutionError::AgentBusy(agent.id().to_string()));
                }
                super::BusyInputMode::Steer => {
                    // Mid-loop steering: if the busy run is on THIS session,
                    // inject the message into the live event log so the running
                    // loop picks it up at its next turn boundary (codex parity).
                    let injected = super::steering::try_inject_steering(
                        self.config.mid_turn_steering,
                        &self.active_runs,
                        self.orchestrator.as_ref(),
                        request,
                        run_id,
                    )
                    .await;
                    if injected {
                        return Ok(GateOutcome::HandledInline);
                    }
                    return Err(ExecutionError::AgentBusy(agent.id().to_string()));
                }
            }
        }

        // Slot held — now enforce the concurrent-run limit and register the run.
        {
            let mut runs = self.active_runs.write().await;
            let agent_runs = runs
                .values()
                .filter(|r| {
                    r.request.session_key.agent_id() == request.session_key.agent_id()
                        && matches!(r.state, RunState::Running)
                })
                .count();

            if agent_runs >= self.config.max_concurrent_runs {
                drop(runs);
                // Release the slot we just reserved before bailing out.
                agent.set_state(AgentState::Idle).await;
                return Err(ExecutionError::TooManyRuns(format!(
                    "Agent {} has {} active runs (max: {})",
                    request.session_key.agent_id(),
                    agent_runs,
                    self.config.max_concurrent_runs
                )));
            }

            runs.insert(
                run_id.to_string(),
                ActiveRun {
                    request: request.clone(),
                    state: RunState::Running,
                    started_at: chrono::Utc::now(),
                    completed_at: None,
                    steps_completed: 0,
                    current_tool: None,
                    cancel_tx: Some(cancel_tx),
                    seq_counter: crate::sync_primitives::AtomicU64::new(0),
                    chunk_counter: crate::sync_primitives::AtomicU32::new(0),
                },
            );
        }

        Ok(GateOutcome::Admitted)
    }
}
