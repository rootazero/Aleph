//! Three-phase concurrency model for cron job execution.
//!
//! - **Phase 1**: Mark due jobs as running (lock, snapshot, persist, release)
//! - **Phase 2**: Execute jobs (handled externally, no lock held)
//! - **Phase 3**: Write back results (lock, reload, update, persist, release)
//!
//! This model minimizes lock hold time — the lock is only held during
//! brief metadata updates, never during actual job execution.

use tracing::warn;

use crate::sync_primitives::Arc;

use crate::tasks::cron::chain;
use crate::tasks::cron::config::{
    ExecutionResult, JobSnapshot, RunStatus, ScheduleKind, TriggerSource,
};
use crate::tasks::cron::history::CronRunRecord;
use crate::tasks::cron::store::CronStore;
use crate::tasks::cron::template::resolve_job_prompt;
use crate::tasks::shared::clock::Clock;
use crate::tasks::shared::schedule::compute_backoff_ms;

use super::ops::{advance_next_run, recompute_next_run_maintenance};

/// Phase 1: Mark due jobs and return snapshots for execution.
///
/// Locks the store, finds all enabled jobs that are:
/// - Not already running (`running_at_ms == None`)
/// - Past due (`next_run_at_ms <= now`)
///
/// For each due job, sets `running_at_ms = now`, **advances the schedule to
/// the next future occurrence** (at-most-once), and creates a `JobSnapshot`.
/// Persists and releases the lock.
///
/// `default_timeout_ms` is the configured per-job execution timeout, threaded
/// into each snapshot so it reaches the executor.
pub async fn phase1_mark_due_jobs<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    default_timeout_ms: i64,
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
        match job.state.next_run_at_ms {
            Some(t) if t <= now => {}
            _ => continue,
        }

        // Mark as running.
        job.state.running_at_ms = Some(now);

        // At-most-once: advance the schedule to the next future occurrence
        // *before* execution. If the process crashes between here and phase 3
        // writeback, the schedule is already advanced — a recurring job will
        // not re-run the same trigger, and a one-shot `At` job (whose next
        // occurrence is `None`) will not fire again.
        advance_next_run(job, clock);

        snapshots.push(JobSnapshot {
            id: job.id.clone(),
            agent_id: Some(job.agent_id.clone()),
            source_channel_id: job.source_channel_id.clone(),
            source_conversation_id: job.source_conversation_id.clone(),
            prompt: resolve_job_prompt(job, clock),
            model: None,
            timeout_ms: Some(default_timeout_ms),
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
/// Returns `None` if the job is already running. Like `phase1_mark_due_jobs`,
/// the schedule is advanced before execution so a crash does not double-run it.
pub async fn phase1_mark_manual<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    job_id: &str,
    default_timeout_ms: i64,
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
    advance_next_run(job, clock);

    let snapshot = JobSnapshot {
        id: job.id.clone(),
        agent_id: Some(job.agent_id.clone()),
        source_channel_id: job.source_channel_id.clone(),
        source_conversation_id: job.source_conversation_id.clone(),
        prompt: resolve_job_prompt(job, clock),
        model: None,
        timeout_ms: Some(default_timeout_ms),
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
///
/// `next_run_at_ms` is **not** cleared here: phase 1 already advanced the
/// schedule before execution (at-most-once). After processing all results
/// this function:
/// - removes one-shot `At` jobs flagged `delete_after_run`,
/// - runs maintenance recompute (fills only still-missing `next_run_at_ms`),
/// - applies failure backoff to recurring jobs that just failed,
/// - fires job chains,
/// - persists.
pub async fn phase3_writeback<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    results: &[(String, ExecutionResult)],
) -> Result<(), String> {
    let mut guard = store.lock().await;

    guard.force_reload()?;

    // (source_job_id, successor_job_id) pairs resolved during the result
    // loop; fired after maintenance recompute so the `job` borrow is free.
    let mut chain_triggers: Vec<(String, String)> = Vec::new();

    // One-shot `At` jobs flagged `delete_after_run`, removed after history
    // is recorded so the run is preserved.
    let mut jobs_to_delete: Vec<String> = Vec::new();

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

        // Template context for the next run: a bounded copy of this run's
        // output and an incremented run counter feed `{{last_output}}` /
        // `{{run_count}}` when the job uses a `prompt_template`.
        job.state.run_count = job.state.run_count.saturating_add(1);
        job.state.last_output = result.output.as_ref().map(|s| {
            const MAX_CHARS: usize = 2_000;
            if s.chars().count() > MAX_CHARS {
                s.chars().take(MAX_CHARS).collect()
            } else {
                s.clone()
            }
        });

        // Update consecutive errors
        match result.status {
            RunStatus::Ok | RunStatus::Skipped => {
                job.state.consecutive_errors = 0;
            }
            RunStatus::Error | RunStatus::Timeout => {
                job.state.consecutive_errors += 1;
            }
        }

        // Resolve the chain successor for this outcome. Triggering needs
        // `&mut store`, so collect here and apply after the `job` borrow ends.
        let chain_target = match result.status {
            RunStatus::Ok | RunStatus::Skipped => job.next_job_id_on_success.clone(),
            RunStatus::Error | RunStatus::Timeout => job.next_job_id_on_failure.clone(),
        };
        if let Some(target) = chain_target {
            chain_triggers.push((job_id.clone(), target));
        }

        // One-shot `At` jobs with `delete_after_run` are removed once they
        // have run. `next_run_at_ms` is left as phase 1 set it (recurring
        // jobs keep their advanced schedule; `At` jobs are already `None`).
        if let ScheduleKind::At {
            delete_after_run: true,
            ..
        } = job.schedule_kind
        {
            jobs_to_delete.push(job_id.clone());
        }
    }

    // Record execution history
    for (job_id, result) in results {
        let record = CronRunRecord {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: job_id.clone(),
            trigger_source: result.trigger_source.as_str().to_string(),
            status: format!("{:?}", result.status).to_lowercase(),
            started_at: result.started_at,
            ended_at: Some(result.ended_at),
            duration_ms: Some(result.duration_ms),
            error: result.error.clone(),
            error_reason: result.error_reason.as_ref().map(|r| format!("{r:?}")),
            output_summary: None,
            delivery_status: result
                .delivery_status
                .map(|s| format!("{s:?}").to_lowercase()),
            created_at: clock.now_ms(),
        };
        if let Err(e) = guard.insert_run(&record) {
            warn!(job_id, error = %e, "failed to record cron run history");
        }
    }

    // Remove one-shot jobs flagged `delete_after_run` (history is recorded).
    for job_id in &jobs_to_delete {
        guard.remove_job(job_id);
    }

    // Maintenance recompute ALL jobs (fills only still-missing next_run_at_ms).
    {
        let jobs = guard.jobs_mut();
        for job in jobs.iter_mut() {
            recompute_next_run_maintenance(job, clock);
        }
    }

    // Failure backoff: a recurring job that just failed gets its next run
    // pushed out by an escalating delay so it does not hot-loop at its normal
    // interval. `consecutive_errors` was already incremented above.
    let backoff_now = clock.now_ms();
    for (job_id, result) in results {
        if !matches!(result.status, RunStatus::Error | RunStatus::Timeout) {
            continue;
        }
        let Some(job) = guard.get_job_mut(job_id) else {
            continue;
        };
        if matches!(job.schedule_kind, ScheduleKind::At { .. }) {
            continue; // one-shot jobs are not rescheduled
        }
        let backoff = compute_backoff_ms(job.state.consecutive_errors);
        if backoff > 0 {
            let floor = backoff_now.saturating_add(backoff);
            let next = job.state.next_run_at_ms.map_or(floor, |n| n.max(floor));
            job.state.next_run_at_ms = Some(next);
        }
    }

    // Fire job chains: enqueue each successor to run on the next tick.
    // Done *after* maintenance recompute so the chain trigger's
    // `next_run_at_ms = now` wins over a recomputed schedule. Cyclic chains
    // are skipped — chain.rs has no creation-time guard, so an A->B->A loop
    // would otherwise re-trigger every tick (runaway).
    let chain_now = clock.now_ms();
    for (source_id, target_id) in chain_triggers {
        match chain::detect_cycle(&guard, &source_id, &target_id) {
            Ok(true) => {
                warn!(
                    source = %source_id,
                    target = %target_id,
                    "phase3: skipping cyclic chain trigger"
                );
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                warn!(
                    source = %source_id,
                    target = %target_id,
                    error = %e,
                    "phase3: chain cycle check failed; skipping trigger"
                );
                continue;
            }
        }
        match chain::trigger_chain_job(&mut guard, &target_id, chain_now) {
            // Triggered, or a benign no-op (target missing or disabled).
            Ok(_) => {}
            Err(e) => warn!(
                target = %target_id,
                error = %e,
                "phase3: chain trigger failed"
            ),
        }
    }

    guard.persist()?;

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::config::{CronJob, RunStatus, ScheduleKind};
    use crate::tasks::cron::service::ops::add_job;
    use crate::tasks::shared::clock::testing::FakeClock;
    use tempfile::TempDir;

    /// Configured per-job timeout used by the test timer.
    const TEST_TIMEOUT_MS: i64 = 300_000;

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
        let path = dir.path().join("cron.db");
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
            trigger_source: TriggerSource::Schedule,
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

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
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

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
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
        assert!(
            job.state.running_at_ms.is_none(),
            "running_at_ms should be cleared"
        );
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
    async fn phase3_triggers_success_chain() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(1_100_000),
            "Ok result triggers next_job_id_on_success to run now"
        );
    }

    #[tokio::test]
    async fn phase3_triggers_failure_chain() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_failure = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Error))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(1_100_000),
            "Error result triggers next_job_id_on_failure to run now"
        );
    }

    #[tokio::test]
    async fn phase3_success_chain_not_fired_on_failure() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Error))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(9_999_999_999),
            "Error result must not fire the on_success chain"
        );
    }

    #[tokio::test]
    async fn phase3_skips_cyclic_chain() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            let mut job_b = make_test_job("job-b");
            job_b.next_job_id_on_success = Some("job-a".to_string());
            add_job(&mut guard, job_b, &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(9_999_999_999),
            "a cyclic chain (a->b->a) must not be triggered"
        );
    }

    #[tokio::test]
    async fn phase3_chain_target_disabled_not_triggered() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.enabled = false;
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(9_999_999_999),
            "a disabled chain target must not be triggered"
        );
    }

    #[tokio::test]
    async fn phase3_chain_target_missing_is_noop() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("ghost".to_string());
            add_job(&mut guard, job_a, &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        let outcome = phase3_writeback(&store, &clock, &results).await;
        assert!(
            outcome.is_ok(),
            "a chain target that no longer exists must not error"
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

        let snapshot = phase1_mark_manual(&store, &clock, "manual-job", TEST_TIMEOUT_MS)
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

        let snapshot = phase1_mark_manual(&store, &clock, "busy-job", TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert!(
            snapshot.is_none(),
            "should return None for already-running job"
        );
    }

    /// C1: phase 1 advances a recurring job's schedule *before* execution,
    /// so a crash before phase 3 does not double-run the same trigger.
    #[tokio::test]
    async fn phase1_advances_recurring_schedule_before_execution() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("recurring");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("recurring").unwrap();
            j.state.next_run_at_ms = Some(950_000); // due
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].timeout_ms,
            Some(TEST_TIMEOUT_MS),
            "configured timeout must reach the snapshot"
        );

        let guard = store.lock().await;
        let job = guard.get_job("recurring").unwrap();
        let next = job
            .state
            .next_run_at_ms
            .expect("recurring job keeps a next run");
        assert!(
            next > 1_000_000,
            "next_run must be advanced past now before execution, got {next}"
        );
    }

    /// C1: phase 1 clears a one-shot `At` job's schedule before execution,
    /// so it cannot fire twice even across a crash.
    #[tokio::test]
    async fn phase1_clears_one_shot_schedule_before_execution() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);
        {
            let mut guard = store.lock().await;
            let mut job = CronJob::new(
                "once",
                "agent",
                "prompt",
                ScheduleKind::At {
                    at: 950_000,
                    delete_after_run: false,
                },
            );
            job.id = "once".to_string();
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("once").unwrap();
            j.state.next_run_at_ms = Some(950_000); // due
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);

        let guard = store.lock().await;
        let job = guard.get_job("once").unwrap();
        assert_eq!(
            job.state.next_run_at_ms, None,
            "one-shot job must not be re-fireable after being marked"
        );
    }

    /// C3: phase 3 removes a one-shot job flagged `delete_after_run`.
    #[tokio::test]
    async fn phase3_deletes_one_shot_with_delete_after_run() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = CronJob::new(
                "oneshot",
                "agent",
                "prompt",
                ScheduleKind::At {
                    at: 1_000_000,
                    delete_after_run: true,
                },
            );
            job.id = "oneshot".to_string();
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("oneshot").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let results = vec![("oneshot".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        assert!(
            guard.get_job("oneshot").is_none(),
            "delete_after_run job should be removed after it runs"
        );
        // History is still recorded.
        let runs = guard.get_runs("oneshot", 10).unwrap();
        assert_eq!(runs.len(), 1, "the run must be preserved in history");
    }

    /// C3: a one-shot job without `delete_after_run` is kept (and inert).
    #[tokio::test]
    async fn phase3_keeps_one_shot_without_delete_after_run() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = CronJob::new(
                "keep",
                "agent",
                "prompt",
                ScheduleKind::At {
                    at: 1_000_000,
                    delete_after_run: false,
                },
            );
            job.id = "keep".to_string();
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("keep").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let results = vec![("keep".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("keep").expect("job should be kept");
        assert_eq!(
            job.state.next_run_at_ms, None,
            "a completed one-shot job must not re-fire"
        );
    }

    /// C4: a recurring job that fails has its next run pushed out by backoff.
    #[tokio::test]
    async fn phase3_applies_backoff_on_failure() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("flaky"); // Every 60s
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("flaky").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.next_run_at_ms = Some(1_120_000); // phase1-advanced, soon
            j.state.consecutive_errors = 2; // becomes 3 after this error
            guard.persist().unwrap();
        }

        let results = vec![("flaky".to_string(), make_execution_result(RunStatus::Error))];
        phase3_writeback(&store, &clock, &results).await.unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("flaky").unwrap();
        // 3 consecutive errors → 5 min backoff tier.
        let next = job.state.next_run_at_ms.unwrap();
        assert!(
            next >= 1_100_000 + 300_000,
            "backoff not applied: next={next}, expected >= {}",
            1_100_000 + 300_000
        );
    }
}
