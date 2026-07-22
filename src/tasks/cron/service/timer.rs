//! Timer loop and worker pool for cron job execution.
//!
//! The timer loop is the heartbeat of the cron service. It wakes up periodically,
//! finds due jobs (via phase1), executes them (via executor callback), and writes
//! back results (via phase3).

use std::collections::VecDeque;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use futures::FutureExt;
use tracing::{debug, error, info};

use crate::sync_primitives::Arc;

use crate::tasks::cron::config::{ExecutionResult, JobSnapshot, RunStatus, SessionTarget};
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

/// RAII guard that clears the `is_running` flag on drop.
///
/// Ensures the re-entrancy guard is released even if `on_timer_tick` panics.
struct RunningGuard<'a, C: Clock> {
    state: &'a Arc<ServiceState<C>>,
}

impl<'a, C: Clock> Drop for RunningGuard<'a, C> {
    fn drop(&mut self) {
        self.state.set_running(false);
    }
}

/// Run the timer loop until shutdown is requested.
///
/// Each iteration:
/// 1. Sleep for `check_interval_secs` (capped at 60s)
/// 2. Skip if `is_running` (re-entrancy guard)
/// 3. Call `on_timer_tick`
/// 4. Clear `is_running`
pub async fn run_timer_loop<C: Clock>(
    state: Arc<ServiceState<C>>,
    executor: JobExecutorFn,
    alert_dispatcher: Option<AlertDispatcherFn>,
) {
    let interval_secs = state.config.check_interval_secs.clamp(1, 60);

    info!(interval_secs, "cron timer loop started");

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;

        if state.is_shutdown() {
            info!("cron timer loop: shutdown requested, exiting");
            break;
        }

        // Re-entrancy guard: atomic test-and-set so two concurrent callers
        // can't both pass the check and double-run a tick.
        if !state.try_begin_running() {
            debug!("cron timer loop: previous tick still running, skipping");
            continue;
        }
        let _guard = RunningGuard { state: &state };

        if let Err(e) = on_timer_tick(&state, &executor, alert_dispatcher.as_ref()).await {
            error!(error = %e, "cron timer tick failed");
        }

        // _guard drops here, clearing is_running even if on_timer_tick panicked.
    }
}

/// Execute a single timer tick: mark due jobs, execute, writeback.
pub async fn on_timer_tick<C: Clock>(
    state: &Arc<ServiceState<C>>,
    executor: &JobExecutorFn,
    alert_dispatcher: Option<&AlertDispatcherFn>,
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

    if snapshots.is_empty() {
        return Ok(());
    }

    debug!(count = snapshots.len(), "timer tick: found due jobs");

    // Split by session target
    let mut main_jobs = Vec::new();
    let mut isolated_jobs = Vec::new();

    for snapshot in snapshots {
        match snapshot.session_target {
            SessionTarget::Main => main_jobs.push(snapshot),
            SessionTarget::Isolated => isolated_jobs.push(snapshot),
        }
    }

    let mut all_results: Vec<(String, ExecutionResult)> = Vec::new();

    // Main jobs: tokio::spawn each
    let mut main_handles = Vec::new();
    for snapshot in main_jobs {
        let executor = Arc::clone(executor);
        let id = snapshot.id.clone();
        let marked_at = snapshot.marked_at;
        let trigger_source = snapshot.trigger_source;
        let handle = tokio::spawn(async move {
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
                    agent_used_messaging_tool: false,
                    trigger_source,
                    retry_hint: None,
                },
            };
            (id, result)
        });
        main_handles.push(handle);
    }

    // Isolated jobs: worker pool
    let max_workers = state.config.max_concurrent_agents.unwrap_or(2).max(1);
    let isolated_results = run_worker_pool(isolated_jobs, max_workers, executor).await;
    all_results.extend(isolated_results);

    // Await main job handles
    for handle in main_handles {
        match handle.await {
            Ok((id, result)) => all_results.push((id, result)),
            Err(e) => error!(error = %e, "main job task panicked"),
        }
    }

    // Phase 3: writeback all results. The returned `PendingAlert`s must be
    // dispatched here — phase3 is intentionally I/O-free beyond SQLite.
    if !all_results.is_empty() {
        let notify_default = state.config.notify_on_failure_default;
        let alerts = phase3_writeback(
            &state.store,
            state.clock.as_ref(),
            &all_results,
            notify_default,
        )
        .await?;
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
    }

    Ok(())
}

/// Run a worker pool to process isolated jobs concurrently.
///
/// Spawns `max_workers` workers, each pulling from a shared queue.
/// Returns all `(job_id, result)` pairs.
pub async fn run_worker_pool(
    jobs: Vec<JobSnapshot>,
    max_workers: usize,
    executor: &JobExecutorFn,
) -> Vec<(String, ExecutionResult)> {
    if jobs.is_empty() {
        return Vec::new();
    }

    let worker_count = max_workers.min(jobs.len()).max(1);
    let queue = Arc::new(crate::sync_primitives::Mutex::new(VecDeque::from(jobs)));
    let results = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let mut handles = Vec::new();

    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let results = Arc::clone(&results);
        let executor = Arc::clone(executor);

        let handle = tokio::spawn(async move {
            loop {
                let snapshot = {
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.pop_front()
                };

                let snapshot = match snapshot {
                    Some(s) => s,
                    None => break,
                };

                let id = snapshot.id.clone();
                let marked_at = snapshot.marked_at;
                let trigger_source = snapshot.trigger_source;
                // Catch a panic so the worker survives to drain the rest of the
                // queue and the panicked job still flows to phase3 (which clears
                // its `running_at_ms`) instead of being stuck until restart.
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
                        agent_used_messaging_tool: false,
                        trigger_source,
                        retry_hint: None,
                    },
                };

                let mut r = results.lock().await;
                r.push((id, result));
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        if let Err(e) = handle.await {
            error!(error = %e, "cron worker task panicked");
        }
    }

    let guard = results.lock().await;
    guard.clone()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::config::{CronConfig, RunStatus, TriggerSource};

    fn mock_executor(status: RunStatus) -> JobExecutorFn {
        Arc::new(move |snapshot: JobSnapshot| {
            let status = status;
            Box::pin(async move {
                ExecutionResult {
                    started_at: snapshot.marked_at,
                    ended_at: snapshot.marked_at + 100,
                    duration_ms: 100,
                    status,
                    output: Some("test".to_string()),
                    error: None,
                    error_reason: None,
                    delivery_status: None,
                    agent_used_messaging_tool: false,
                    trigger_source: snapshot.trigger_source,
                    retry_hint: None,
                }
            })
        })
    }

    fn make_snapshot(id: &str) -> JobSnapshot {
        JobSnapshot {
            id: id.to_string(),
            agent_id: Some("test-agent".to_string()),
            source_channel_id: None,
            source_conversation_id: None,
            prompt: "test prompt".to_string(),
            model: None,
            timeout_ms: Some(300_000),
            delivery: None,
            session_target: SessionTarget::Isolated,
            marked_at: 1_000_000,
            trigger_source: TriggerSource::Schedule,
        }
    }

    #[tokio::test]
    async fn worker_pool_processes_all_jobs() {
        let jobs: Vec<JobSnapshot> = (0..5).map(|i| make_snapshot(&format!("job-{i}"))).collect();

        let executor = mock_executor(RunStatus::Ok);
        let results = run_worker_pool(jobs, 2, &executor).await;

        assert_eq!(results.len(), 5, "all 5 jobs should be processed");

        // Verify all job IDs are present
        let mut ids: Vec<String> = results.iter().map(|(id, _)| id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["job-0", "job-1", "job-2", "job-3", "job-4"]);

        // Verify all results have Ok status
        for (_, result) in &results {
            assert_eq!(result.status, RunStatus::Ok);
        }
    }

    #[tokio::test]
    async fn worker_pool_empty_input() {
        let executor = mock_executor(RunStatus::Ok);
        let results = run_worker_pool(Vec::new(), 2, &executor).await;
        assert!(results.is_empty(), "empty input should yield empty output");
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
