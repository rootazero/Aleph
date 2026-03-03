//! Scheduler engine for the cron system.
//!
//! Pure functions for schedule computation, backoff, and job state checks.

use chrono::{DateTime, Utc};
use crate::cron::config::{CronJob, ScheduleKind};

/// Exponential backoff schedule for consecutive failures
pub const BACKOFF_SCHEDULE_MS: &[u64] = &[
    30_000,     // 1st failure → 30s
    60_000,     // 2nd → 1 min
    300_000,    // 3rd → 5 min
    900_000,    // 4th → 15 min
    3_600_000,  // 5th+ → 60 min
];

/// Threshold for detecting stuck jobs (2 hours in ms)
pub const STUCK_THRESHOLD_MS: i64 = 2 * 60 * 60 * 1000;

/// Compute the backoff delay based on consecutive failure count.
pub fn compute_backoff_ms(consecutive_failures: u32) -> u64 {
    if consecutive_failures == 0 {
        return 0;
    }
    let idx = (consecutive_failures.saturating_sub(1) as usize)
        .min(BACKOFF_SCHEDULE_MS.len() - 1);
    BACKOFF_SCHEDULE_MS[idx]
}

/// Compute next run time for a job based on its schedule kind.
/// Returns millisecond timestamp or None if the job should not run again.
pub fn compute_next_run_at(job: &CronJob, from: DateTime<Utc>) -> Option<i64> {
    let from_ms = from.timestamp_millis();

    match job.schedule_kind {
        ScheduleKind::Cron => {
            use std::str::FromStr;
            let schedule = cron::Schedule::from_str(&job.schedule).ok()?;

            // Try timezone-aware computation
            if let Some(tz_str) = job.timezone.as_deref() {
                if let Ok(tz) = tz_str.parse::<chrono_tz::Tz>() {
                    let local_now = from.with_timezone(&tz);
                    return schedule
                        .after(&local_now)
                        .next()
                        .map(|t| t.with_timezone(&Utc).timestamp_millis());
                }
            }

            // Fallback to UTC
            schedule.upcoming(Utc).next().map(|t| t.timestamp_millis())
        }
        ScheduleKind::Every => {
            let interval = job.every_ms?;
            if interval <= 0 { return None; }
            Some(from_ms + interval)
        }
        ScheduleKind::At => {
            let target = job.at_time?;
            if target > from_ms { Some(target) } else { None }
        }
    }
}

/// Check if a one-shot job has already been completed.
pub fn is_completed_oneshot(job: &CronJob) -> bool {
    job.schedule_kind == ScheduleKind::At && job.last_run_at.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_zero_failures() {
        assert_eq!(compute_backoff_ms(0), 0);
    }

    #[test]
    fn test_backoff_schedule() {
        assert_eq!(compute_backoff_ms(1), 30_000);
        assert_eq!(compute_backoff_ms(2), 60_000);
        assert_eq!(compute_backoff_ms(3), 300_000);
        assert_eq!(compute_backoff_ms(4), 900_000);
        assert_eq!(compute_backoff_ms(5), 3_600_000);
        assert_eq!(compute_backoff_ms(100), 3_600_000);
    }

    #[test]
    fn test_next_run_every() {
        let mut job = CronJob::new("T", "unused", "main", "p");
        job.schedule_kind = ScheduleKind::Every;
        job.every_ms = Some(60_000);
        let now = Utc::now();
        let next = compute_next_run_at(&job, now).unwrap();
        assert_eq!(next, now.timestamp_millis() + 60_000);
    }

    #[test]
    fn test_next_run_at_future() {
        let mut job = CronJob::new("T", "unused", "main", "p");
        job.schedule_kind = ScheduleKind::At;
        let future = Utc::now().timestamp_millis() + 3_600_000;
        job.at_time = Some(future);
        assert_eq!(compute_next_run_at(&job, Utc::now()).unwrap(), future);
    }

    #[test]
    fn test_next_run_at_past() {
        let mut job = CronJob::new("T", "unused", "main", "p");
        job.schedule_kind = ScheduleKind::At;
        job.at_time = Some(Utc::now().timestamp_millis() - 3_600_000);
        assert!(compute_next_run_at(&job, Utc::now()).is_none());
    }

    #[test]
    fn test_is_completed_oneshot() {
        let mut job = CronJob::new("T", "unused", "main", "p");
        job.schedule_kind = ScheduleKind::At;
        assert!(!is_completed_oneshot(&job));
        job.last_run_at = Some(1000);
        assert!(is_completed_oneshot(&job));
    }

    #[test]
    fn test_cron_job_not_oneshot() {
        let mut job = CronJob::new("T", "0 0 * * * *", "main", "p");
        job.last_run_at = Some(1000);
        assert!(!is_completed_oneshot(&job));
    }

    #[test]
    fn test_next_run_cron_expression() {
        let job = CronJob::new("T", "0 0 * * * *", "main", "p");
        let now = Utc::now();
        let next = compute_next_run_at(&job, now);
        assert!(next.is_some());
        assert!(next.unwrap() > now.timestamp_millis());
    }
}
