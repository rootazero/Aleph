//! Subprocess supervisor primitives — exponential backoff + restart window.
//!
//! The `SwiftBridge` client (bridge/client.rs) uses these to decide:
//! - How long to wait before a respawn attempt (`Backoff`).
//! - Whether the helper has become chronically unreliable and further calls
//!   should be short-circuited with `DesktopError::BridgeDisabled`
//!   (`RestartWindow`). The default policy is "5 restarts in 10 minutes".

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Exponential backoff ladder: 1s, 2s, 4s, 8s, 16s, then capped at 30s.
#[derive(Debug, Default)]
pub struct Backoff {
    step: u32,
}

impl Backoff {
    /// Returns the next delay and advances the ladder one step.
    pub fn next_delay(&mut self) -> Duration {
        let secs: u64 = match self.step {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 8,
            4 => 16,
            _ => 30,
        };
        self.step = self.step.saturating_add(1);
        Duration::from_secs(secs)
    }

    pub fn reset(&mut self) {
        self.step = 0;
    }
}

/// Sliding-window restart counter used to trip the "bridge disabled" latch.
#[derive(Debug)]
pub struct RestartWindow {
    threshold: usize,
    window: Duration,
    events: VecDeque<Instant>,
}

impl RestartWindow {
    pub fn new(threshold: usize, window: Duration) -> Self {
        Self {
            threshold,
            window,
            events: VecDeque::new(),
        }
    }

    /// Record a restart; return `true` when the count exceeds `threshold`
    /// within `window` (caller should flip the helper into disabled mode).
    pub fn record_and_should_disable(&mut self) -> bool {
        let now = Instant::now();
        self.events.push_back(now);
        while let Some(&front) = self.events.front() {
            if now.duration_since(front) > self.window {
                self.events.pop_front();
            } else {
                break;
            }
        }
        self.events.len() > self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_progresses() {
        let mut s = Backoff::default();
        assert_eq!(s.next_delay(), Duration::from_secs(1));
        assert_eq!(s.next_delay(), Duration::from_secs(2));
        assert_eq!(s.next_delay(), Duration::from_secs(4));
        assert_eq!(s.next_delay(), Duration::from_secs(8));
        assert_eq!(s.next_delay(), Duration::from_secs(16));
        assert_eq!(s.next_delay(), Duration::from_secs(30));
        assert_eq!(s.next_delay(), Duration::from_secs(30));
    }

    #[test]
    fn backoff_reset_returns_to_one_second() {
        let mut s = Backoff::default();
        s.next_delay();
        s.next_delay();
        s.reset();
        assert_eq!(s.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn disable_threshold_trips_after_5_within_10min() {
        let mut w = RestartWindow::new(5, Duration::from_secs(600));
        for _ in 0..5 {
            assert!(!w.record_and_should_disable());
        }
        assert!(w.record_and_should_disable()); // 6th trips
    }

    #[test]
    fn disable_threshold_drops_aged_events() {
        let mut w = RestartWindow::new(5, Duration::from_millis(50));
        for _ in 0..5 {
            assert!(!w.record_and_should_disable());
        }
        std::thread::sleep(Duration::from_millis(80));
        assert!(
            !w.record_and_should_disable(),
            "aged events should have been evicted"
        );
    }
}
