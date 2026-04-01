//! P1 Lifecycle Probes — 8 end-to-end scenarios covering the full
//! cron job lifecycle: create, schedule, execute, update, disable, delete, persist.

use alephcore::cron::config::{ScheduleKind, TriggerSource};
use alephcore::cron::service::ops::CronJobUpdates;

use super::harness::CronTestHarness;

// ── 1. full_lifecycle_every ─────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_every() {
    let h = CronTestHarness::new();
    let interval = 60_000; // 60s

    h.add_every_job("every-1", interval).await;

    // Advance to first due time and tick
    h.advance(interval);
    h.tick().await;
    h.assert_executed("every-1");
    h.assert_execution_count("every-1", 1);

    // Advance to second period and tick
    h.advance(interval);
    h.tick().await;
    h.assert_execution_count("every-1", 2);

    // Verify persisted to disk
    let content = h.store_file_content();
    assert!(
        content.contains("every-1"),
        "job should be persisted to disk"
    );
}

// ── 2. full_lifecycle_cron ──────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_cron() {
    let h = CronTestHarness::new();

    // Use a cron expression: every hour at minute 0
    h.add_cron_job("cron-1", "0 0 * * * *").await;

    // Verify next_run_at_ms is computed and in the future
    let state = h.job_state("cron-1").await;
    let next = state
        .next_run_at_ms
        .expect("cron job should have next_run_at_ms");
    assert!(
        next > h.now(),
        "next_run_at_ms ({next}) should be > now ({})",
        h.now()
    );
}

// ── 3. full_lifecycle_at ────────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_at() {
    let h = CronTestHarness::new();
    let fire_at = h.now() + 30_000; // 30s from now

    h.add_at_job("at-1", fire_at).await;

    // Before due: not executed
    h.advance(10_000);
    h.tick().await;
    h.assert_not_executed("at-1");

    // At due: executed
    h.advance(20_000); // now at fire_at
    h.tick().await;
    h.assert_executed("at-1");
    h.assert_execution_count("at-1", 1);

    // After: not executed again
    h.advance(30_000);
    h.tick().await;
    h.assert_execution_count("at-1", 1);
}

// ── 4. manual_trigger ───────────────────────────────────────────────

#[tokio::test]
async fn manual_trigger() {
    let h = CronTestHarness::new();

    // Add job with long interval (not due any time soon)
    h.add_every_job("manual-1", 3_600_000).await;
    h.assert_not_executed("manual-1");

    // Manual trigger
    h.manual_run("manual-1").await;
    h.assert_executed("manual-1");
    h.assert_execution_count("manual-1", 1);

    // Verify trigger source was Manual
    let calls = h.executor.calls_for("manual-1");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].trigger_source,
        TriggerSource::Manual,
        "manual_run should set TriggerSource::Manual"
    );
}

// ── 5. disable_prevents_execution ───────────────────────────────────

#[tokio::test]
async fn disable_prevents_execution() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("disable-1", interval).await;

    // Disable the job
    let enabled = h.toggle_job("disable-1").await;
    assert!(!enabled, "toggle should disable");
    h.assert_job_enabled("disable-1", false).await;

    // Advance past due and tick — should NOT execute
    h.advance(interval);
    h.tick().await;
    h.assert_not_executed("disable-1");
}

// ── 6. update_reschedules ───────────────────────────────────────────

#[tokio::test]
async fn update_reschedules() {
    let h = CronTestHarness::new();

    h.add_every_job("update-1", 60_000).await;
    let before = h
        .job_state("update-1")
        .await
        .next_run_at_ms
        .expect("should have next_run");

    // Advance time so the new interval computes a different next_run
    h.advance(30_000);

    // Update to a much longer interval (5 min)
    let updates = CronJobUpdates {
        schedule_kind: Some(ScheduleKind::Every {
            every_ms: 300_000,
            anchor_ms: None,
        }),
        ..Default::default()
    };
    h.update_job("update-1", updates).await;

    let after = h
        .job_state("update-1")
        .await
        .next_run_at_ms
        .expect("should have next_run after update");

    // next_run should have changed (recomputed with new interval from new anchor)
    assert_ne!(
        before, after,
        "update_job should recompute next_run_at_ms (before={before}, after={after})"
    );
}

// ── 7. delete_stops_execution ───────────────────────────────────────

#[tokio::test]
async fn delete_stops_execution() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("delete-1", interval).await;

    // Execute once
    h.advance(interval);
    h.tick().await;
    h.assert_execution_count("delete-1", 1);

    // Delete the job
    h.delete_job("delete-1").await;
    assert!(
        !h.job_exists("delete-1").await,
        "job should be gone after delete"
    );

    // Advance and tick — no more executions
    h.advance(interval);
    h.tick().await;
    h.assert_execution_count("delete-1", 1); // still 1, not 2
}

// ── 8. job_persists_across_reload ───────────────────────────────────

#[tokio::test]
async fn job_persists_across_reload() {
    let h = CronTestHarness::new();

    h.add_every_job("persist-1", 60_000).await;

    // Verify job exists before reload
    assert!(h.job_exists("persist-1").await);

    // Force reload the store from disk
    {
        let mut store = h.state.store.lock().await;
        store.force_reload().unwrap();
    }

    // Job should still be there after reload
    assert!(
        h.job_exists("persist-1").await,
        "job should survive force_reload"
    );
}
