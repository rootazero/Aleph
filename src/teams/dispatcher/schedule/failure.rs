//! Failure handling: bounded retry (delegating to the pure `tasks::retry`
//! decision) and terminal fail.

use std::time::Duration;

use crate::agents::swarm::tasks::retry::{
    jittered_backoff_secs, read_max_retries, retry_decision, with_retry_not_before, RetryDecision,
};
use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskUpdate, TaskRunStatus};
use crate::sync_primitives::Arc;

use super::TeamDispatcher;

impl TeamDispatcher {
    /// Decide a failed/timed-out attempt's fate: **bounded automatic retry**, or
    /// a terminal `Failed` (the doc's `FailedFinal`).
    ///
    /// Counts the failed/timed-out attempts already recorded for the task — the
    /// just-failed attempt is recorded (`finish_task_run`) *before* this runs —
    /// against the task's retry ceiling (`max_retries` in metadata, else the
    /// dispatcher's [`default_max_retries`](super::DispatcherConfig::default_max_retries)).
    ///
    /// - **Under the ceiling** → reset to `Pending`; the next tick re-claims it
    ///   and [`build_handoff_context`](super::handoff::build_handoff_context)
    ///   surfaces the prior attempts as recovery context, so the resuming member
    ///   resumes instead of cold-starting. The retry *count* is the only
    ///   mechanical decision here; *how* to recover is the model's call via the
    ///   injected prompt (R7/R9/R10).
    /// - **At/over the ceiling** → [`fail_task`](Self::fail_task) → terminal
    ///   `Failed`.
    ///
    /// Orphan reclaims leave a `Running` row that never finished, so they do not
    /// consume the retry budget — only clean `Failed`/`Timeout` attempts do.
    /// Zombies bypass this entirely and go straight to `fail_task` (they have
    /// already exhausted their runtime budget; retrying would just re-zombify).
    ///
    /// Cancelled stays sticky on both paths — a cancel issued mid-flight is
    /// neither retried nor overwritten with a failure.
    pub(super) async fn fail_or_retry(&self, task: &CoordTask, error: &str) {
        // One fresh fetch serves two guards. (1) Terminal-sticky: a task an
        // operator moved to ANY terminal state mid-flight (cancel, skip,
        // manual complete — not just Cancelled) is neither retried nor
        // overwritten with a failure. (2) Fresh-basis stamp: the retry
        // metadata below is based on the CURRENT row, not the claim-time
        // snapshot, so mid-run edits (max_retries / timeout_secs raised via
        // task_update while the attempt ran) survive the failure write-back
        // instead of being reverted to the dispatch-time values.
        let fresh = self.coord_store.get_task(&task.id).await.ok().flatten();
        if let Some(t) = &fresh {
            if t.status.is_terminal() {
                tracing::info!(task_id = %task.id, status = %t.status, "dispatcher: task already terminal; neither retrying nor failing");
                return;
            }
        }
        let base_metadata = fresh
            .map(|t| t.metadata)
            .unwrap_or_else(|| task.metadata.clone());

        let max_retries =
            read_max_retries(&base_metadata).unwrap_or(self.config.default_max_retries);
        let failed_attempts = self
            .coord_store
            .list_task_runs(&task.id)
            .await
            .map(|runs| {
                runs.iter()
                    .filter(|r| matches!(r.status, TaskRunStatus::Failed | TaskRunStatus::Timeout))
                    .count() as u32
            })
            .unwrap_or(0);

        match retry_decision(failed_attempts, max_retries) {
            RetryDecision::Retry => {
                // Exponential backoff (with jitter): stamp the earliest epoch
                // this task may be re-claimed into metadata, so the scheduler's
                // eligibility gate spaces the next attempt out instead of
                // re-claiming on the next tick. `0` backoff → immediate (the
                // pre-enhancement behaviour). The jitter seed is a hash of the
                // task id — deterministic and RNG-free, but distinct per task so
                // a team whose tasks failed together don't all retry in lockstep
                // and re-stampede the recovering provider (thundering herd).
                let seed = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    task.id.hash(&mut h);
                    h.finish()
                };
                let backoff = jittered_backoff_secs(
                    failed_attempts,
                    self.config.retry_backoff_base_secs,
                    self.config.retry_backoff_cap_secs,
                    seed,
                );
                let not_before = Self::now_epoch().saturating_add(backoff);
                let metadata = with_retry_not_before(base_metadata, not_before);
                tracing::info!(
                    task_id = %task.id,
                    attempt = failed_attempts,
                    max_retries,
                    backoff_secs = backoff,
                    "dispatcher: task attempt failed; scheduling retry"
                );
                // Reset to Pending so a later tick re-dispatches. Surfacing the
                // last error as the (transient) result keeps the panel honest
                // until the retry overwrites it; the durable recovery context is
                // the run log + exit journal, assembled at hand-off time.
                if let Err(e) = self
                    .coord_store
                    .update_task(
                        &task.id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Pending),
                            result: Some(format!(
                                "retry {failed_attempts}/{max_retries} in {backoff}s after: {error}"
                            )),
                            metadata: Some(metadata),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(task_id = %task.id, error = %e, "dispatcher: failed to reset task for retry; marking failed");
                    self.fail_task(task, error).await;
                } else if backoff > 0 {
                    // Precise wake so a short backoff isn't stranded until the
                    // (minute-scale) fallback tick. A detached timer that fires a
                    // single signal is cheap and idiomatic — leaning on Tokio
                    // rather than busy-polling the deadline.
                    let signal = Arc::clone(&self.signal);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(backoff)).await;
                        signal.notify_one();
                    });
                }
            }
            RetryDecision::GiveUp => self.fail_task(task, error).await,
        }
    }

    /// Mark a task terminally `Failed` (`FailedFinal`). The
    /// `AlephEvent::TeamTaskFailed` broadcast is emitted by
    /// [`CoordTaskStore::emit_task_topic`] inside `update_task`, so panel-driven
    /// failure (drawer "Fail" button) gets the same listener fan-out without
    /// per-caller wiring.
    ///
    /// `pub(in crate::teams::dispatcher)` so the clarify executor
    /// ([`crate::teams::dispatcher::clarify`]) can terminate an unanswerable
    /// clarification with the same path. Reached from
    /// [`fail_or_retry`](Self::fail_or_retry) once the retry budget is spent, and
    /// directly from zombie reclamation (which never retries).
    ///
    /// Terminal states are sticky: if the task reached ANY terminal state
    /// since the caller's snapshot was taken (cancelled, skipped, manually
    /// completed), the failure is NOT written over it — the attempt is
    /// already recorded in run history and an externally-decided outcome
    /// must stand.
    pub(in crate::teams::dispatcher) async fn fail_task(&self, task: &CoordTask, error: &str) {
        if matches!(
            self.coord_store.get_task(&task.id).await,
            Ok(Some(t)) if t.status.is_terminal()
        ) {
            tracing::info!(task_id = %task.id, "dispatcher: not overwriting terminal task with failure");
            return;
        }
        if let Err(e) = self
            .coord_store
            .update_task(
                &task.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    result: Some(error.to_string()),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e, "dispatcher: failed to persist task failure state");
        }
    }
}
