//! Restart recovery: clear stale running markers and catch up missed jobs.

use std::sync::Arc;

use tracing::info;

use crate::cron::clock::Clock;
use crate::cron::store::CronStore;

/// Summary of what the catchup pass did.
#[derive(Debug, Default)]
pub struct CatchupReport {
    pub stale_markers_cleared: usize,
    pub immediate_count: usize,
    pub deferred_count: usize,
}

/// Default stale threshold: 2 hours in milliseconds.
const DEFAULT_STALE_THRESHOLD_MS: i64 = 7_200_000;

/// Default maximum number of missed jobs to execute immediately.
const DEFAULT_MAX_MISSED: usize = 5;

/// Default stagger interval between deferred jobs: 30 seconds.
const DEFAULT_STAGGER_MS: i64 = 30_000;

/// Run startup catchup: clear stale running markers and reschedule missed jobs.
///
/// - Stale markers: if `running_at_ms` is set and `now - running_at_ms > max(7_200_000, timeout_ms * 2)`, clear it.
/// - Missed jobs: enabled, not running, `next_run_at_ms <= now`. Sorted by `next_run_at_ms` ASC.
/// - First `max_missed` are kept as-is (immediate). Rest are deferred with stagger.
pub async fn run_startup_catchup<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    max_missed: Option<usize>,
    stagger_ms: Option<i64>,
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
            let stale_threshold = DEFAULT_STALE_THRESHOLD_MS.max(job.timeout_ms() * 2);
            if now - running_at > stale_threshold {
                job.state.running_at_ms = None;
                report.stale_markers_cleared += 1;
                changed = true;
            }
        }
    }

    // Phase 2: Collect missed job indices (enabled, not running, past due)
    let mut missed_indices: Vec<(usize, i64)> = Vec::new();
    for (i, job) in guard.jobs().iter().enumerate() {
        if !job.enabled {
            continue;
        }
        if job.state.running_at_ms.is_some() {
            continue;
        }
        if let Some(next) = job.state.next_run_at_ms {
            if next <= now {
                missed_indices.push((i, next));
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
        "startup catchup complete"
    );

    Ok(report)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use crate::cron::config::{CronJob, ScheduleKind};
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

        let report = run_startup_catchup(&store, &clock, None, None)
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
        let report = run_startup_catchup(&store, &clock, Some(3), Some(stagger))
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

        let report = run_startup_catchup(&store, &clock, None, None)
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
        let report = run_startup_catchup(&store, &clock, Some(max_missed), Some(30_000))
            .await
            .unwrap();

        assert_eq!(
            report.immediate_count, 3,
            "only max_missed=3 should be immediate"
        );
        assert_eq!(
            report.deferred_count, 7,
            "remaining 7 should be deferred"
        );

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

        let report = run_startup_catchup(&store, &clock, None, None)
            .await
            .unwrap();

        assert_eq!(report.stale_markers_cleared, 0);
        assert_eq!(report.immediate_count, 0);
        assert_eq!(report.deferred_count, 0);
    }
}
