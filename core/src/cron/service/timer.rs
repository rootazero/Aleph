//! Timer loop and worker pool for cron job execution.
//!
//! The timer loop is the heartbeat of the cron service. It wakes up periodically,
//! finds due jobs (via phase1), executes them (via executor callback), and writes
//! back results (via phase3).

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::{debug, error, info};

use crate::cron::clock::Clock;
use crate::cron::config::{ExecutionResult, JobSnapshot, SessionTarget};
use crate::cron::service::concurrency::{phase1_mark_due_jobs, phase3_writeback};
use crate::cron::service::state::ServiceState;

/// Executor function type: takes a snapshot and returns an execution result.
pub type JobExecutorFn = Arc<
    dyn Fn(JobSnapshot) -> Pin<Box<dyn Future<Output = ExecutionResult> + Send>> + Send + Sync,
>;

/// Run the timer loop until shutdown is requested.
///
/// Each iteration:
/// 1. Sleep for `check_interval_secs` (capped at 60s)
/// 2. Skip if `is_running` (re-entrancy guard)
/// 3. Call `on_timer_tick`
/// 4. Clear `is_running`
pub async fn run_timer_loop<C: Clock>(state: Arc<ServiceState<C>>, executor: JobExecutorFn) {
    let interval_secs = state.config.check_interval_secs.min(60);

    info!(interval_secs, "cron timer loop started");

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;

        if state.is_shutdown() {
            info!("cron timer loop: shutdown requested, exiting");
            break;
        }

        // Re-entrancy guard
        if state.is_running() {
            debug!("cron timer loop: previous tick still running, skipping");
            continue;
        }

        state.set_running(true);

        if let Err(e) = on_timer_tick(&state, &executor).await {
            error!(error = %e, "cron timer tick failed");
        }

        state.set_running(false);
    }
}

/// Execute a single timer tick: mark due jobs, execute, writeback.
pub async fn on_timer_tick<C: Clock>(
    state: &Arc<ServiceState<C>>,
    executor: &JobExecutorFn,
) -> Result<(), String> {
    // Phase 1: mark due jobs
    let snapshots = phase1_mark_due_jobs(&state.store, state.clock.as_ref()).await?;

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
        let handle = tokio::spawn(async move {
            let result = executor(snapshot).await;
            (id, result)
        });
        main_handles.push(handle);
    }

    // Isolated jobs: worker pool
    let max_workers = state
        .config
        .max_concurrent_agents
        .unwrap_or(2)
        .max(1);
    let isolated_results = run_worker_pool(isolated_jobs, max_workers, executor).await;
    all_results.extend(isolated_results);

    // Await main job handles
    for handle in main_handles {
        match handle.await {
            Ok((id, result)) => all_results.push((id, result)),
            Err(e) => error!(error = %e, "main job task panicked"),
        }
    }

    // Phase 3: writeback all results
    if !all_results.is_empty() {
        phase3_writeback(&state.store, state.clock.as_ref(), &all_results).await?;
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
    let queue = Arc::new(std::sync::Mutex::new(VecDeque::from(jobs)));
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
                let result = executor(snapshot).await;

                let mut r = results.lock().await;
                r.push((id, result));
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    let guard = results.lock().await;
    guard.clone()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::{RunStatus, TriggerSource};

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
                }
            })
        })
    }

    fn make_snapshot(id: &str) -> JobSnapshot {
        JobSnapshot {
            id: id.to_string(),
            agent_id: Some("test-agent".to_string()),
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
}
