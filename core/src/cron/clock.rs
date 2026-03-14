//! Clock abstraction for time-dependent modules.
//!
//! All time-dependent code should receive `&dyn Clock` instead of calling
//! `Utc::now()` directly, enabling deterministic testing via `FakeClock`.

use chrono::{DateTime, TimeZone, Utc};

/// Trait abstracting over system time.
///
/// Implementations must be thread-safe and `'static` so they can be shared
/// across async tasks and stored in long-lived containers.
pub trait Clock: Send + Sync + 'static {
    /// Returns the current time as milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;

    /// Returns the current time as a `DateTime<Utc>`.
    ///
    /// Default implementation converts from [`now_ms`](Clock::now_ms).
    fn now_utc(&self) -> DateTime<Utc> {
        let ms = self.now_ms();
        let secs = ms / 1000;
        let nanos = ((ms % 1000) * 1_000_000) as u32;
        Utc.timestamp_opt(secs, nanos)
            .single()
            .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap())
    }
}

/// Production clock backed by `Utc::now()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
}

/// Test utilities for deterministic time control.
#[cfg(any(test, feature = "test-helpers"))]
pub mod testing {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// A fake clock whose time is controlled explicitly.
    ///
    /// Uses `AtomicI64` for interior mutability so it can be shared across
    /// threads without external synchronization.
    #[derive(Debug)]
    pub struct FakeClock {
        ms: AtomicI64,
    }

    impl FakeClock {
        /// Create a new `FakeClock` pinned at `initial_ms` since epoch.
        pub fn new(initial_ms: i64) -> Self {
            Self {
                ms: AtomicI64::new(initial_ms),
            }
        }

        /// Advance the clock by `delta_ms` milliseconds.
        pub fn advance(&self, delta_ms: i64) {
            self.ms.fetch_add(delta_ms, Ordering::SeqCst);
        }

        /// Set the clock to an absolute value.
        pub fn set(&self, ms: i64) {
            self.ms.store(ms, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> i64 {
            self.ms.load(Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::testing::FakeClock;

    #[test]
    fn system_clock_returns_reasonable_time() {
        let clock = SystemClock;
        // 2025-01-01T00:00:00Z in milliseconds
        let jan_2025_ms = 1_735_689_600_000_i64;
        assert!(
            clock.now_ms() > jan_2025_ms,
            "SystemClock should return a time after 2025-01-01"
        );
    }

    #[test]
    fn fake_clock_initial_value() {
        let clock = FakeClock::new(42_000);
        assert_eq!(clock.now_ms(), 42_000);
    }

    #[test]
    fn fake_clock_advance() {
        let clock = FakeClock::new(1_000);
        clock.advance(500);
        assert_eq!(clock.now_ms(), 1_500);
        clock.advance(200);
        assert_eq!(clock.now_ms(), 1_700);
    }

    #[test]
    fn fake_clock_set() {
        let clock = FakeClock::new(0);
        clock.set(99_999);
        assert_eq!(clock.now_ms(), 99_999);
    }

    #[test]
    fn fake_clock_now_utc() {
        // 2025-06-15T12:00:00.500Z
        let ms = 1_750_003_200_500_i64;
        let clock = FakeClock::new(ms);
        let dt = clock.now_utc();
        assert_eq!(dt.timestamp(), 1_750_003_200);
        assert_eq!(dt.timestamp_subsec_millis(), 500);
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2025-06-15");
    }
}
