//! Pure-function scheduling computation.
//!
//! All functions are stateless — they take raw i64 millisecond timestamps
//! and return computed results. No CronJob references, no persistence.

use chrono::{DateTime, Utc};

/// Minimum refire gap to prevent spin loops (2 seconds).
pub const MIN_REFIRE_GAP_MS: i64 = 2_000;

/// Backoff tiers for consecutive errors.
pub const BACKOFF_TIERS_MS: &[i64] = &[
    30_000,    // 1st error → 30s
    60_000,    // 2nd → 1 min
    300_000,   // 3rd → 5 min
    900_000,   // 4th → 15 min
    3_600_000, // 5th+ → 60 min
];

/// Compute the next run time for an "every N ms" schedule, aligned to an anchor.
///
/// - `now_ms`: current time in epoch ms
/// - `every_ms`: interval in ms (must be > 0)
/// - `anchor_ms`: the alignment anchor in epoch ms
/// - `last_run_at_ms`: when the job last started (if ever)
///
/// Returns `None` for zero or negative intervals.
///
/// Alignment formula: `anchor + ceil((now - anchor) / every) * every`
///
/// Special cases:
/// - `now == anchor` → returns `anchor` (fire at anchor point)
/// - Future anchor (anchor > now) → returns `anchor`
/// - Future manual trigger (last_run_at > now) → returns `last_run_at + every`
pub fn compute_next_every(
    now_ms: i64,
    every_ms: i64,
    anchor_ms: i64,
    last_run_at_ms: Option<i64>,
) -> Option<i64> {
    if every_ms <= 0 {
        return None;
    }

    // Future manual trigger: last_run_at is in the future
    if let Some(last) = last_run_at_ms {
        if last > now_ms {
            return Some(last + every_ms);
        }
    }

    // Future anchor: not yet reached
    if anchor_ms > now_ms {
        return Some(anchor_ms);
    }

    // now == anchor: fire at anchor
    if now_ms == anchor_ms {
        return Some(anchor_ms);
    }

    // Anchor-aligned: anchor + ceil((now - anchor) / every) * every
    let elapsed = now_ms - anchor_ms;
    let periods = (elapsed + every_ms - 1) / every_ms; // ceil division
    Some(anchor_ms + periods * every_ms)
}

/// Apply minimum refire gap to prevent spin loops.
///
/// Returns `max(next_run_ms, last_ended_ms + MIN_REFIRE_GAP_MS)`,
/// or `next_run_ms` if `last_ended_ms` is None.
pub fn apply_min_gap(next_run_ms: i64, last_ended_ms: Option<i64>) -> i64 {
    match last_ended_ms {
        Some(ended) => next_run_ms.max(ended + MIN_REFIRE_GAP_MS),
        None => next_run_ms,
    }
}

/// Resolve the anchor timestamp: use explicit if provided, else fall back to created_at.
pub fn resolve_anchor(explicit: Option<i64>, created_at_ms: i64) -> i64 {
    explicit.unwrap_or(created_at_ms)
}

/// Compute next occurrence for a cron expression.
///
/// Uses the `cron` crate for parsing and `chrono_tz` for timezone support.
/// Returns the next occurrence as epoch ms, or `None` if no future occurrence exists.
pub fn compute_next_cron(
    expr: &str,
    tz: Option<&str>,
    from: DateTime<Utc>,
) -> Result<Option<i64>, String> {
    use std::str::FromStr;

    let schedule =
        cron::Schedule::from_str(expr).map_err(|e| format!("invalid cron expression: {e}"))?;

    if let Some(tz_str) = tz {
        let tz_parsed: chrono_tz::Tz = tz_str
            .parse()
            .map_err(|e| format!("invalid timezone '{tz_str}': {e}"))?;
        let local_now = from.with_timezone(&tz_parsed);
        Ok(schedule
            .after(&local_now)
            .next()
            .map(|t| t.with_timezone(&Utc).timestamp_millis()))
    } else {
        Ok(schedule.upcoming(Utc).next().map(|t| t.timestamp_millis()))
    }
}

/// Compute backoff delay based on consecutive error count.
///
/// - 0 errors → 0ms (no delay)
/// - 1 error → 30s, 2 → 60s, 3 → 5min, 4 → 15min, 5+ → 60min
pub fn compute_backoff_ms(consecutive_errors: u32) -> i64 {
    if consecutive_errors == 0 {
        return 0;
    }
    let idx = (consecutive_errors.saturating_sub(1) as usize).min(BACKOFF_TIERS_MS.len() - 1);
    BACKOFF_TIERS_MS[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- compute_next_every --

    #[test]
    fn anchor_aligned_basic() {
        // 10 min after anchor, 30 min interval → next at 30 min mark
        let anchor = 0;
        let now = 10 * 60 * 1000; // 10 min
        let every = 30 * 60 * 1000; // 30 min
        let next = compute_next_every(now, every, anchor, None).unwrap();
        assert_eq!(next, 30 * 60 * 1000); // 30 min mark
    }

    #[test]
    fn anchor_aligned_no_drift_after_slow_execution() {
        // Run at 10:00 finishes 10:07, next is 10:30 not 10:37
        let anchor = 0;
        let every = 30 * 60 * 1000; // 30 min
        // "now" is 10:07 (job just finished)
        let now = 10 * 60 * 1000 + 7 * 60 * 1000; // 17 min
        let next = compute_next_every(now, every, anchor, None).unwrap();
        assert_eq!(next, 30 * 60 * 1000); // 30 min mark, not 17+30=47
    }

    #[test]
    fn anchor_aligned_exactly_on_anchor() {
        let anchor = 1_000_000;
        let next = compute_next_every(anchor, 60_000, anchor, None).unwrap();
        assert_eq!(next, anchor);
    }

    #[test]
    fn anchor_aligned_future_anchor() {
        let now = 1_000;
        let anchor = 5_000;
        let next = compute_next_every(now, 60_000, anchor, None).unwrap();
        assert_eq!(next, anchor);
    }

    #[test]
    fn anchor_aligned_future_manual_trigger() {
        let now = 1_000;
        let last = 2_000; // in the future relative to now
        let every = 60_000;
        let next = compute_next_every(now, every, 0, Some(last)).unwrap();
        assert_eq!(next, last + every);
    }

    #[test]
    fn anchor_aligned_zero_interval() {
        assert!(compute_next_every(1_000, 0, 0, None).is_none());
    }

    #[test]
    fn anchor_aligned_negative_interval() {
        assert!(compute_next_every(1_000, -5_000, 0, None).is_none());
    }

    // -- apply_min_gap --

    #[test]
    fn min_gap_prevents_spin() {
        // Computed next is 500ms after last_ended
        let ended = 10_000;
        let next = ended + 500;
        let safe = apply_min_gap(next, Some(ended));
        assert_eq!(safe, ended + MIN_REFIRE_GAP_MS);
    }

    #[test]
    fn min_gap_no_effect_when_far_enough() {
        let ended = 10_000;
        let next = ended + 60_000; // 60s later
        let safe = apply_min_gap(next, Some(ended));
        assert_eq!(safe, next); // no change
    }

    #[test]
    fn min_gap_no_last_ended() {
        let next = 5_000;
        assert_eq!(apply_min_gap(next, None), next);
    }

    // -- resolve_anchor --

    #[test]
    fn resolve_anchor_explicit() {
        assert_eq!(resolve_anchor(Some(42_000), 10_000), 42_000);
    }

    #[test]
    fn resolve_anchor_fallback() {
        assert_eq!(resolve_anchor(None, 10_000), 10_000);
    }

    // -- compute_backoff_ms --

    #[test]
    fn backoff_zero_errors() {
        assert_eq!(compute_backoff_ms(0), 0);
    }

    #[test]
    fn backoff_tiers() {
        assert_eq!(compute_backoff_ms(1), 30_000);
        assert_eq!(compute_backoff_ms(2), 60_000);
        assert_eq!(compute_backoff_ms(3), 300_000);
        assert_eq!(compute_backoff_ms(4), 900_000);
        assert_eq!(compute_backoff_ms(5), 3_600_000);
    }

    #[test]
    fn backoff_clamps_at_max() {
        assert_eq!(compute_backoff_ms(100), 3_600_000);
        assert_eq!(compute_backoff_ms(u32::MAX), 3_600_000);
    }

    // -- compute_next_cron --

    #[test]
    fn cron_next_basic() {
        // Every hour at minute 0
        let from = Utc::now();
        let result = compute_next_cron("0 0 * * * *", None, from);
        assert!(result.is_ok());
        let next = result.unwrap();
        assert!(next.is_some());
        assert!(next.unwrap() > from.timestamp_millis());
    }

    #[test]
    fn cron_invalid_expression() {
        let result = compute_next_cron("not a cron", None, Utc::now());
        assert!(result.is_err());
    }

    #[test]
    fn cron_invalid_timezone() {
        let result = compute_next_cron("0 0 * * * *", Some("Not/A/Timezone"), Utc::now());
        assert!(result.is_err());
    }
}

// ── Regression tests ──────────────────────────────────────────────────

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use crate::cron::config::{CronJob, ScheduleKind};
    use crate::cron::service::ops::{recompute_next_run_maintenance, recompute_next_run_full};

    fn make_test_job() -> CronJob {
        let mut job = CronJob::new(
            "regression-test",
            "agent-1",
            "test prompt",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.created_at = 100_000;
        job
    }

    /// OpenClaw Bug #13992: maintenance recompute must not advance past-due jobs.
    ///
    /// If a job is past-due (next_run_at_ms < now), the maintenance recompute
    /// must leave it alone so the timer loop can pick it up and execute it.
    /// Advancing it to a future time would silently skip the missed execution.
    #[test]
    fn regression_13992_maintenance_recompute_no_advance() {
        let clock = FakeClock::new(1_000_000_000);
        let mut job = make_test_job();
        job.state.next_run_at_ms = Some(500_000_000); // Past due

        recompute_next_run_maintenance(&mut job, &clock);
        assert_eq!(
            job.state.next_run_at_ms,
            Some(500_000_000),
            "maintenance recompute must NOT advance past-due jobs"
        );
    }

    /// OpenClaw Bug #17821: MIN_REFIRE_GAP prevents spin loops.
    ///
    /// When a job finishes and the computed next run time is very close to
    /// the end time, we must enforce a minimum gap to prevent the system
    /// from spinning (executing the same job hundreds of times per second).
    #[test]
    fn regression_17821_min_refire_gap() {
        let ended_at = 1_000_000;
        let computed_next = 1_000_500; // 500ms later — too close
        let safe = apply_min_gap(computed_next, Some(ended_at));
        assert!(
            safe >= ended_at + MIN_REFIRE_GAP_MS,
            "min refire gap not enforced: safe={safe}, min={}",
            ended_at + MIN_REFIRE_GAP_MS
        );
    }

    /// OpenClaw Bug #17821 variant: gap should not affect jobs that are
    /// already far enough in the future.
    #[test]
    fn regression_17821_no_unnecessary_delay() {
        let ended_at = 1_000_000;
        let computed_next = ended_at + 60_000; // 60 seconds later — plenty of gap
        let safe = apply_min_gap(computed_next, Some(ended_at));
        assert_eq!(
            safe, computed_next,
            "min refire gap should not delay jobs already far enough in the future"
        );
    }

    /// OpenClaw Bug #13992 complement: full recompute SHOULD advance past-due.
    ///
    /// When a user explicitly modifies a job (update/toggle), the full
    /// recompute must advance to a future time even if currently past-due.
    #[test]
    fn regression_13992_full_recompute_does_advance() {
        let clock = FakeClock::new(1_000_000_000);
        let mut job = make_test_job();
        job.state.next_run_at_ms = Some(500_000_000); // Past due

        recompute_next_run_full(&mut job, &clock);
        let next = job.state.next_run_at_ms.unwrap();
        assert!(
            next >= 1_000_000_000,
            "full recompute should advance past-due to future, got {next}"
        );
    }

    /// Backoff tier boundaries: ensure consecutive errors produce
    /// monotonically increasing delays up to the cap.
    #[test]
    fn regression_backoff_monotonic() {
        let mut prev = 0_i64;
        for errors in 1..=10 {
            let delay = compute_backoff_ms(errors);
            assert!(
                delay >= prev,
                "backoff must be monotonically increasing: errors={errors}, delay={delay}, prev={prev}"
            );
            prev = delay;
        }
    }

    /// Edge case: zero-interval schedules must return None, not divide-by-zero.
    #[test]
    fn regression_zero_interval_no_panic() {
        let result = compute_next_every(1_000_000, 0, 0, None);
        assert!(result.is_none(), "zero interval must return None");
    }

    /// Edge case: very large timestamps should not overflow.
    #[test]
    fn regression_large_timestamp_no_overflow() {
        // Year 2100 in ms
        let now = 4_102_444_800_000_i64;
        let every = 3_600_000_i64; // 1 hour
        let anchor = 0_i64;
        let result = compute_next_every(now, every, anchor, None);
        assert!(result.is_some(), "large timestamp should not overflow");
        assert!(
            result.unwrap() >= now,
            "result should be at or after now"
        );
    }
}
