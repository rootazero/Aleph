//! Admission gate for [`ExecutionEngine::execute`]: reserve the *session's*
//! run slot (per-session mutual exclusion — Task 6 replaces the old
//! per-agent `AgentState` gate), apply the per-channel busy-input policy
//! (Interrupt / Queue / Steer) when the slot is already held on THIS
//! session, and — once the slot is held — acquire a run-lifetime
//! concurrency permit and register the run in `active_runs`.
//!
//! Verbatim extraction from `execute()` (Task 2, behavior-preserving split)
//! so Task 6's gate rewrite has a small, isolated landing zone.
//!
//! # Task 6: per-session claim + run-lifetime permit
//!
//! The admission predicate is now [`super::session_run_registry::SessionRunRegistry::try_claim`]
//! instead of `agent.try_start_run` — two sessions of the SAME agent now run
//! in parallel; only a second run on the SAME session is rejected. Once
//! admitted, a [`super::concurrency::ConcurrencyLimiter`] permit bounds total
//! concurrency (global + per-agent caps). Both the session claim and the
//! permit are held for the run's whole lifetime by the [`RunSlot`] RAII
//! guard `execute()` binds at the top of its body — dropping it (including
//! on early return or panic) releases both.

use super::concurrency::RunPermit;
use super::{ActiveRun, ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::AgentInstance;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;
use tokio::sync::mpsc;
use tracing::info;

/// Outcome of [`ExecutionEngine::admit_run`] — tells `execute()` whether to
/// proceed with the run or return immediately.
pub(super) enum GateOutcome {
    /// Session slot claimed, a concurrency permit acquired, and the run
    /// registered in `active_runs` — proceed with the run. Carries the RAII
    /// [`RunSlot`] that `execute()` must hold for the run's lifetime;
    /// dropping it releases the session claim + permit (including on early
    /// return or panic).
    Admitted(RunSlot),
    /// Already fully handled inline (a busy sibling on this session was
    /// steered via mid-loop injection) — `execute()` must return `Ok(())`
    /// now, without running anything.
    HandledInline,
}

/// RAII guard for an admitted run: holds the session's run-mutex claim and
/// the run-lifetime `ConcurrencyLimiter` permit for as long as it's alive.
///
/// `execute()` binds this in its own body scope (NOT inside the `admit_run`
/// match arm) so it lives across the run's execution — including every early
/// return between admission and completion, and a panic unwind. It is
/// dropped **explicitly** at the same point the legacy per-agent gate used to
/// reset `AgentState::Idle` (before `execute()`'s post-run continuation/
/// steering-rescue logic, which may re-enter `execute()` on the SAME
/// session) rather than left to drop at the physical end of the function —
/// see `execute.rs` for the exact release point.
pub(super) struct RunSlot {
    registry: Arc<super::session_run_registry::SessionRunRegistry>,
    session_key: SessionKey,
    run_id: String,
    /// The run-lifetime concurrency permit. `Option` only so the `RunSlot` can
    /// be constructed the instant the session claim is taken — guarding the
    /// claim across the `ConcurrencyLimiter::acquire().await` below — and have
    /// the permit slotted in once acquired. It is always `Some` by the time
    /// `admit_run` returns [`GateOutcome::Admitted`]. Dropping the slot drops
    /// the permit (freeing the global + per-agent semaphore slots) and releases
    /// the session claim.
    _permit: Option<RunPermit>,
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        self.registry.release(&self.session_key, &self.run_id);
    }
}

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    /// Admission gate for a run. Verbatim extraction of the original
    /// `execute()` body (`execute.rs:123-247`) — see Task 2 (behavior-
    /// preserving split). Task 6 replaces the per-agent gate predicate with
    /// `SessionRunRegistry::try_claim` and adds the run-lifetime concurrency
    /// permit; the busy-input branches (Interrupt / Queue / Steer) are
    /// unchanged. `cancel_tx` is consumed: stored into the registered
    /// [`ActiveRun`] on [`GateOutcome::Admitted`], otherwise simply dropped
    /// (identical to the original inline control flow).
    pub(super) async fn admit_run(
        &self,
        request: &RunRequest,
        run_id: &str,
        agent: &Arc<AgentInstance>,
        cancel_tx: mpsc::Sender<()>,
    ) -> Result<GateOutcome, ExecutionError> {
        // Reserve the SESSION's run slot first (per-session mutual exclusion —
        // INV-SEQ / audit 4.2: exactly one run may be `Running` per session at
        // a time, or two runs could interleave writes into the same
        // `session_events` transcript). Sessions of the same agent no longer
        // contend with each other — they run in parallel, bounded only by
        // `ConcurrencyLimiter` below. Registering the run in `active_runs`
        // before reserving the slot (the previous order) inserted a transient
        // `Running` row that inflated the concurrent-run count and could
        // spuriously reject a sibling run. `simple.rs` already uses this
        // ordering (its own per-agent `try_start_run` gate, untouched).
        if !self
            .session_run_registry
            .try_claim(&request.session_key, run_id)
        {
            // A run is already active on THIS session. The per-channel
            // busy-input policy (stamped into metadata by the inbound router;
            // absent → Steer) decides what happens to this message. The run
            // was never registered, so there is nothing to undo on either
            // path.
            match super::BusyInputMode::from_metadata(&request.metadata) {
                super::BusyInputMode::Interrupt => {
                    // Cancel the running sibling on THIS session — AND any
                    // in-flight delegated children it owns — then let the inbound
                    // router's FIFO busy queue restart this message as a fresh run
                    // once the slot frees. The new run reads the interrupted
                    // task's full context from the session log plus this
                    // instruction. Reuses `cancel_session` + the `AgentBusy`
                    // delivery path; no new dispatch machinery (R10).
                    //
                    // A content-less request (resume-style continuation, or a
                    // synthetic run that lost the `try_start_run` race and only
                    // inherited an `Interrupt` busy-input key) must never cancel a
                    // healthy sibling: there is nothing new to contribute, so
                    // tearing down in-flight work is pure loss. Symmetric with the
                    // `Steer` empty-content guard, and the same intrinsic
                    // protection Hermes gives `internal` events. Likewise, when no
                    // same-session sibling is running (e.g. a cross-session busy),
                    // there is nothing to cancel — the message just waits its turn.
                    //
                    // A live sub-agent fan-out is no longer a special case. It
                    // used to force this Interrupt to *demote* to the follow-up
                    // queue, because a delegated fan-out was un-cancellable and
                    // cancelling the parent would detach its member runs (a token
                    // leak). Item 1 made delegated members cooperatively
                    // cancellable — `cancel_session` walks the tracker and fires
                    // each child's per-run token — so an Interrupt now cancels the
                    // whole session uniformly, exactly like `/stop`, rather than
                    // second-guessing the channel's declared `Interrupt` policy
                    // (R10: the gateway does not re-score in-flight work). Operators
                    // who want in-flight work preserved use the first-class `Queue`
                    // / `Steer` modes. (`/stop` and Panel `chat.abort` are explicit
                    // user-stops that already route straight through
                    // `cancel_session`, NOT this branch.)
                    let has_sibling = if !super::steering::has_steering_content(request) {
                        false
                    } else {
                        let runs = self.active_runs.read().await;
                        super::steering::find_steering_target_id(
                            &runs,
                            run_id,
                            &request.session_key,
                        )
                        .is_some()
                    };
                    if has_sibling {
                        // Cancel the leader's own run AND its delegated children
                        // (no detached-member leak). A bare `self.cancel(target)`
                        // would kill only the parent and re-open that leak. No
                        // interruption marker is persisted here: the harness
                        // bridge emits `RunFinished{Cancelled}` when each cancelled
                        // loop tears down, and the prompt builder replays it as a
                        // `<system-reminder>` interruption note (covers /stop,
                        // Panel chat.abort and this Interrupt mode alike) — single
                        // source, nothing stored twice.
                        let _ = self.cancel_session(&request.session_key).await;
                        info!(
                            session = %request.session_key.to_key_string(),
                            "busy-input interrupt: cancelled running sibling and any delegated children; message will restart as a fresh run via the busy queue",
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

        // Session slot claimed. Build the RAII `RunSlot` NOW — before acquiring
        // the concurrency permit — so the claim is released on EVERY exit path,
        // including a mid-`await` future drop. Previously the claim was a bare
        // `try_claim` insert until the `RunSlot` was constructed *after*
        // `concurrency.acquire().await`; a future dropped in that window (e.g.
        // runtime shutdown) leaked the claim and left the session permanently
        // busy — every later message on it hit the busy-input path forever.
        // The permit is slotted in once acquired.
        let mut slot = RunSlot {
            registry: Arc::clone(&self.session_run_registry),
            session_key: request.session_key.clone(),
            run_id: run_id.to_string(),
            _permit: None,
        };

        // Acquire a run-lifetime concurrency permit — try non-blocking first;
        // if both caps (global + per-agent) are currently full, queue (await)
        // rather than fail the run (audit 1.2). The wait is now visible via
        // `ConcurrencySnapshot::waiting`; if this future is dropped while
        // parked here, `slot` drops → the session claim is released (no leak).
        let agent_id = request.session_key.agent_id().to_string();
        slot._permit = Some(match self.concurrency.try_acquire(&agent_id) {
            Some(p) => p,
            None => self.concurrency.acquire(&agent_id).await,
        });

        // Register the run.
        {
            let mut runs = self.active_runs.write().await;
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

        Ok(GateOutcome::Admitted(slot))
    }
}

#[cfg(test)]
mod tests {
    use super::super::session_run_registry::SessionRunRegistry;
    use super::*;

    /// Regression: a `RunSlot` holding a session claim but **no permit yet**
    /// (`_permit: None`) — the exact state `admit_run` is in while parked on
    /// `ConcurrencyLimiter::acquire().await` — must still release the claim on
    /// drop. This is what turns a mid-await future drop from a permanent-busy
    /// leak into a clean release.
    #[test]
    fn run_slot_releases_session_claim_on_drop_without_permit() {
        let registry = Arc::new(SessionRunRegistry::default());
        let sk = SessionKey::peer("main", "conv-1");
        assert!(registry.try_claim(&sk, "run-1"));

        let slot = RunSlot {
            registry: Arc::clone(&registry),
            session_key: sk.clone(),
            run_id: "run-1".to_string(),
            _permit: None,
        };
        assert!(
            !registry.try_claim(&sk, "run-2"),
            "claim is held while the slot is alive"
        );

        drop(slot);
        assert!(
            registry.try_claim(&sk, "run-3"),
            "dropping the permit-less slot releases the session claim"
        );
    }
}
