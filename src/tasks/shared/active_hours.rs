//! Weekly active-hours gate for periodic tasks.
//!
//! Ports openclaw's `src/infra/heartbeat-active-hours.ts`: each weekday
//! carries zero-or-more `(HH:MM, HH:MM)` windows during which the task is
//! allowed to fire. Outside the windows, the timer skips the tick and
//! advances `next_due_ms` to the next window's start. A task with no
//! windows for the current weekday simply rolls to the next allowed
//! weekday — week-long gaps included.
//!
//! The schedule is timezone-aware. Empty / absent timezone defaults to
//! UTC so the calculation stays deterministic regardless of the host's
//! `TZ` env var (a deliberate divergence from openclaw, which used the
//! host's local zone and broke when servers were containerised).

use chrono::{Datelike, NaiveTime, TimeZone, Timelike};
use chrono_tz::Tz;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A weekly schedule of allowed time windows.
///
/// `windows` is a length-7 array indexed by `Weekday::num_days_from_monday()`
/// (Mon = 0 … Sun = 6). Each entry is a list of disjoint, ascending
/// `(start, end)` half-open intervals expressed as minutes-since-midnight
/// (`0..1440`). An empty list means "this weekday is fully off". An
/// `end <= start` interval is rejected at validate time so the
/// "wrap past midnight" case is forbidden — callers must split it into
/// two same-day windows.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActiveHoursSchedule {
    /// IANA timezone name (e.g. `"America/Los_Angeles"`). Empty / unset →
    /// UTC. Invalid names cause every gate check to fall through to "skip
    /// for the next 24 hours" so a misconfigured schedule never silently
    /// fires at the wrong time.
    #[serde(default)]
    pub timezone: Option<String>,

    /// Allowed windows per weekday, Mon..Sun.
    #[serde(default)]
    pub windows: [Vec<TimeWindow>; 7],
}

impl Default for ActiveHoursSchedule {
    fn default() -> Self {
        Self {
            timezone: None,
            // Default = always-on: every weekday has a single 00:00..24:00 window.
            windows: std::array::from_fn(|_| {
                vec![TimeWindow {
                    start_minute: 0,
                    end_minute: 1440,
                }]
            }),
        }
    }
}

/// A half-open `[start, end)` interval expressed as minutes since midnight.
///
/// `end == 1440` means "until end of day"; `end <= start` is invalid.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TimeWindow {
    pub start_minute: u16,
    pub end_minute: u16,
}

impl TimeWindow {
    #[must_use]
    pub const fn contains(&self, minute_of_day: u16) -> bool {
        minute_of_day >= self.start_minute && minute_of_day < self.end_minute
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.start_minute >= self.end_minute {
            return Err(format!(
                "invalid time window: start {} must be < end {}",
                self.start_minute, self.end_minute
            ));
        }
        if self.end_minute > 1440 {
            return Err(format!(
                "invalid time window: end {} > 1440 (24h)",
                self.end_minute
            ));
        }
        Ok(())
    }
}

impl ActiveHoursSchedule {
    /// Resolve the schedule's timezone. Missing → UTC; unparseable → `None`.
    fn tz(&self) -> Option<Tz> {
        match self.timezone.as_deref() {
            None => Some(chrono_tz::UTC),
            Some(s) => s.parse().ok(),
        }
    }

    /// Check every defined window for basic well-formedness.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(tz_str) = self.timezone.as_deref() {
            let _: Tz = tz_str
                .parse()
                .map_err(|e| format!("invalid timezone '{tz_str}': {e}"))?;
        }
        for (i, day) in self.windows.iter().enumerate() {
            for (j, win) in day.iter().enumerate() {
                win.validate()
                    .map_err(|e| format!("weekday {i} window {j}: {e}"))?;
            }
        }
        Ok(())
    }

    /// Is the schedule open at the given wall-clock ms-since-epoch?
    ///
    /// Returns `true` if any window for the current weekday (in the
    /// schedule's timezone) contains the current minute-of-day.
    #[must_use]
    pub fn is_open_at(&self, now_ms: i64) -> bool {
        let tz = match self.tz() {
            Some(tz) => tz,
            None => return false,
        };
        let dt = match tz.timestamp_millis_opt(now_ms).single() {
            Some(dt) => dt,
            None => return false, // ambiguous DST boundary — treat as closed
        };
        let weekday_idx = dt.weekday().num_days_from_monday() as usize;
        let minute_of_day = (dt.hour() * 60 + dt.minute()) as u16;
        self.windows[weekday_idx]
            .iter()
            .any(|w| w.contains(minute_of_day))
    }

    /// Compute the next ms-since-epoch when this schedule opens, starting
    /// strictly after `now_ms`. Returns `None` if no window is defined
    /// anywhere in the week or the configured timezone is unparseable.
    ///
    /// The search is bounded to at most 7 days, so the cost is O(7) per
    /// call regardless of how sparse the schedule is.
    #[must_use]
    pub fn next_open_after(&self, now_ms: i64) -> Option<i64> {
        let tz = self.tz()?;
        let now = tz.timestamp_millis_opt(now_ms).single()?;

        // Start from today; if no remaining window today, advance one day at a time.
        for day_offset in 0..7 {
            let date = (now.date_naive() + chrono::Duration::days(i64::from(day_offset)))
                .and_hms_opt(0, 0, 0)?;
            let weekday_idx = date.weekday().num_days_from_monday() as usize;
            for win in &self.windows[weekday_idx] {
                // Build the window's wall-clock start in the schedule's tz.
                let start_time = NaiveTime::from_hms_opt(
                    u32::from(win.start_minute / 60),
                    u32::from(win.start_minute % 60),
                    0,
                )?;
                let start_naive = date.date().and_time(start_time);
                // A window start landing in a DST spring-forward gap (or a
                // fall-back ambiguity) yields no single local instant. Skip
                // just this window rather than aborting the whole 7-day search
                // — otherwise one unlucky DST boundary would report the gate as
                // permanently closed and suppress the task for a full day.
                let start_dt = match tz.from_local_datetime(&start_naive).single() {
                    Some(dt) => dt,
                    None => continue,
                };
                let start_ms = start_dt.timestamp_millis();
                if start_ms > now_ms {
                    return Some(start_ms);
                }
                // If we're already inside this window, "now+1ms" counts as
                // the next open moment so callers don't get stuck.
                if start_ms <= now_ms {
                    let end_time = NaiveTime::from_hms_opt(
                        u32::from(win.end_minute / 60),
                        u32::from(win.end_minute % 60),
                        0,
                    )?;
                    let end_naive = date.date().and_time(end_time);
                    // Same DST hazard as the window start above: if the end
                    // wall-clock time is unresolvable, treat this window as
                    // already past and keep scanning later windows/days.
                    let end_dt = match tz.from_local_datetime(&end_naive).single() {
                        Some(dt) => dt,
                        None => continue,
                    };
                    if end_dt.timestamp_millis() > now_ms {
                        return Some(now_ms + 1);
                    }
                }
            }
        }
        None
    }
}

/// Convenience helper for callers that just need to push `next_due_ms`
/// forward to the next window boundary. Returns `None` if the schedule
/// has no future opening within the next 7 days.
#[must_use]
pub fn next_due_after_gate(
    schedule: &ActiveHoursSchedule,
    fallback_next_due_ms: i64,
    now_ms: i64,
) -> Option<i64> {
    if schedule.is_open_at(fallback_next_due_ms) {
        Some(fallback_next_due_ms)
    } else {
        // Search from the later of `now` and the fallback so the result is
        // always an actual opening. Using `next_open_after(now).max(fallback)`
        // could land on a `fallback` that itself falls in a closed window.
        schedule.next_open_after(now_ms.max(fallback_next_due_ms))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(year: i32, month: u32, day: u32, h: u32, m: u32) -> i64 {
        chrono::Utc
            .with_ymd_and_hms(year, month, day, h, m, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    fn business_hours_utc() -> ActiveHoursSchedule {
        // Mon-Fri 09:00-17:00 UTC, weekends off.
        let mon_fri = vec![TimeWindow {
            start_minute: 9 * 60,
            end_minute: 17 * 60,
        }];
        let windows: [Vec<TimeWindow>; 7] = [
            mon_fri.clone(),
            mon_fri.clone(),
            mon_fri.clone(),
            mon_fri.clone(),
            mon_fri,
            Vec::new(), // Sat
            Vec::new(), // Sun
        ];
        ActiveHoursSchedule {
            timezone: Some("UTC".into()),
            windows,
        }
    }

    #[test]
    fn default_is_always_open() {
        let sched = ActiveHoursSchedule::default();
        assert!(sched.is_open_at(0));
        assert!(sched.is_open_at(1_700_000_000_000));
    }

    #[test]
    fn is_open_during_business_hours() {
        let sched = business_hours_utc();
        // Wed 2024-01-03 10:00 UTC is inside the window.
        assert!(sched.is_open_at(ms(2024, 1, 3, 10, 0)));
        // Wed 2024-01-03 17:00 UTC is the half-open boundary (exclusive).
        assert!(!sched.is_open_at(ms(2024, 1, 3, 17, 0)));
        // Wed 2024-01-03 08:59 UTC is just before the window.
        assert!(!sched.is_open_at(ms(2024, 1, 3, 8, 59)));
    }

    #[test]
    fn is_closed_on_weekend() {
        let sched = business_hours_utc();
        // Sat 2024-01-06 12:00 UTC.
        assert!(!sched.is_open_at(ms(2024, 1, 6, 12, 0)));
        // Sun 2024-01-07 12:00 UTC.
        assert!(!sched.is_open_at(ms(2024, 1, 7, 12, 0)));
    }

    #[test]
    fn next_open_after_within_same_day() {
        let sched = business_hours_utc();
        // Wed 2024-01-03 07:30 UTC → next open should be 09:00 same day.
        let now = ms(2024, 1, 3, 7, 30);
        let next = sched.next_open_after(now).unwrap();
        assert_eq!(next, ms(2024, 1, 3, 9, 0));
    }

    #[test]
    fn next_open_after_overnight() {
        let sched = business_hours_utc();
        // Wed 2024-01-03 18:00 UTC (window closed) → next open Thu 09:00.
        let now = ms(2024, 1, 3, 18, 0);
        let next = sched.next_open_after(now).unwrap();
        assert_eq!(next, ms(2024, 1, 4, 9, 0));
    }

    #[test]
    fn next_open_after_skips_weekend() {
        let sched = business_hours_utc();
        // Fri 2024-01-05 18:00 UTC → next open Mon 09:00.
        let now = ms(2024, 1, 5, 18, 0);
        let next = sched.next_open_after(now).unwrap();
        assert_eq!(next, ms(2024, 1, 8, 9, 0));
    }

    #[test]
    fn next_open_after_returns_none_when_no_windows() {
        // All seven days empty → gate permanently closed.
        let sched = ActiveHoursSchedule {
            timezone: None,
            windows: std::array::from_fn(|_| Vec::new()),
        };
        assert!(sched.next_open_after(0).is_none());
    }

    #[test]
    fn validate_rejects_invalid_timezone() {
        let sched = ActiveHoursSchedule {
            timezone: Some("Mars/Olympus_Mons".into()),
            windows: std::array::from_fn(|_| Vec::new()),
        };
        assert!(
            sched.validate().is_err(),
            "validate() must reject unparseable timezone strings"
        );
    }

    #[test]
    fn invalid_timezone_fails_closed_at_runtime() {
        let sched = ActiveHoursSchedule {
            timezone: Some("Mars/Olympus_Mons".into()),
            ..business_hours_utc()
        };
        assert!(!sched.is_open_at(ms(2024, 1, 3, 10, 0)));
        assert!(sched.next_open_after(ms(2024, 1, 3, 10, 0)).is_none());
    }

    #[test]
    fn missing_timezone_still_uses_default_open() {
        let sched = business_hours_utc();
        let same_no_tz = ActiveHoursSchedule {
            timezone: None,
            ..business_hours_utc()
        };
        assert!(same_no_tz.validate().is_ok());
        assert_eq!(
            sched.is_open_at(ms(2024, 1, 3, 10, 0)),
            same_no_tz.is_open_at(ms(2024, 1, 3, 10, 0)),
        );
    }

    #[test]
    fn validate_rejects_inverted_window() {
        let sched = ActiveHoursSchedule {
            timezone: None,
            windows: [
                vec![TimeWindow {
                    start_minute: 600,
                    end_minute: 600,
                }],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
        };
        assert!(sched.validate().is_err());
    }

    #[test]
    fn next_due_after_gate_short_circuits_when_open() {
        let sched = business_hours_utc();
        let now = ms(2024, 1, 3, 10, 0); // Wed 10:00 UTC
        let next_due = now + 60_000; // 10:01 UTC, still in window
        assert_eq!(next_due_after_gate(&sched, next_due, now), Some(next_due));
    }

    #[test]
    fn next_due_after_gate_pushes_to_window_start() {
        let sched = business_hours_utc();
        let now = ms(2024, 1, 3, 18, 0); // Wed evening (closed)
        let next_due = ms(2024, 1, 3, 18, 5); // would-be next due (still closed)
        let result = next_due_after_gate(&sched, next_due, now).unwrap();
        // Should snap forward to Thu 09:00.
        assert_eq!(result, ms(2024, 1, 4, 9, 0));
    }
}
