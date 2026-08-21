//! Local-calendar period boundaries for spend ceilings.
//!
//! [`period_start_ms`] / [`period_end_ms`] answer "which window is this
//! spend event in": the half-open `[start, end)` millisecond range a
//! [`SpendPeriod`] resolves to around a given instant. The ledger keys rows
//! by [`period_start_ms`] and the spend verdict uses both to decide whether
//! a principal is still inside their ceiling for the period containing
//! "now".
//!
//! # Local time, not UTC — a deliberate tradeoff
//!
//! Boundaries are computed in the machine's local timezone, not UTC. A
//! machine whose timezone changes mid-period will see one short or long
//! period as a result of that choice — an accepted, rare cost. The
//! alternative, UTC boundaries, would put the reset in the middle of the
//! workday for most of the world's timezones, which is the failure mode
//! that actually matters for a *spend ceiling*: a principal hitting a hard
//! stop mid-shift because "today" reset six hours ago in a timezone nobody
//! in this deployment is in. Local was chosen over UTC for that reason.
//!
//! # Why the timezone is a parameter, not `std::env::set_var("TZ")`
//!
//! [`period_start_ms`] / [`period_end_ms`] hard-code `chrono::Local`, but
//! the logic underneath ([`period_start_ms_in`] / [`period_end_ms_in`])
//! takes the timezone as a generic parameter instead of reading process
//! state. That is deliberate — do not "simplify" it back into a test that
//! calls `std::env::set_var("TZ", ..)`:
//!
//! - `set_var` is process-global, and `cargo test` runs tests in parallel
//!   in the same process — a DST test that sets `TZ` can read (or race)
//!   whatever zone a sibling test just set, or just set.
//! - it is `unsafe` as of the 2024 edition, and chrono caches the local
//!   zone on some platforms, so an assertion about DST behavior could
//!   silently keep observing the *host machine's* zone regardless of what
//!   the test just set — a test that passes for the wrong reason, which is
//!   worse than no test.
//!
//! Tests instead pass an explicit [`chrono_tz::Tz`] value.

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};

use crate::config::types::policies::SpendPeriod;

/// How many past periods (in addition to the current one) the spend ledger
/// retains before a sweep may delete rows.
///
/// Kept at 3 (current + 2 prior): one prior period absorbs the ambiguity
/// right at a boundary — a host whose timezone or clock is adjusted can
/// briefly disagree with itself about which period "now" falls in — and a
/// second prior period leaves a full historical cycle available for
/// whoever is investigating a spend dispute before the sweep removes it.
/// Ledger growth is bounded at `3 ×` the row count of the busiest single
/// period, indefinitely, regardless of how long the process has been
/// running.
pub const RETENTION_PERIODS: u32 = 3;

/// Start of the spend period containing `now_ms`, in the system's local
/// timezone.
///
/// Delegates to [`period_start_ms_in`] with `chrono::Local` — see the
/// module doc for why production code should call this (and
/// [`period_end_ms`]) rather than the `_in` variants directly.
#[must_use]
pub fn period_start_ms(now_ms: i64, period: SpendPeriod) -> i64 {
    period_start_ms_in(now_ms, period, &chrono::Local)
}

/// End of the spend period containing `now_ms` — equivalently, the start of
/// the *next* period — in the system's local timezone.
#[must_use]
pub fn period_end_ms(now_ms: i64, period: SpendPeriod) -> i64 {
    period_end_ms_in(now_ms, period, &chrono::Local)
}

/// Start of the spend period containing `now_ms`, in `tz`.
///
/// `Day` periods start at local midnight of the same calendar date;
/// `Month` periods start at local midnight of the 1st of the same calendar
/// month.
#[must_use]
pub fn period_start_ms_in<Tz: TimeZone>(now_ms: i64, period: SpendPeriod, tz: &Tz) -> i64 {
    // Ask the local calendar, not epoch-millisecond arithmetic: the date
    // component of `now` in `tz`, then the calendar day (or month) that
    // date belongs to. `local_midnight` resolves the DST edges (see its
    // own doc) rather than assuming every local midnight exists exactly
    // once.
    let date = to_local(now_ms, tz).date_naive();
    let period_start_date = match period {
        SpendPeriod::Day => date,
        SpendPeriod::Month => first_of_month(date),
    };
    local_midnight(tz, period_start_date).timestamp_millis()
}

/// End of the spend period containing `now_ms` — equivalently, the start of
/// the *next* period — in `tz`.
#[must_use]
pub fn period_end_ms_in<Tz: TimeZone>(now_ms: i64, period: SpendPeriod, tz: &Tz) -> i64 {
    // Same calendar-first approach as `period_start_ms_in`: find the next
    // period's first calendar date, then resolve *its* local midnight
    // through the same DST-aware path — never "start + fixed duration",
    // which is exactly what collapses or inverts the window on a 23h or
    // 25h local day (see `local_midnight`'s doc and the DST test above).
    let date = to_local(now_ms, tz).date_naive();
    let next_period_start_date = match period {
        SpendPeriod::Day => date
            .succ_opt()
            .expect("a spend event's calendar date is never chrono::NaiveDate::MAX"),
        SpendPeriod::Month => next_month(date),
    };
    local_midnight(tz, next_period_start_date).timestamp_millis()
}

/// Convert epoch milliseconds to a `DateTime<Tz>`. Out-of-range `now_ms`
/// falls back to the Unix epoch rather than panicking — this is a
/// defensive floor (P7), not a path any current caller can reach, matching
/// the fallback [`crate::tasks::shared::clock::Clock::now_utc`] uses for
/// the same reason.
fn to_local<Tz: TimeZone>(now_ms: i64, tz: &Tz) -> DateTime<Tz> {
    DateTime::from_timestamp_millis(now_ms)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap_or_else(Utc::now))
        .with_timezone(tz)
}

/// The first day of `date`'s calendar month.
fn first_of_month(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
        .expect("day 1 of any valid month is itself always a valid date")
}

/// A date in the calendar month after `date`'s (always the 1st — the only
/// day this module ever asks [`first_of_month`] to resolve next).
fn next_month(date: NaiveDate) -> NaiveDate {
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1)
        .expect("month 1..=12 with day 1 is always a valid date")
}

/// Local midnight of `date` in `tz`, resolved through the DST-aware path
/// (`TimeZone::from_local_datetime`) rather than fixed-duration arithmetic
/// — the latter is exactly what produces an `end <= start` boundary on a
/// DST-transition day, since a 23-hour or 25-hour local day breaks any
/// implementation that gets "the next boundary" by adding a constant
/// number of hours instead of asking the calendar.
fn local_midnight<Tz: TimeZone>(tz: &Tz, date: NaiveDate) -> DateTime<Tz> {
    let naive = date
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is always a valid NaiveTime");
    if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
        return dt;
    }
    // Local midnight does not exist for this date in this zone. This is
    // not the ordinary spring-forward/fall-back case (those move the clock
    // by 1-2 hours, essentially never across midnight) — it is the rarer
    // case of a national calendar realignment dropping a whole day (Samoa
    // skipped 30 Dec 2011 entirely when it crossed the international date
    // line). Walk forward minute by minute until wall-clock time resumes
    // existing; bounded at 48h so this can never loop forever. If even
    // that fails — no recorded jump is that large — read the wall clock as
    // UTC instead of panicking: a slightly-wrong boundary beats crashing
    // the spend gate.
    let mut probe = naive;
    for _ in 0..48 * 60 {
        probe += chrono::Duration::minutes(1);
        if let Some(dt) = tz.from_local_datetime(&probe).earliest() {
            return dt;
        }
    }
    Utc.from_utc_datetime(&naive).with_timezone(tz)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// A day window starts at **local** midnight, not UTC midnight. Pick a
    /// `now` where the UTC calendar date has already rolled to the next
    /// day while the local (America/New_York, EST = UTC-5 in January)
    /// calendar date has not, so a UTC-date implementation and a
    /// local-date implementation disagree.
    #[test]
    fn day_window_starts_at_local_midnight_not_utc_midnight() {
        let tz = chrono_tz::America::New_York;
        // 2024-01-15T03:00:00Z = 2024-01-14T22:00:00 EST: UTC says the
        // 15th, local says the 14th.
        let now_ms = Utc
            .with_ymd_and_hms(2024, 1, 15, 3, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();

        let start = period_start_ms_in(now_ms, SpendPeriod::Day, &tz);

        let expected_local_midnight = tz
            .with_ymd_and_hms(2024, 1, 14, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let utc_midnight_same_calendar_day = Utc
            .with_ymd_and_hms(2024, 1, 15, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();

        assert_eq!(start, expected_local_midnight);
        assert_ne!(start, utc_midnight_same_calendar_day);
    }

    /// A month window rolls Jan 31 -> Feb 1, not "31 days later" — and
    /// Feb 2024 (a leap year, 29 days) -> Mar 1, not "+30" or "+31" days,
    /// which is the case that actually distinguishes calendar rollover
    /// from duration arithmetic.
    #[test]
    fn month_window_rolls_jan_31_to_feb_1_not_31_days_later() {
        let tz = chrono_tz::UTC;

        let jan_now_ms = tz
            .with_ymd_and_hms(2024, 1, 31, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let jan_start = period_start_ms_in(jan_now_ms, SpendPeriod::Month, &tz);
        let jan_end = period_end_ms_in(jan_now_ms, SpendPeriod::Month, &tz);
        assert_eq!(
            jan_start,
            tz.with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
                .single()
                .unwrap()
                .timestamp_millis()
        );
        assert_eq!(
            jan_end,
            tz.with_ymd_and_hms(2024, 2, 1, 0, 0, 0)
                .single()
                .unwrap()
                .timestamp_millis()
        );

        let feb_now_ms = tz
            .with_ymd_and_hms(2024, 2, 15, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let feb_end = period_end_ms_in(feb_now_ms, SpendPeriod::Month, &tz);
        assert_eq!(
            feb_end,
            tz.with_ymd_and_hms(2024, 3, 1, 0, 0, 0)
                .single()
                .unwrap()
                .timestamp_millis()
        );
    }

    /// Across every real hour of a DST-transition day (both directions —
    /// the 23-hour spring-forward day and the 25-hour fall-back day),
    /// `period_end_ms > period_start_ms` always holds. Hours-arithmetic
    /// ("start + 24h") is exactly what breaks here: on the 23-hour day it
    /// overshoots the next boundary, and computed the same way for `start`
    /// itself (as a truncation of `now`) it can put `start` after the true
    /// local midnight, collapsing or inverting the window.
    #[test]
    fn dst_transition_day_end_always_after_start() {
        let tz = chrono_tz::America::New_York;
        let hour_ms = 3_600_000i64;
        // 2024-03-10: US spring-forward — the local day is 23h long
        // (02:00 -> 03:00 does not exist).
        // 2024-11-03: US fall-back — the local day is 25h long (01:00
        // happens twice).
        for (year, month, day, expected_day_length_ms) in
            [(2024, 3, 10, 23 * hour_ms), (2024, 11, 3, 25 * hour_ms)]
        {
            // Local midnight always exists in this zone — its DST
            // transitions move the clock at 02:00, never at 00:00 — so
            // this anchor point is unaffected by the hazard under test.
            let local_midnight_ms = tz
                .with_ymd_and_hms(year, month, day, 0, 0, 0)
                .single()
                .unwrap()
                .timestamp_millis();

            // The discriminating assertion: the transition day is NOT 24
            // hours long. `period_end_ms > period_start_ms` alone is
            // satisfied by any implementation that adds a positive
            // duration — including "start + 24h", which is exactly the
            // fixed-duration-arithmetic bug this test exists to catch.
            // Only the exact length pins it down.
            let start = period_start_ms_in(local_midnight_ms, SpendPeriod::Day, &tz);
            let end = period_end_ms_in(local_midnight_ms, SpendPeriod::Day, &tz);
            assert_eq!(
                end - start,
                expected_day_length_ms,
                "{year}-{month:02}-{day:02}: local day should be {}h long, was {}h",
                expected_day_length_ms / hour_ms,
                (end - start) as f64 / hour_ms as f64
            );

            // Cheap invariant, checked across every real hour landing on
            // (or adjacent to) the transition day: start/end never invert
            // or collapse regardless of which instant `now` is.
            for hour in 0..24i64 {
                let now_ms = local_midnight_ms + hour * hour_ms;
                let start = period_start_ms_in(now_ms, SpendPeriod::Day, &tz);
                let end = period_end_ms_in(now_ms, SpendPeriod::Day, &tz);
                assert!(
                    end > start,
                    "{year}-{month:02}-{day:02} hour {hour}: start={start} end={end} now={now_ms}"
                );
            }
        }
    }

    /// `period_start_ms(period_end_ms(t))` is the **next** window's start:
    /// feeding a period's own end back in as `now` must resolve to that
    /// same instant, proving the windows tile with no gap and no overlap.
    /// Anchored on a DST-transition day so the tiling property is checked
    /// under the same hazard the boundary computation itself has to
    /// survive.
    #[test]
    fn period_start_of_period_end_is_the_next_windows_start() {
        let tz = chrono_tz::America::New_York;
        for period in [SpendPeriod::Day, SpendPeriod::Month] {
            let now_ms = tz
                .with_ymd_and_hms(2024, 3, 10, 12, 0, 0)
                .single()
                .unwrap()
                .timestamp_millis();

            let end = period_end_ms_in(now_ms, period, &tz);
            let next_start = period_start_ms_in(end, period, &tz);

            assert_eq!(
                end, next_start,
                "{period:?}: period_end should already be the next period's start"
            );
        }
    }
}
