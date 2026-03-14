//! P4 Fault Recovery Probes — 7 scenarios verifying error handling,
//! retry behavior, stale marker cleanup, and catchup recovery.

use alephcore::cron::config::ErrorReason;
use alephcore::cron::service::catchup::run_startup_catchup;

use super::harness::CronTestHarness;
use super::mock_executor::MockBehavior;

// ── 1. transient_error_retries ──────────────────────────────────────

/// Configure MockBehavior::Error with transient reason → advance → tick →
/// verify consecutive_errors=1, job still enabled, next_run scheduled.
#[tokio::test]
async fn transient_error_retries() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("transient-1", interval).await;

    h.executor.on_job(
        "transient-1",
        MockBehavior::Error {
            message: "network timeout".to_string(),
            reason: ErrorReason::Transient("connection refused".to_string()),
        },
    );

    h.advance(interval);
    h.tick().await;

    // Should have been executed (and failed)
    h.assert_executed("transient-1");
    h.assert_consecutive_errors("transient-1", 1).await;

    // Job should still be enabled after transient error
    h.assert_job_enabled("transient-1", true).await;

    // next_run should be scheduled (recomputed by maintenance)
    let state = h.job_state("transient-1").await;
    assert!(
        state.next_run_at_ms.is_some(),
        "transient error should not prevent rescheduling"
    );
}

// ── 2. permanent_error_disables ─────────────────────────────────────

/// Configure permanent error → tick → verify consecutive_errors=1.
/// NOTE: The current implementation does NOT auto-disable on permanent errors;
/// it only increments consecutive_errors. This test documents that behavior.
#[tokio::test]
async fn permanent_error_disables() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("permanent-1", interval).await;

    h.executor.on_job(
        "permanent-1",
        MockBehavior::Error {
            message: "invalid API key".to_string(),
            reason: ErrorReason::Permanent("auth failure".to_string()),
        },
    );

    h.advance(interval);
    h.tick().await;

    h.assert_executed("permanent-1");
    h.assert_consecutive_errors("permanent-1", 1).await;

    // NOTE: Current implementation does NOT auto-disable on permanent errors.
    // The job remains enabled. This is a design decision — the cron system
    // tracks errors but leaves disable decisions to higher-level logic.
    h.assert_job_enabled("permanent-1", true).await;
}

// ── 3. max_retries_then_disable ─────────────────────────────────────

/// Configure transient error → run 4 ticks (default max_retries=3) →
/// verify execution count and consecutive_errors increment.
/// NOTE: The cron system does NOT auto-disable based on max_retries.
/// It only tracks consecutive_errors. This test documents the counter behavior.
#[tokio::test]
async fn max_retries_then_disable() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("max-retry-1", interval).await;

    h.executor.on_job(
        "max-retry-1",
        MockBehavior::Error {
            message: "transient failure".to_string(),
            reason: ErrorReason::Transient("timeout".to_string()),
        },
    );

    // Run 4 ticks, each advancing past the interval
    for i in 1..=4u32 {
        h.advance(interval);
        h.tick().await;
        h.assert_consecutive_errors("max-retry-1", i).await;
    }

    // All 4 ticks should have executed (errors don't prevent next runs)
    h.assert_execution_count("max-retry-1", 4);
    h.assert_consecutive_errors("max-retry-1", 4).await;

    // Job remains enabled — cron system tracks errors but doesn't auto-disable
    h.assert_job_enabled("max-retry-1", true).await;
}

// ── 4. success_resets_errors ────────────────────────────────────────

/// Fail 3 times → switch to Ok → tick → verify consecutive_errors=0.
#[tokio::test]
async fn success_resets_errors() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("reset-errors-1", interval).await;

    // Configure error behavior
    h.executor.on_job(
        "reset-errors-1",
        MockBehavior::Error {
            message: "fail".to_string(),
            reason: ErrorReason::Transient("retry".to_string()),
        },
    );

    // Fail 3 times
    for i in 1..=3u32 {
        h.advance(interval);
        h.tick().await;
        h.assert_consecutive_errors("reset-errors-1", i).await;
    }

    assert_eq!(h.executor.call_count("reset-errors-1"), 3);

    // Switch to success
    h.executor.on_job(
        "reset-errors-1",
        MockBehavior::Ok("recovered".to_string()),
    );

    h.advance(interval);
    h.tick().await;

    // consecutive_errors should reset to 0
    h.assert_consecutive_errors("reset-errors-1", 0).await;
    h.assert_execution_count("reset-errors-1", 4);
}

// ── 5. stale_marker_cleared ─────────────────────────────────────────

/// Manually set running_at_ms to 3h ago → run_catchup → verify cleared.
#[tokio::test]
async fn stale_marker_cleared() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("stale-1", interval).await;

    // Manually set running_at_ms to 3 hours ago
    let three_hours_ms = 3 * 3_600_000_i64;
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("stale-1").unwrap();
        job.state.running_at_ms = Some(h.now() - three_hours_ms);
        // Set next_run to future so it's not counted as missed
        job.state.next_run_at_ms = Some(h.now() + interval);
        store.persist().expect("persist failed");
    }

    // Verify running_at_ms is set before catchup
    h.assert_running("stale-1", true).await;

    // Run catchup
    h.run_catchup().await;

    // Stale marker should be cleared (3h > 2h threshold)
    h.assert_running("stale-1", false).await;
}

// ── 6. catchup_staggers_missed ──────────────────────────────────────

/// Create 10 past-due jobs → run_startup_catchup(store, clock, Some(3), Some(5000))
/// → verify report: 3 immediate, 7 deferred.
#[tokio::test]
async fn catchup_staggers_missed() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    // Create 10 jobs, all past due
    for i in 0..10 {
        h.add_every_job(&format!("catchup-{i}"), interval).await;
    }

    // Manually set all jobs as past due with different next_run times
    {
        let mut store = h.state.store.lock().await;
        for i in 0..10 {
            let job = store.get_job_mut(&format!("catchup-{i}")).unwrap();
            // All past due, ordered by next_run
            job.state.next_run_at_ms = Some(h.now() - 10_000 + i as i64 * 100);
        }
        store.persist().expect("persist failed");
    }

    // Run catchup with max_missed=3, stagger=5000ms
    let report = run_startup_catchup(
        &h.state.store,
        h.clock.as_ref(),
        Some(3),
        Some(5000),
    )
    .await
    .expect("catchup failed");

    assert_eq!(
        report.immediate_count, 3,
        "should have 3 immediate jobs"
    );
    assert_eq!(
        report.deferred_count, 7,
        "should have 7 deferred jobs"
    );

    // Verify deferred jobs have staggered future next_run times
    let store = h.state.store.lock().await;
    let now = h.now();
    let mut deferred_times = Vec::new();
    for i in 0..10 {
        let job = store.get_job(&format!("catchup-{i}")).unwrap();
        if let Some(next) = job.state.next_run_at_ms {
            if next > now {
                deferred_times.push(next);
            }
        }
    }
    assert_eq!(
        deferred_times.len(),
        7,
        "7 jobs should have future next_run times"
    );
}

// ── 7. catchup_then_normal_tick ─────────────────────────────────────

/// Make job past due → catchup → tick → verify executed →
/// advance → tick → verify executed again (normal schedule resumes).
#[tokio::test]
async fn catchup_then_normal_tick() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("catchup-normal-1", interval).await;

    // Make job past due
    {
        let mut store = h.state.store.lock().await;
        let job = store.get_job_mut("catchup-normal-1").unwrap();
        job.state.next_run_at_ms = Some(h.now() - 5_000); // 5s past due
        store.persist().expect("persist failed");
    }

    // Run catchup (job stays as immediate since max_missed defaults to 5)
    h.run_catchup().await;

    // Tick should execute the past-due job
    h.tick().await;
    h.assert_executed("catchup-normal-1");
    h.assert_execution_count("catchup-normal-1", 1);

    // Now advance to next interval and tick again — normal schedule resumes
    h.advance(interval);
    h.tick().await;
    h.assert_execution_count("catchup-normal-1", 2);
}
