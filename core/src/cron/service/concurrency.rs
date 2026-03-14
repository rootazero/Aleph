//! Three-phase concurrency model for cron job execution.
//!
//! - **Phase 1**: Mark due jobs as running (lock, snapshot, persist, release)
//! - **Phase 2**: Execute jobs (handled externally, no lock held)
//! - **Phase 3**: Write back results (lock, reload, update, persist, release)
//!
//! This model minimizes lock hold time — the lock is only held during
//! brief metadata updates, never during actual job execution.

use std::sync::Arc;

use tracing::warn;

use crate::cron::clock::Clock;
use crate::cron::config::{ExecutionResult, JobSnapshot, RunStatus, TriggerSource};
use crate::cron::store::CronStore;

use super::ops::recompute_next_run_maintenance;

/// Phase 1: Mark due jobs and return snapshots for execution.
///
/// Locks the store, finds all enabled jobs that are:
/// - Not already running (`running_at_ms == None`)
/// - Past due (`next_run_at_ms <= now`)
///
/// For each due job, sets `running_at_ms = now` and creates a `JobSnapshot`.
/// Persists and releases the lock.
pub async fn phase1_mark_due_jobs<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
) -> Result<Vec<JobSnapshot>, String> {
    let now = clock.now_ms();
    let mut guard = store.lock().await;

    guard.reload_if_changed()?;

    let mut snapshots = Vec::new();

    for job in guard.jobs_mut().iter_mut() {
        if !job.enabled {
            continue;
        }
        if job.state.running_at_ms.is_some() {
            continue;
        }
        let next = match job.state.next_run_at_ms {
            Some(t) if t <= now => t,
            _ => continue,
        };
        let _ = next;

        // Mark as running
        job.state.running_at_ms = Some(now);

        snapshots.push(JobSnapshot {
            id: job.id.clone(),
            agent_id: Some(job.agent_id.clone()),
            prompt: job.prompt.clone(),
            model: None,
            timeout_ms: Some(job.timeout_ms()),
            delivery: job.delivery_config.clone(),
            session_target: job.session_target.clone(),
            marked_at: now,
            trigger_source: TriggerSource::Schedule,
        });
    }

    if !snapshots.is_empty() {
        guard.persist()?;
    }

    Ok(snapshots)
}

/// Phase 1 (manual): Mark a specific job for manual execution.
///
/// Returns `None` if the job is already running.
pub async fn phase1_mark_manual<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    job_id: &str,
) -> Result<Option<JobSnapshot>, String> {
    let now = clock.now_ms();
    let mut guard = store.lock().await;

    guard.reload_if_changed()?;

    let job = guard
        .get_job_mut(job_id)
        .ok_or_else(|| format!("job not found: {job_id}"))?;

    if job.state.running_at_ms.is_some() {
        return Ok(None); // Already running
    }

    job.state.running_at_ms = Some(now);

    let snapshot = JobSnapshot {
        id: job.id.clone(),
        agent_id: Some(job.agent_id.clone()),
        prompt: job.prompt.clone(),
        model: None,
        timeout_ms: Some(job.timeout_ms()),
        delivery: job.delivery_config.clone(),
        session_target: job.session_target.clone(),
        marked_at: now,
        trigger_source: TriggerSource::Manual,
    };

    guard.persist()?;

    Ok(Some(snapshot))
}

/// Phase 3: Write back execution results after jobs complete.
///
/// Locks the store, force-reloads to capture concurrent edits,
/// then for each result:
/// - Clears `running_at_ms`
/// - Writes execution result fields (status, error, duration, etc.)
/// - Resets or increments `consecutive_errors`
/// - Clears `next_run_at_ms` so maintenance recompute fills it
///
/// After processing all results, runs maintenance recompute on ALL jobs
/// and persists.
pub async fn phase3_writeback<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    results: &[(String, ExecutionResult)],
) -> Result<(), String> {
    let mut guard = store.lock().await;

    guard.force_reload()?;

    for (job_id, result) in results {
        let job = match guard.get_job_mut(job_id) {
            Some(j) => j,
            None => {
                warn!("phase3: job '{job_id}' was deleted during execution, discarding result");
                continue;
            }
        };

        // Clear running state
        job.state.running_at_ms = None;

        // Write execution result
        job.state.last_run_at_ms = Some(result.started_at);
        job.state.last_run_status = Some(result.status);
        job.state.last_duration_ms = Some(result.duration_ms);
        job.state.last_error = result.error.clone();
        job.state.last_error_reason = result.error_reason.clone();
        job.state.last_delivery_status = result.delivery_status;

        // Update consecutive errors
        match result.status {
            RunStatus::Ok | RunStatus::Skipped => {
                job.state.consecutive_errors = 0;
            }
            RunStatus::Error | RunStatus::Timeout => {
                job.state.consecutive_errors += 1;
            }
        }

        // Clear next_run_at_ms so maintenance recompute fills it
        job.state.next_run_at_ms = None;
    }

    // Maintenance recompute ALL jobs
    let job_count = guard.jobs().len();
    for i in 0..job_count {
        // We need to work with indices because recompute needs &mut job and clock
        let jobs = guard.jobs_mut();
        recompute_next_run_maintenance(&mut jobs[i], clock);
    }

    guard.persist()?;

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use crate::cron::config::{CronJob, RunStatus, ScheduleKind};
    use crate::cron::service::ops::add_job;
    use tempfile::TempDir;

    fn make_test_job(id: &str) -> CronJob {
        let mut job = CronJob::new(
            id.to_string(),
            "test-agent".to_string(),
            "test prompt".to_string(),
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.id = id.to_string();
        job
    }

    fn make_store() -> (Arc<tokio::sync::Mutex<CronStore>>, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");
        let store = CronStore::load(path).unwrap();
        (Arc::new(tokio::sync::Mutex::new(store)), dir)
    }

    fn make_execution_result(status: RunStatus) -> ExecutionResult {
        ExecutionResult {
            started_at: 1_000_000,
            ended_at: 1_005_000,
            duration_ms: 5_000,
            status,
            output: Some("done".to_string()),
            error: if status == RunStatus::Error {
                Some("test error".to_string())
            } else {
                None
            },
            error_reason: None,
            delivery_status: None,
            agent_used_messaging_tool: false,
        }
    }

    #[tokio::test]
    async fn phase1_marks_due_jobs() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);

        // Add a job with next_run in the past (due)
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("due-job");
            job.created_at = 900_000;
            let id = add_job(&mut guard, job, &clock);
            // Manually set next_run to past
            let j = guard.get_job_mut(&id).unwrap();
            j.state.next_run_at_ms = Some(950_000);
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock).await.unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "due-job");
        assert_eq!(snapshots[0].trigger_source, TriggerSource::Schedule);

        // Verify running_at_ms was set
        let guard = store.lock().await;
        let job = guard.get_job("due-job").unwrap();
        assert_eq!(job.state.running_at_ms, Some(1_000_000));
    }

    #[tokio::test]
    async fn phase1_skips_already_running() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("running-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("running-job").unwrap();
            j.state.next_run_at_ms = Some(950_000);
            j.state.running_at_ms = Some(990_000); // already running
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock).await.unwrap();
        assert!(snapshots.is_empty(), "running jobs should be skipped");
    }

    #[tokio::test]
    async fn phase3_merges_results() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);

        // Set up a running job
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("completed-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("completed-job").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.consecutive_errors = 3; // had errors before
            guard.persist().unwrap();
        }

        let result = make_execution_result(RunStatus::Ok);
        let results = vec![("completed-job".to_string(), result)];

        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("completed-job").unwrap();
        assert!(job.state.running_at_ms.is_none(), "running_at_ms should be cleared");
        assert_eq!(job.state.last_run_status, Some(RunStatus::Ok));
        assert_eq!(job.state.consecutive_errors, 0, "errors should reset on Ok");
        assert!(
            job.state.next_run_at_ms.is_some(),
            "next_run should be recomputed by maintenance"
        );
    }

    #[tokio::test]
    async fn phase3_handles_deleted_job() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);

        // Don't add any job — simulate deletion during execution
        let result = make_execution_result(RunStatus::Ok);
        let results = vec![("deleted-job".to_string(), result)];

        // Should not error, just warn
        let outcome = phase3_writeback(&store, &clock, &results).await;
        assert!(outcome.is_ok(), "should gracefully handle deleted jobs");
    }

    #[tokio::test]
    async fn phase3_increments_consecutive_errors() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("error-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("error-job").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.consecutive_errors = 2; // had 2 errors
            guard.persist().unwrap();
        }

        let result = make_execution_result(RunStatus::Error);
        let results = vec![("error-job".to_string(), result)];

        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("error-job").unwrap();
        assert_eq!(
            job.state.consecutive_errors, 3,
            "consecutive_errors should increment on Error"
        );
    }

    #[tokio::test]
    async fn phase1_mark_manual_creates_snapshot() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("manual-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            guard.persist().unwrap();
        }

        let snapshot = phase1_mark_manual(&store, &clock, "manual-job")
            .await
            .unwrap();
        assert!(snapshot.is_some());
        let snap = snapshot.unwrap();
        assert_eq!(snap.id, "manual-job");
        assert_eq!(snap.trigger_source, TriggerSource::Manual);

        // Verify running_at_ms was set
        let guard = store.lock().await;
        let job = guard.get_job("manual-job").unwrap();
        assert_eq!(job.state.running_at_ms, Some(1_000_000));
    }

    #[tokio::test]
    async fn phase1_mark_manual_returns_none_if_running() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("busy-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("busy-job").unwrap();
            j.state.running_at_ms = Some(990_000);
            guard.persist().unwrap();
        }

        let snapshot = phase1_mark_manual(&store, &clock, "busy-job")
            .await
            .unwrap();
        assert!(snapshot.is_none(), "should return None for already-running job");
    }
}
