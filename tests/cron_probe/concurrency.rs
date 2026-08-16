//! P2 Concurrency Safety Probes — 6 scenarios verifying safe behavior
//! when jobs are listed, updated, deleted, or added during execution.

use alephcore::tasks::cron::config::TriggerSource;
use alephcore::tasks::cron::service::concurrency::{phase1_mark_due_jobs, phase3_writeback};

use super::harness::CronTestHarness;

const TEST_TIMEOUT_MS: i64 = 300_000;

// ── 1. concurrent_list_during_execution ─────────────────────────────

/// Phase 1 marks job running → list_jobs while "executing" → verify
/// running_at_ms visible → Phase 3 writeback → verify cleared.
#[tokio::test]
async fn concurrent_list_during_execution() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("list-during-exec", interval).await;
    h.advance(interval);

    // Phase 1: mark due jobs
    let snapshots = phase1_mark_due_jobs(&h.state.store, h.clock.as_ref(), TEST_TIMEOUT_MS)
        .await
        .expect("phase1 failed");
    assert_eq!(snapshots.len(), 1);

    // During "execution" (Phase 2): list_jobs should show running_at_ms set
    h.assert_running("list-during-exec", true).await;
    let views = h.list_jobs().await;
    assert_eq!(views.len(), 1);
    assert!(
        views[0].state.running_at_ms.is_some(),
        "running_at_ms should be visible during execution"
    );

    // Simulate execution result
    let result = alephcore::tasks::cron::config::ExecutionResult {
        started_at: h.now(),
        ended_at: h.now() + 100,
        duration_ms: 100,
        status: alephcore::tasks::cron::config::RunStatus::Ok,
        output: Some("ok".to_string()),
        error: None,
        error_reason: None,
        delivery_status: Some(alephcore::tasks::cron::config::DeliveryStatus::NotRequested),
        trigger_source: TriggerSource::Schedule,
        retry_hint: None,
    };

    // Phase 3: writeback
    let results = vec![("list-during-exec".to_string(), result)];
    phase3_writeback(
        &h.state.store,
        h.clock.as_ref(),
        &results,
        h.state.config.notify_on_failure_default,
    )
    .await
    .expect("phase3 failed");

    // After writeback: running_at_ms should be cleared
    h.assert_running("list-during-exec", false).await;
}

// ── 2. update_during_execution ──────────────────────────────────────

/// Phase 1 → during Phase 2, update job name directly in store + persist →
/// Phase 3 writeback → verify new name preserved AND execution result written.
#[tokio::test]
async fn update_during_execution() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("update-during", interval).await;
    h.advance(interval);

    // Phase 1: mark due
    let snapshots = phase1_mark_due_jobs(&h.state.store, h.clock.as_ref(), TEST_TIMEOUT_MS)
        .await
        .expect("phase1 failed");
    assert_eq!(snapshots.len(), 1);

    // During Phase 2: update job name directly via store
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("update-during").unwrap();
        job.name = "updated-name".to_string();
        store.persist().expect("persist after update failed");
    }

    // Simulate execution result
    let result = alephcore::tasks::cron::config::ExecutionResult {
        started_at: h.now(),
        ended_at: h.now() + 100,
        duration_ms: 100,
        status: alephcore::tasks::cron::config::RunStatus::Ok,
        output: Some("ok".to_string()),
        error: None,
        error_reason: None,
        delivery_status: Some(alephcore::tasks::cron::config::DeliveryStatus::NotRequested),
        trigger_source: TriggerSource::Schedule,
        retry_hint: None,
    };

    // Phase 3: writeback (force_reload merges concurrent edits)
    let results = vec![("update-during".to_string(), result)];
    phase3_writeback(
        &h.state.store,
        h.clock.as_ref(),
        &results,
        h.state.config.notify_on_failure_default,
    )
    .await
    .expect("phase3 failed");

    // Verify: new name preserved AND execution result written
    let store = h.state.store.lock().await;
    let job = store.get_job("update-during").unwrap();
    assert_eq!(
        job.name, "updated-name",
        "name change during execution should be preserved (MVCC merge)"
    );
    assert_eq!(
        job.state.last_run_status,
        Some(alephcore::tasks::cron::config::RunStatus::Ok),
        "execution result should be written"
    );
    assert!(
        job.state.running_at_ms.is_none(),
        "running_at_ms should be cleared"
    );
}

// ── 3. delete_during_execution ──────────────────────────────────────

/// Phase 1 → during Phase 2, remove job from store + persist →
/// Phase 3 writeback → no panic, job gone.
#[tokio::test]
async fn delete_during_execution() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("delete-during", interval).await;
    h.advance(interval);

    // Phase 1: mark due
    let snapshots = phase1_mark_due_jobs(&h.state.store, h.clock.as_ref(), TEST_TIMEOUT_MS)
        .await
        .expect("phase1 failed");
    assert_eq!(snapshots.len(), 1);

    // During Phase 2: delete the job
    h.delete_job("delete-during").await;
    assert!(
        !h.job_exists("delete-during").await,
        "job should be deleted during execution"
    );

    // Simulate execution result
    let result = alephcore::tasks::cron::config::ExecutionResult {
        started_at: h.now(),
        ended_at: h.now() + 100,
        duration_ms: 100,
        status: alephcore::tasks::cron::config::RunStatus::Ok,
        output: Some("ok".to_string()),
        error: None,
        error_reason: None,
        delivery_status: Some(alephcore::tasks::cron::config::DeliveryStatus::NotRequested),
        trigger_source: TriggerSource::Schedule,
        retry_hint: None,
    };

    // Phase 3: writeback — should NOT panic, just warn and skip
    let results = vec![("delete-during".to_string(), result)];
    let outcome = phase3_writeback(
        &h.state.store,
        h.clock.as_ref(),
        &results,
        h.state.config.notify_on_failure_default,
    )
    .await;
    assert!(
        outcome.is_ok(),
        "phase3 should gracefully handle deleted job"
    );

    // Job should still be gone
    assert!(
        !h.job_exists("delete-during").await,
        "job should remain deleted after writeback"
    );
}

// ── 4. multiple_jobs_concurrent ─────────────────────────────────────

/// 5 jobs all due → tick → all 5 executed, call_log has 5 entries.
#[tokio::test]
async fn multiple_jobs_concurrent() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    for i in 0..5 {
        h.add_every_job(&format!("multi-{i}"), interval).await;
    }

    h.advance(interval);
    h.tick().await;

    // All 5 should be executed
    for i in 0..5 {
        h.assert_executed(&format!("multi-{i}"));
    }

    let calls = h.executor.calls();
    assert_eq!(
        calls.len(),
        5,
        "call_log should have 5 entries, got {}",
        calls.len()
    );
}

// ── 5. an in-flight job is not dispatched twice ─────────────────────

/// Overlapping ticks must not double-dispatch a job.
///
/// This used to be asserted against `ServiceState::is_running`, a whole-tick
/// re-entrancy flag — and the test's own comment admitted `on_timer_tick` never
/// consulted it, so it exercised a setter and a getter and nothing else. That
/// flag is gone: holding it across the whole batch froze the due-scan for as
/// long as the slowest job ran (`job_timeout_secs`, default 900s) and lost
/// every firing in between.
///
/// The invariant that actually protects re-entrancy is per job and always was:
/// `phase1_mark_due_jobs` stamps `running_at_ms` under the store lock and skips
/// any job already carrying one.
#[tokio::test]
async fn a_job_already_in_flight_is_not_dispatched_again() {
    let h = CronTestHarness::new();
    let interval = 60_000;
    h.add_every_job("in-flight", interval).await;

    // Stamp the marker phase1 would set for a live run, without letting a
    // writeback clear it.
    {
        let mut guard = h.state.store.lock().await;
        let job = guard.get_job_mut("in-flight").expect("job present");
        job.state.running_at_ms = Some(h.now());
        guard.persist().expect("persist");
    }

    h.advance(interval);
    // Not `tick()`: the marker above is artificial and nothing will clear it,
    // so waiting for quiescence would wait forever by construction.
    h.tick_without_settling().await;
    assert_eq!(
        h.executor.call_count("in-flight"),
        0,
        "a job carrying a running marker must be skipped, not re-dispatched"
    );

    // Clear it the way phase3 does; the next tick picks the job up normally.
    {
        let mut guard = h.state.store.lock().await;
        guard
            .get_job_mut("in-flight")
            .expect("job present")
            .state
            .running_at_ms = None;
        guard.persist().expect("persist");
    }
    h.advance(interval);
    h.tick().await;
    h.assert_executed("in-flight");
}

// ── 6. concurrent_add_during_tick ───────────────────────────────────

/// Add job → advance → tick → verify executed → add new job →
/// advance → tick → verify new job also executed.
#[tokio::test]
async fn concurrent_add_during_tick() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    // Add first job
    h.add_every_job("add-first", interval).await;
    h.advance(interval);
    h.tick().await;
    h.assert_executed("add-first");
    h.assert_execution_count("add-first", 1);

    // Add second job while first has already been through a cycle
    h.add_every_job("add-second", interval).await;

    // Advance past the interval for both jobs
    h.advance(interval);
    h.tick().await;

    // First job should have been executed again
    h.assert_execution_count("add-first", 2);

    // Second job should have been executed
    h.assert_executed("add-second");
    h.assert_execution_count("add-second", 1);
}
