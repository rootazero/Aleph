//! Restart recovery: clear stale running markers and catch up missed jobs.

use tracing::info;

use crate::sync_primitives::Arc;

use crate::tasks::cron::config::{CronJob, ScheduleKind};
use crate::tasks::cron::service::ops::{advance_next_run, recompute_next_run_full};
use crate::tasks::cron::store::CronStore;
use crate::tasks::shared::clock::Clock;
use crate::tasks::shared::schedule::compute_next_cron;

/// Summary of what the catchup pass did.
#[derive(Debug, Default)]
pub struct CatchupReport {
    pub stale_markers_cleared: usize,
    pub immediate_count: usize,
    pub deferred_count: usize,
    /// Recurring jobs that were too stale to catch up and were fast-forwarded.
    pub fast_forwarded: usize,
}

/// Default stale threshold: 2 hours in milliseconds.
const DEFAULT_STALE_THRESHOLD_MS: i64 = 7_200_000;

/// Default maximum number of missed jobs to execute immediately.
const DEFAULT_MAX_MISSED: usize = 5;

/// Default stagger interval between deferred jobs: 30 seconds.
const DEFAULT_STAGGER_MS: i64 = 30_000;

/// Minimum missed-run grace window: 2 minutes.
const MIN_GRACE_MS: i64 = 120_000;

/// Maximum missed-run grace window: 2 hours.
const MAX_GRACE_MS: i64 = 7_200_000;

/// Compute the missed-run grace window for a job.
///
/// A recurring job whose `next_run_at_ms` is older than this window is
/// hopelessly stale and is fast-forwarded instead of replaying out-of-date
/// runs. The window is half the schedule period, clamped to `[2min, 2h]`.
///
/// Returns `None` for one-shot `At` jobs — they have no period and are always
/// caught up (the user asked for a specific wall-clock event).
fn compute_grace_ms(job: &CronJob, now: i64) -> Option<i64> {
    let period_ms = match &job.schedule_kind {
        ScheduleKind::At { .. } => return None,
        ScheduleKind::Every { every_ms, .. } => *every_ms,
        ScheduleKind::Cron { expr, tz, .. } => {
            // Period ≈ gap between the next two occurrences from `now`.
            let from = chrono::DateTime::from_timestamp_millis(now)?;
            let n1 = compute_next_cron(expr, tz.as_deref(), from).ok()??;
            let from2 = chrono::DateTime::from_timestamp_millis(n1)?;
            let n2 = compute_next_cron(expr, tz.as_deref(), from2).ok()??;
            n2 - n1
        }
    };
    if period_ms <= 0 {
        return Some(MAX_GRACE_MS);
    }
    Some((period_ms / 2).clamp(MIN_GRACE_MS, MAX_GRACE_MS))
}

/// Run startup catchup: clear stale running markers and reschedule missed jobs.
///
/// - Stale markers: if `running_at_ms` is set and `now - running_at_ms > max(7_200_000, timeout_ms * 2)`, clear it.
/// - Missed jobs: enabled, not running, `next_run_at_ms <= now`.
///   - Recurring jobs staler than their grace window are fast-forwarded
///     (the stale run is skipped) rather than replayed.
///   - The rest are sorted by `next_run_at_ms` ASC; the first `max_missed`
///     run immediately, the rest are deferred with stagger.
pub async fn run_startup_catchup<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    max_missed: Option<usize>,
    stagger_ms: Option<i64>,
    default_timeout_ms: i64,
) -> Result<CatchupReport, String> {
    let now = clock.now_ms();
    let max_missed = max_missed.unwrap_or(DEFAULT_MAX_MISSED);
    let stagger_interval = stagger_ms.unwrap_or(DEFAULT_STAGGER_MS);

    let mut guard = store.lock().await;
    guard.reload_if_changed()?;

    let mut report = CatchupReport::default();
    let mut changed = false;

    // Phase 1: Clear stale running markers
    for job in guard.jobs_mut().iter_mut() {
        if let Some(running_at) = job.state.running_at_ms {
            let stale_threshold =
                DEFAULT_STALE_THRESHOLD_MS.max(job.effective_timeout_ms(default_timeout_ms) * 2);
            if now.saturating_sub(running_at) > stale_threshold {
                job.state.running_at_ms = None;
                report.stale_markers_cleared += 1;
                changed = true;
            }
        }
    }

    // Phase 1.5: Recompute next_run_at_ms for enabled jobs that have None
    // (e.g. after manual DB edits or data migration)
    for job in guard.jobs_mut().iter_mut() {
        if job.enabled && job.state.next_run_at_ms.is_none() {
            recompute_next_run_full(job, clock);
            changed = true;
        }
    }

    // Phase 2: Classify past-due jobs.
    //  - within grace window → collect as "missed" (catch up)
    //  - beyond grace window → fast-forward (skip the stale run)
    let mut missed_indices: Vec<(usize, i64)> = Vec::new();
    {
        let jobs = guard.jobs_mut();
        for (i, job) in jobs.iter_mut().enumerate() {
            if !job.enabled || job.state.running_at_ms.is_some() {
                continue;
            }
            let Some(next) = job.state.next_run_at_ms else {
                continue;
            };
            if next > now {
                continue; // not due
            }
            match compute_grace_ms(job, now) {
                Some(grace) if now - next > grace => {
                    // Hopelessly stale recurring run: fast-forward past it.
                    advance_next_run(job, clock);
                    report.fast_forwarded += 1;
                    changed = true;
                }
                _ => {
                    // Within grace, or a one-shot job: catch it up.
                    missed_indices.push((i, next));
                }
            }
        }
    }

    // Sort by next_run_at_ms ASC
    missed_indices.sort_by_key(|&(_, next)| next);

    // Phase 3: Split into immediate and deferred
    let total_missed = missed_indices.len();
    if total_missed > 0 {
        let immediate_count = total_missed.min(max_missed);
        report.immediate_count = immediate_count;
        report.deferred_count = total_missed.saturating_sub(max_missed);

        // Deferred jobs get staggered next_run_at_ms
        let jobs = guard.jobs_mut();
        for (rank, &(idx, _)) in missed_indices.iter().enumerate() {
            if rank >= max_missed {
                let deferred_rank = rank - max_missed;
                jobs[idx].state.next_run_at_ms =
                    Some(now + (deferred_rank as i64 + 1) * stagger_interval);
                changed = true;
            }
            // Immediate jobs keep their existing next_run_at_ms (already <= now)
        }
    }

    if changed {
        guard.persist()?;
    }

    info!(
        stale_cleared = report.stale_markers_cleared,
        immediate = report.immediate_count,
        deferred = report.deferred_count,
        fast_forwarded = report.fast_forwarded,
        "startup catchup complete"
    );

    Ok(report)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::config::{CronJob, ScheduleKind};
    use crate::tasks::cron::service::ops::add_job;
    use crate::tasks::shared::clock::testing::FakeClock;
    use tempfile::TempDir;

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

    #[tokio::test]
    async fn clears_stale_running_markers() {
        let (store, _dir) = make_store();
        // now = 20_000_000 (20M ms)
        let clock = FakeClock::new(20_000_000);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("stale-job");
            job.created_at = 1_000_000;
            let id = add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut(&id).unwrap();
            // running_at 3 hours ago (> 2h threshold)
            j.state.running_at_ms = Some(20_000_000 - 3 * 3_600_000);
            j.state.next_run_at_ms = Some(25_000_000); // future, not missed
            guard.persist().unwrap();
        }

        let report = run_startup_catchup(&store, &clock, None, None, TEST_TIMEOUT_MS)
            .await
            .unwrap();

        assert_eq!(report.stale_markers_cleared, 1);

        let guard = store.lock().await;
        let job = guard.get_job("stale-job").unwrap();
        assert!(
            job.state.running_at_ms.is_none(),
            "stale running marker should be cleared"
        );
    }

    #[tokio::test]
    async fn staggers_deferred_missed_jobs() {
        let (store, _dir) = make_store();
        let now = 10_000_000_i64;
        let clock = FakeClock::new(now);

        // Create 8 missed jobs
        {
            let mut guard = store.lock().await;
            for i in 0..8 {
                let mut job = make_test_job(&format!("missed-{i}"));
                job.created_at = 1_000_000;
                add_job(&mut guard, job, &clock);
                let j = guard.get_job_mut(&format!("missed-{i}")).unwrap();
                // All past due, ordered by next_run
                j.state.next_run_at_ms = Some(now - 8_000 + i as i64 * 1_000);
            }
            guard.persist().unwrap();
        }

        let stagger = 10_000_i64;
        let report = run_startup_catchup(&store, &clock, Some(3), Some(stagger), TEST_TIMEOUT_MS)
            .await
            .unwrap();

        assert_eq!(report.immediate_count, 3);
        assert_eq!(report.deferred_count, 5);

        // Verify deferred jobs got staggered times
        let guard = store.lock().await;
        for i in 3..8 {
            let job = guard.get_job(&format!("missed-{i}")).unwrap();
            let expected = now + ((i - 3) as i64 + 1) * stagger;
            assert_eq!(
                job.state.next_run_at_ms,
                Some(expected),
                "deferred job missed-{i} should have staggered time"
            );
        }

        // Immediate jobs keep their original past-due times
        for i in 0..3 {
            let job = guard.get_job(&format!("missed-{i}")).unwrap();
            let next = job.state.next_run_at_ms.unwrap();
            assert!(
                next <= now,
                "immediate job missed-{i} should keep past-due time, got {next}"
            );
        }
    }

    /// OpenClaw Bug #17554: stale running markers cleared on startup.
    ///
    /// If the server crashes while a job is running, the running_at_ms marker
    /// becomes stale. On restart, catchup must detect and clear it so the
    /// job can be rescheduled.
    #[tokio::test]
    async fn regression_17554_stale_running_marker() {
        let (store, _dir) = make_store();
        let now = 20_000_000_i64;
        let clock = FakeClock::new(now);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("stale-runner");
            job.created_at = 1_000_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("stale-runner").unwrap();
            // Set running_at 3 hours ago (well past the 2h threshold)
            j.state.running_at_ms = Some(now - 3 * 3_600_000);
            j.state.next_run_at_ms = Some(now + 60_000); // future, not missed
            guard.persist().unwrap();
        }

        let report = run_startup_catchup(&store, &clock, None, None, TEST_TIMEOUT_MS)
            .await
            .unwrap();

        assert_eq!(
            report.stale_markers_cleared, 1,
            "stale running marker should be cleared"
        );

        let guard = store.lock().await;
        let job = guard.get_job("stale-runner").unwrap();
        assert!(
            job.state.running_at_ms.is_none(),
            "running_at_ms should be None after clearing stale marker"
        );
    }

    /// OpenClaw Bug #18892: startup catchup respects max_missed limit.
    ///
    /// When many jobs are missed during downtime, only max_missed should
    /// run immediately to prevent thundering herd. The rest must be deferred
    /// with stagger intervals.
    #[tokio::test]
    async fn regression_18892_startup_overload() {
        let (store, _dir) = make_store();
        let now = 10_000_000_i64;
        let clock = FakeClock::new(now);

        // Create 10 missed jobs
        {
            let mut guard = store.lock().await;
            for i in 0..10 {
                let mut job = make_test_job(&format!("overload-{i}"));
                job.created_at = 1_000_000;
                add_job(&mut guard, job, &clock);
                let j = guard.get_job_mut(&format!("overload-{i}")).unwrap();
                j.state.next_run_at_ms = Some(now - 10_000 + i as i64 * 100);
            }
            guard.persist().unwrap();
        }

        let max_missed = 3;
        let report = run_startup_catchup(
            &store,
            &clock,
            Some(max_missed),
            Some(30_000),
            TEST_TIMEOUT_MS,
        )
        .await
        .unwrap();

        assert_eq!(
            report.immediate_count, 3,
            "only max_missed=3 should be immediate"
        );
        assert_eq!(report.deferred_count, 7, "remaining 7 should be deferred");

        // Verify deferred jobs have future next_run times
        let guard = store.lock().await;
        let mut deferred_count = 0;
        for i in 0..10 {
            let job = guard.get_job(&format!("overload-{i}")).unwrap();
            if let Some(next) = job.state.next_run_at_ms {
                if next > now {
                    deferred_count += 1;
                }
            }
        }
        assert_eq!(
            deferred_count, 7,
            "7 jobs should have deferred (future) next_run times"
        );
    }

    #[tokio::test]
    async fn no_changes_when_nothing_missed() {
        let (store, _dir) = make_store();
        let now = 10_000_000_i64;
        let clock = FakeClock::new(now);

        // Create jobs with future next_run times
        {
            let mut guard = store.lock().await;
            for i in 0..3 {
                let mut job = make_test_job(&format!("future-{i}"));
                job.created_at = 1_000_000;
                add_job(&mut guard, job, &clock);
                let j = guard.get_job_mut(&format!("future-{i}")).unwrap();
                j.state.next_run_at_ms = Some(now + 60_000 * (i as i64 + 1));
            }
            guard.persist().unwrap();
        }

        let report = run_startup_catchup(&store, &clock, None, None, TEST_TIMEOUT_MS)
            .await
            .unwrap();

        assert_eq!(report.stale_markers_cleared, 0);
        assert_eq!(report.immediate_count, 0);
        assert_eq!(report.deferred_count, 0);
    }

    /// C2: a recurring job missed by far more than its grace window is
    /// fast-forwarded instead of replaying a stale run.
    #[tokio::test]
    async fn fast_forwards_hopelessly_stale_recurring_job() {
        let (store, _dir) = make_store();
        let now = 100_000_000_i64;
        let clock = FakeClock::new(now);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("ancient"); // Every 60s → grace = 2 min
            job.created_at = 1_000_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("ancient").unwrap();
            // Missed by 3 hours — far beyond the 2-minute grace window.
            j.state.next_run_at_ms = Some(now - 3 * 3_600_000);
            guard.persist().unwrap();
        }

        let report = run_startup_catchup(&store, &clock, None, None, TEST_TIMEOUT_MS)
            .await
            .unwrap();

        assert_eq!(
            report.fast_forwarded, 1,
            "stale job should be fast-forwarded"
        );
        assert_eq!(report.immediate_count, 0, "no stale run should be replayed");

        let guard = store.lock().await;
        let job = guard.get_job("ancient").unwrap();
        let next = job.state.next_run_at_ms.unwrap();
        assert!(
            next > now,
            "fast-forwarded job runs in the future, got {next}"
        );
    }

    /// C2: a recurring job missed within its grace window is still caught up.
    #[tokio::test]
    async fn catches_up_recently_missed_recurring_job() {
        let (store, _dir) = make_store();
        let now = 100_000_000_i64;
        let clock = FakeClock::new(now);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("recent"); // Every 60s → grace = 2 min
            job.created_at = 1_000_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("recent").unwrap();
            // Missed by 30 seconds — within the 2-minute grace window.
            j.state.next_run_at_ms = Some(now - 30_000);
            guard.persist().unwrap();
        }

        let report = run_startup_catchup(&store, &clock, None, None, TEST_TIMEOUT_MS)
            .await
            .unwrap();

        assert_eq!(report.fast_forwarded, 0);
        assert_eq!(
            report.immediate_count, 1,
            "recently-missed job is caught up"
        );
    }
}
