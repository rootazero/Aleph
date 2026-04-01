//! P3 Scheduling Precision Probes — 6 scenarios verifying anchor alignment,
//! stagger spread, backoff, maintenance safety, one-shot semantics, and refire gap.

use alephcore::cron::config::{CronJob, ErrorReason, ScheduleKind};
use alephcore::cron::schedule::MIN_REFIRE_GAP_MS;

use super::harness::CronTestHarness;
use super::mock_executor::MockBehavior;

// ── 1. anchor_alignment_no_drift ────────────────────────────────────

#[tokio::test]
async fn anchor_alignment_no_drift() {
    let h = CronTestHarness::new();
    let interval = 30 * 60 * 1000; // 30 min in ms
    let anchor = h.now();

    // Executor returns Delayed(7 min) to simulate slow execution.
    // Note: the mock doesn't actually advance FakeClock — it only reports duration.
    h.executor.on_job(
        "anchor-1",
        MockBehavior::Delayed {
            delay_ms: 7 * 60 * 1000,
            output: "ok".to_string(),
        },
    );

    let mut job = CronJob::new(
        "anchor-1",
        "test-agent",
        "prompt",
        ScheduleKind::Every {
            every_ms: interval,
            anchor_ms: Some(anchor),
        },
    );
    job.id = "anchor-1".to_string();
    h.add_job(job).await;

    // Run 5 cycles. Advance slightly past each grid point so that
    // the scheduler sees "now > grid point" and computes the NEXT slot.
    for n in 1..=5 {
        let grid_point = anchor + n * interval;

        // Advance 1ms past the grid point so ceil lands on the next slot.
        h.advance_to(grid_point + 1);
        h.tick().await;

        h.assert_execution_count("anchor-1", n as usize);

        // After execution at grid_point+1, maintenance recompute sees
        // now = grid_point+1, so next slot = anchor + (n+1)*interval.
        let state = h.job_state("anchor-1").await;
        let next = state
            .next_run_at_ms
            .expect("should have next_run after execution");
        let expected_next = anchor + (n + 1) * interval;
        assert_eq!(
            next, expected_next,
            "cycle {n}: next_run ({next}) should align to anchor grid ({expected_next})"
        );
    }
}

// ── 2. stagger_spreads_jobs ─────────────────────────────────────────

#[tokio::test]
async fn stagger_spreads_jobs() {
    let h = CronTestHarness::new();
    let stagger_window = 300_000; // 5 min stagger

    // Create 10 jobs with same cron expr + stagger_ms = 300_000
    let mut next_runs = Vec::new();
    for i in 0..10 {
        let id = format!("stagger-{i}");
        let mut job = CronJob::new(
            &id,
            "test-agent",
            format!("prompt for {id}"),
            ScheduleKind::Cron {
                expr: "0 0 * * * *".to_string(),
                tz: None,
                stagger_ms: Some(stagger_window),
            },
        );
        job.id = id.clone();
        h.add_job(job).await;

        let state = h.job_state(&id).await;
        let next = state
            .next_run_at_ms
            .expect(&format!("job {id} should have next_run"));
        next_runs.push(next);
    }

    // Verify not all next_run are identical
    let all_same = next_runs.iter().all(|&t| t == next_runs[0]);
    assert!(
        !all_same,
        "staggered jobs should NOT all have the same next_run_at_ms: {:?}",
        next_runs
    );

    // Verify spread is within the stagger window
    let min = *next_runs.iter().min().unwrap();
    let max = *next_runs.iter().max().unwrap();
    let spread = max - min;
    assert!(
        spread <= stagger_window,
        "spread ({spread}ms) should be <= stagger window ({stagger_window}ms)"
    );
}

// ── 3. backoff_after_errors ─────────────────────────────────────────

#[tokio::test]
async fn backoff_after_errors() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("backoff-1", interval).await;

    // Configure executor to return errors
    h.executor.on_job(
        "backoff-1",
        MockBehavior::Error {
            message: "transient failure".to_string(),
            reason: ErrorReason::Transient("network timeout".to_string()),
        },
    );

    // Fail 3 times, verifying consecutive_errors increments each time
    for i in 1..=3u32 {
        h.advance(interval);
        h.tick().await;

        h.assert_consecutive_errors("backoff-1", i).await;
    }

    // Verify consecutive_errors is 3
    h.assert_consecutive_errors("backoff-1", 3).await;

    // Verify that compute_backoff_ms returns increasing delays
    use alephcore::cron::schedule::compute_backoff_ms;
    let d1 = compute_backoff_ms(1);
    let d2 = compute_backoff_ms(2);
    let d3 = compute_backoff_ms(3);
    assert!(d1 > 0, "backoff for 1 error should be > 0");
    assert!(d2 > d1, "backoff should increase: d2={d2} > d1={d1}");
    assert!(d3 > d2, "backoff should increase: d3={d3} > d2={d2}");
}

// ── 4. maintenance_recompute_safe ───────────────────────────────────

/// Regression #13992: A past-due job must still execute when ticked,
/// not be skipped by recompute.
#[tokio::test]
async fn maintenance_recompute_safe() {
    let h = CronTestHarness::new();
    let interval = 60_000;

    h.add_every_job("maint-1", interval).await;

    // Advance way past due (3x interval)
    h.advance(interval * 3);
    h.tick().await;

    // Job MUST have executed (not skipped by recompute)
    h.assert_executed("maint-1");
    h.assert_execution_count("maint-1", 1);
}

// ── 5. at_job_fires_once ────────────────────────────────────────────

#[tokio::test]
async fn at_job_fires_once() {
    let h = CronTestHarness::new();
    let fire_at = h.now() + 10_000;

    h.add_at_job("once-1", fire_at).await;

    // Advance to fire time and tick
    h.advance(10_000);
    h.tick().await;
    h.assert_execution_count("once-1", 1);

    // Tick many more times — should never fire again
    for _ in 0..5 {
        h.advance(10_000);
        h.tick().await;
    }
    h.assert_execution_count("once-1", 1);
}

// ── 6. min_refire_gap_applied ───────────────────────────────────────

/// MIN_REFIRE_GAP is applied in the Cron schedule path via `apply_min_gap`.
/// Verify that after executing a cron job, the gap between execution end
/// and next_run_at_ms is at least MIN_REFIRE_GAP_MS (2s).
#[tokio::test]
async fn min_refire_gap_applied() {
    let h = CronTestHarness::new();

    // Use a cron expression that fires every minute (very frequent)
    // "0 * * * * *" = at second 0 of every minute
    h.add_cron_job("refire-1", "0 * * * * *").await;

    // Get the next_run, advance to it, and execute
    let state = h.job_state("refire-1").await;
    let first_fire = state.next_run_at_ms.expect("should have next_run");
    h.advance_to(first_fire);
    h.tick().await;
    h.assert_execution_count("refire-1", 1);

    // Check that next_run_at_ms respects MIN_REFIRE_GAP from execution end
    let state = h.job_state("refire-1").await;
    let next = state
        .next_run_at_ms
        .expect("should have next_run after execution");
    let last_ended = state
        .last_run_at_ms
        .map(|started| started + state.last_duration_ms.unwrap_or(0))
        .expect("should have last_run_at_ms");

    let gap = next - last_ended;
    assert!(
        gap >= MIN_REFIRE_GAP_MS,
        "gap ({gap}ms) between end and next_run must be >= MIN_REFIRE_GAP ({MIN_REFIRE_GAP_MS}ms)"
    );
}
