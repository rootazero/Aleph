//! Timer loop for cron job execution.
//!
//! The timer loop is the heartbeat of the cron service. It wakes up
//! periodically, marks due jobs (phase1), and hands each one to a **detached**
//! task that executes it and writes the result back (phase3).
//!
//! ## Why every job is detached
//!
//! This loop used to `await` the whole batch inside the tick, behind a
//! process-wide re-entrancy flag that skipped every wake-up while a tick was in
//! flight. With `job_timeout_secs` defaulting to 900, one slow job froze the
//! *due-scan itself* for up to 15 minutes: a one-minute monitor did not queue
//! up behind it, it simply was never looked at, and `advance_next_run` only
//! computes the next occurrence strictly after `now`, so those firings were
//! lost with no error, no log and no counter.
//!
//! The re-entrancy flag was never the thing protecting the invariant anyway —
//! `phase1_mark_due_jobs` skips any job whose `running_at_ms` is set, and
//! `phase3_writeback` re-reads the store before writing its own batch, so two
//! overlapping ticks were already safe at job granularity. The flag has been
//! deleted rather than kept as a second source of truth.
//!
//! The shape here is copied from `tasks::heartbeat::service::timer`, which
//! solved the same problem in the twin subsystem ("a slow L2 turn never blocks
//! the timer loop"): permits come from one semaphore shared across ticks and
//! are acquired *inside* the spawned future, so the loop returns to its sleep
//! immediately.

use futures::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

use crate::sync_primitives::Arc;

use crate::tasks::cron::config::{ExecutionResult, JobSnapshot, RunStatus};
use crate::tasks::cron::service::concurrency::{
    phase1_mark_due_jobs, phase3_writeback, PendingAlert,
};
use crate::tasks::cron::service::state::ServiceState;
use crate::tasks::shared::clock::Clock;

/// Executor function type: takes a snapshot and returns an execution result.
pub type JobExecutorFn =
    Arc<dyn Fn(JobSnapshot) -> Pin<Box<dyn Future<Output = ExecutionResult> + Send>> + Send + Sync>;

/// Dispatcher for failure alerts produced by `phase3_writeback`.
///
/// Wired by the daemon's startup glue (`build_cron_alert_dispatcher_fn`) so
/// `PendingAlert`s actually reach the delivery pipeline — without this hook
/// the alerts are dropped on the floor, which is the pre-D4 silent-failure
/// bug. Callers without a dispatcher (e.g. unit tests for the timer loop)
/// pass `None`.
pub type AlertDispatcherFn =
    Arc<dyn Fn(Vec<PendingAlert>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Emitter for `CronJobChanged` (state) frames after a scheduler-tick
/// writeback.
///
/// The webchat panel dropped polling and relies on `cron.job.changed` push;
/// without this, scheduled runs completing silently leave the panel stale
/// (`last_run_at_ms`/`last_run_status` change but nothing is published).
/// Wired at startup from the `CronService`'s event bus. Takes the job id.
pub type ChangeEmitterFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Run the timer loop until shutdown is requested.
///
/// Each iteration sleeps for `check_interval_secs` (clamped to 1..=60), marks
/// the due jobs, and spawns one detached task per job. The loop itself never
/// awaits job execution — see the module docs for why.
pub async fn run_timer_loop<C: Clock>(
    state: Arc<ServiceState<C>>,
    executor: JobExecutorFn,
    alert_dispatcher: Option<AlertDispatcherFn>,
    change_emitter: Option<ChangeEmitterFn>,
) {
    let interval_secs = state.config.check_interval_secs.clamp(1, 60);
    // One semaphore for the whole loop, not one per tick: the cap is
    // "concurrent agent sessions spawned by cron" (its config doc), which is a
    // property of the process, not of a batch. Acquired inside each spawned
    // future so the loop is never the thing waiting for it.
    let permits = state.config.max_concurrent_agents.unwrap_or(2).max(1);
    let semaphore = Arc::new(Semaphore::new(permits));

    info!(
        interval_secs,
        max_concurrent_agents = permits,
        "cron timer loop started"
    );

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;

        if state.is_shutdown() {
            info!("cron timer loop: shutdown requested, exiting");
            break;
        }

        if let Err(e) = on_timer_tick(
            &state,
            &executor,
            &semaphore,
            alert_dispatcher.as_ref(),
            change_emitter.as_ref(),
        )
        .await
        {
            error!(error = %e, "cron timer tick failed");
        }
    }
}

/// Mark the due jobs and dispatch each to its own detached task.
///
/// Returns as soon as the jobs are spawned; the `Err` arm covers only the
/// phase1 store failure, which is the one thing that can go wrong before any
/// job leaves this function.
pub async fn on_timer_tick<C: Clock>(
    state: &Arc<ServiceState<C>>,
    executor: &JobExecutorFn,
    semaphore: &Arc<Semaphore>,
    alert_dispatcher: Option<&AlertDispatcherFn>,
    change_emitter: Option<&ChangeEmitterFn>,
) -> Result<(), String> {
    // Phase 1: mark due jobs. The configured per-job timeout is threaded
    // into each snapshot so it reaches the executor (C5).
    let default_timeout_ms = state
        .config
        .job_timeout_secs
        .saturating_mul(1000)
        .min(i64::MAX as u64) as i64;
    let snapshots =
        phase1_mark_due_jobs(&state.store, state.clock.as_ref(), default_timeout_ms).await?;

    // Stamp the scan *after* it succeeded and regardless of whether anything
    // was due — "the scheduler looked" is the fact `cron.status` needs, and an
    // idle scheduler is still a live one.
    state.mark_tick(state.clock.now_ms());

    if snapshots.is_empty() {
        return Ok(());
    }

    debug!(count = snapshots.len(), "timer tick: found due jobs");

    for snapshot in snapshots {
        let state = Arc::clone(state);
        let executor = Arc::clone(executor);
        let semaphore = Arc::clone(semaphore);
        let alert_dispatcher = alert_dispatcher.map(Arc::clone);
        let change_emitter = change_emitter.map(Arc::clone);
        tokio::spawn(async move {
            let job_id = snapshot.id.clone();
            let _permit = match semaphore.acquire_owned().await {
                Ok(p) => p,
                Err(e) => {
                    // Phase 1 already stamped `running_at_ms`; bailing without
                    // clearing it would make phase1 skip this job forever. The
                    // cheapest honest clear is an empty writeback through the
                    // same path every other outcome takes.
                    error!(
                        error = %e, job_id = %job_id,
                        "cron: semaphore closed; clearing running marker"
                    );
                    clear_running_marker(&state, &job_id).await;
                    return;
                }
            };

            let marked_at = snapshot.marked_at;
            let trigger_source = snapshot.trigger_source;
            // Catch a panic in the executor so the (id, result) contract stays
            // intact — otherwise phase3 never clears this job's `running_at_ms`
            // and phase1 skips it forever (until daemon restart).
            let result = match AssertUnwindSafe(executor(snapshot)).catch_unwind().await {
                Ok(r) => r,
                Err(_) => ExecutionResult {
                    started_at: marked_at,
                    ended_at: marked_at,
                    duration_ms: 0,
                    status: RunStatus::Error,
                    output: None,
                    error: Some("cron job task panicked".to_string()),
                    error_reason: None,
                    delivery_status: None,
                    trigger_source,
                    retry_hint: None,
                },
            };

            writeback_one(
                &state,
                &job_id,
                result,
                alert_dispatcher.as_ref(),
                change_emitter.as_ref(),
            )
            .await;
        });
    }

    Ok(())
}

/// Phase 3 for a single finished job, plus its alert dispatch and change frame.
///
/// `phase3_writeback` takes a slice and is serialised by the store mutex, so a
/// one-element batch is the same operation the old whole-tick batch performed —
/// only now it happens when *this* job finishes instead of when the slowest one
/// does. The maintenance recompute inside it only fills still-missing
/// `next_run_at_ms` values, so running it per job is idempotent.
async fn writeback_one<C: Clock>(
    state: &Arc<ServiceState<C>>,
    job_id: &str,
    result: ExecutionResult,
    alert_dispatcher: Option<&AlertDispatcherFn>,
    change_emitter: Option<&ChangeEmitterFn>,
) {
    let batch = [(job_id.to_string(), result)];
    let alerts = match phase3_writeback(
        &state.store,
        state.clock.as_ref(),
        &batch,
        state.config.notify_on_failure_default,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            error!(error = %e, job_id = %job_id, "cron: phase3 writeback failed");
            return;
        }
    };

    if !alerts.is_empty() {
        if let Some(disp) = alert_dispatcher {
            disp(alerts).await;
        } else {
            debug!(
                count = alerts.len(),
                "cron: failure alerts produced but no dispatcher wired; dropping"
            );
        }
    }

    // Push a `StateChanged` frame so the panel (which dropped polling)
    // refreshes `last_run_at_ms` etc. CRUD ops emit their own frames via
    // `CronService`; this covers the scheduler-tick path only.
    if let Some(emitter) = change_emitter {
        emitter(job_id);
    }
}

/// Release a job that was marked due but never executed.
///
/// Mirrors `heartbeat::service::timer::clear_running_marker`: a marker left
/// behind by a job that never started is invisible to `parked` and to
/// `enabled`, so the job simply stops firing until the next daemon restart.
async fn clear_running_marker<C: Clock>(state: &Arc<ServiceState<C>>, job_id: &str) {
    let mut guard = state.store.lock().await;
    if let Some(job) = guard.get_job_mut(job_id) {
        job.state.running_at_ms = None;
    }
    if let Err(e) = guard.persist() {
        warn!(error = %e, job_id = %job_id, "cron: failed to persist cleared running marker");
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::config::{CronConfig, CronJob, ScheduleKind};
    use crate::tasks::cron::service::ops::add_job;
    use crate::tasks::cron::store::CronStore;
    use crate::tasks::shared::clock::testing::FakeClock;
    use tempfile::TempDir;

    fn ok_result(snapshot: &JobSnapshot) -> ExecutionResult {
        ExecutionResult {
            started_at: snapshot.marked_at,
            ended_at: snapshot.marked_at + 100,
            duration_ms: 100,
            status: RunStatus::Ok,
            output: Some("test".to_string()),
            error: None,
            error_reason: None,
            delivery_status: None,
            trigger_source: snapshot.trigger_source,
            retry_hint: None,
        }
    }

    /// Seed one due job per id and return the shared state.
    async fn state_with_due_jobs(ids: &[&str]) -> (Arc<ServiceState<FakeClock>>, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = CronStore::load(dir.path().join("cron.db")).unwrap();
        let store = Arc::new(tokio::sync::Mutex::new(store));
        let clock = Arc::new(FakeClock::new(1_000_000));
        {
            let mut guard = store.lock().await;
            for id in ids {
                let mut job = CronJob::new(
                    (*id).to_string(),
                    "test-agent".to_string(),
                    "test prompt".to_string(),
                    ScheduleKind::Every {
                        every_ms: 60_000,
                        anchor_ms: None,
                    },
                );
                job.id = (*id).to_string();
                job.created_at = 900_000;
                let added = add_job(&mut guard, job, clock.as_ref());
                guard.get_job_mut(&added).unwrap().state.next_run_at_ms = Some(950_000);
            }
            guard.persist().unwrap();
        }
        let state = Arc::new(ServiceState::new(store, clock, CronConfig::default()));
        (state, dir)
    }

    /// The regression this whole restructure exists for: a job that takes a
    /// long time must not delay another due job's writeback. Awaiting the
    /// batch inside the tick (the pre-2026-08-09 shape) makes this hang for
    /// the slow job's full duration.
    #[tokio::test]
    async fn a_slow_job_does_not_delay_another_jobs_writeback() {
        let (state, _dir) = state_with_due_jobs(&["slow-job", "fast-job"]).await;

        let executor: JobExecutorFn = Arc::new(|snapshot: JobSnapshot| {
            Box::pin(async move {
                if snapshot.id == "slow-job" {
                    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                }
                ok_result(&snapshot)
            })
        });

        // Two permits so the fast job is not merely queued behind the slow one.
        let semaphore = Arc::new(Semaphore::new(2));
        on_timer_tick(&state, &executor, &semaphore, None, None)
            .await
            .expect("tick should return as soon as the jobs are spawned");

        // Poll for the fast job's writeback with a deadline far below the slow
        // job's 30s sleep: under the old shape nothing lands until it finishes.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        loop {
            {
                let guard = state.store.lock().await;
                let fast = guard.get_job("fast-job").expect("fast job present");
                if fast.state.last_run_status.is_some() {
                    assert!(
                        fast.state.running_at_ms.is_none(),
                        "writeback must clear the running marker"
                    );
                    break;
                }
                let slow = guard.get_job("slow-job").expect("slow job present");
                assert!(
                    slow.state.last_run_status.is_none(),
                    "the slow job is expected to still be in flight"
                );
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "fast job's writeback never landed while a slow job was running"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_tick_stamps_the_scan_even_when_nothing_is_due() {
        let (state, _dir) = state_with_due_jobs(&[]).await;
        assert_eq!(state.last_tick_at_ms(), 0);

        let executor: JobExecutorFn =
            Arc::new(|snapshot: JobSnapshot| Box::pin(async move { ok_result(&snapshot) }));
        let semaphore = Arc::new(Semaphore::new(1));
        on_timer_tick(&state, &executor, &semaphore, None, None)
            .await
            .unwrap();

        assert_eq!(
            state.last_tick_at_ms(),
            1_000_000,
            "an idle scheduler is still a live one — `cron.status` needs to say so"
        );
    }

    #[test]
    fn cron_check_interval_is_clamped_at_loop_entry() {
        let cfg = |s: u64| CronConfig {
            check_interval_secs: s,
            ..Default::default()
        };
        assert_eq!(
            cfg(0).check_interval_secs.clamp(1, 60),
            1,
            "0 must clamp to 1, not spin"
        );
        assert_eq!(cfg(1).check_interval_secs.clamp(1, 60), 1);
        assert_eq!(cfg(30).check_interval_secs.clamp(1, 60), 30);
        assert_eq!(cfg(60).check_interval_secs.clamp(1, 60), 60);
        assert_eq!(
            cfg(1_000).check_interval_secs.clamp(1, 60),
            60,
            "absurd interval must clamp to 60"
        );
    }
}
