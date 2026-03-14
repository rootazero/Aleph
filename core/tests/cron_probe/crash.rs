//! P7 Crash Recovery Probes — 3 scenarios verifying store integrity
//! and stale marker recovery after simulated crashes using manual
//! phase-call + drop patterns.

use std::sync::Arc;

use alephcore::cron::clock::testing::FakeClock;
use alephcore::cron::config::{CronJob, ScheduleKind};
use alephcore::cron::service::catchup::run_startup_catchup;
use alephcore::cron::service::concurrency::phase1_mark_due_jobs;
use alephcore::cron::service::ops;
use alephcore::cron::store::CronStore;
use tempfile::TempDir;

// ── Helpers ─────────────────────────────────────────────────────────────

fn make_job(id: &str) -> CronJob {
    let mut job = CronJob::new(
        id,
        "test-agent",
        format!("prompt for {id}"),
        ScheduleKind::Every {
            every_ms: 60_000,
            anchor_ms: None,
        },
    );
    job.id = id.to_string();
    job
}

// ── 1. crash_mid_execution_recovers ─────────────────────────────────────

/// Simulate crash after Phase 1 (mark due jobs as running) by dropping
/// everything, then restart from the same store path and verify catchup
/// clears stale running markers.
#[tokio::test]
async fn crash_mid_execution_recovers() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let store_path = temp_dir.path().join("cron.json");

    let initial_time = 1_000_000_i64;
    let interval = 60_000_i64;

    // === First "run": add job, advance to due, mark running, then "crash" ===
    {
        let clock = Arc::new(FakeClock::new(initial_time));
        let store = CronStore::load(store_path.clone()).expect("load store");
        let store = Arc::new(tokio::sync::Mutex::new(store));

        // Add a job
        {
            let mut guard = store.lock().await;
            let job = make_job("crash-test-1");
            ops::add_job(&mut guard, job, clock.as_ref());
            guard.persist().expect("persist after add");
        }

        // Advance clock past the due time
        clock.advance(interval);

        // Phase 1: mark due jobs as running (persists running_at_ms)
        let snapshots = phase1_mark_due_jobs(&store, clock.as_ref())
            .await
            .expect("phase1 should succeed");
        assert!(
            !snapshots.is_empty(),
            "phase1 should find at least one due job"
        );

        // === CRASH: drop everything without Phase 2 or Phase 3 ===
        // store and clock go out of scope here
    }

    // === Second "run": restart from same store path, 3 hours later ===
    {
        let three_hours_ms = 3 * 3_600_000_i64;
        let restart_time = initial_time + interval + three_hours_ms;
        let clock = Arc::new(FakeClock::new(restart_time));
        let store = CronStore::load(store_path.clone()).expect("reload store after crash");
        let store = Arc::new(tokio::sync::Mutex::new(store));

        // Verify running_at_ms is still set from the crashed run
        {
            let guard = store.lock().await;
            let job = guard.get_job("crash-test-1").expect("job should exist after reload");
            assert!(
                job.state.running_at_ms.is_some(),
                "running_at_ms should still be set from the crashed run"
            );
        }

        // Run startup catchup — should clear stale marker
        let report = run_startup_catchup(&store, clock.as_ref(), None, None)
            .await
            .expect("catchup should succeed");

        assert_eq!(
            report.stale_markers_cleared, 1,
            "catchup should clear 1 stale running marker"
        );

        // Verify running_at_ms is cleared
        {
            let guard = store.lock().await;
            let job = guard.get_job("crash-test-1").unwrap();
            assert!(
                job.state.running_at_ms.is_none(),
                "running_at_ms should be cleared after catchup"
            );
        }
    }
}

// ── 2. crash_after_phase1_before_phase2 ─────────────────────────────────

/// Verify that the raw JSON file contains running_at_ms after Phase 1,
/// and that restart + catchup clears it.
#[tokio::test]
async fn crash_after_phase1_before_phase2() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let store_path = temp_dir.path().join("cron.json");

    let initial_time = 2_000_000_i64;
    let interval = 60_000_i64;

    // === First "run": phase1 then crash ===
    {
        let clock = Arc::new(FakeClock::new(initial_time));
        let store = CronStore::load(store_path.clone()).expect("load store");
        let store = Arc::new(tokio::sync::Mutex::new(store));

        {
            let mut guard = store.lock().await;
            let job = make_job("phase1-crash");
            ops::add_job(&mut guard, job, clock.as_ref());
            guard.persist().expect("persist after add");
        }

        clock.advance(interval);

        let _snapshots = phase1_mark_due_jobs(&store, clock.as_ref())
            .await
            .expect("phase1 should succeed");

        // Verify the raw JSON file contains running_at_ms
        let raw_json = std::fs::read_to_string(&store_path)
            .expect("should be able to read store file");
        assert!(
            raw_json.contains("running_at_ms"),
            "raw JSON should contain running_at_ms after Phase 1"
        );

        // CRASH: drop
    }

    // === Restart ===
    {
        let three_hours_ms = 3 * 3_600_000_i64;
        let restart_time = initial_time + interval + three_hours_ms;
        let clock = Arc::new(FakeClock::new(restart_time));
        let store = CronStore::load(store_path).expect("reload store");
        let store = Arc::new(tokio::sync::Mutex::new(store));

        let report = run_startup_catchup(&store, clock.as_ref(), None, None)
            .await
            .expect("catchup should succeed");

        assert_eq!(
            report.stale_markers_cleared, 1,
            "catchup should clear stale marker from Phase 1 crash"
        );

        let guard = store.lock().await;
        let job = guard.get_job("phase1-crash").unwrap();
        assert!(
            job.state.running_at_ms.is_none(),
            "running_at_ms should be None after catchup"
        );
    }
}

// ── 3. crash_preserves_store_integrity ──────────────────────────────────

/// Verify that persisted store file is valid JSON and preserves all jobs
/// across reload.
#[tokio::test]
async fn crash_preserves_store_integrity() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let store_path = temp_dir.path().join("cron.json");

    let clock = FakeClock::new(5_000_000);

    // Create store with 5 jobs
    {
        let mut store = CronStore::load(store_path.clone()).expect("load store");
        for i in 0..5 {
            let job = make_job(&format!("integrity-{i}"));
            ops::add_job(&mut store, job, &clock);
        }
        store.persist().expect("persist should succeed");
    }

    // Verify file is valid JSON
    let raw_json = std::fs::read_to_string(&store_path)
        .expect("should be able to read store file");
    let parsed: serde_json::Value =
        serde_json::from_str(&raw_json).expect("store file should be valid JSON");
    let jobs_array = parsed["jobs"]
        .as_array()
        .expect("should have a jobs array");
    assert_eq!(
        jobs_array.len(),
        5,
        "JSON should contain exactly 5 jobs"
    );

    // Reload and verify count
    let store = CronStore::load(store_path).expect("reload store");
    assert_eq!(
        store.job_count(),
        5,
        "reloaded store should have 5 jobs"
    );

    // Verify all job IDs are present
    for i in 0..5 {
        let id = format!("integrity-{i}");
        assert!(
            store.get_job(&id).is_some(),
            "job '{}' should exist after reload",
            id
        );
    }
}
