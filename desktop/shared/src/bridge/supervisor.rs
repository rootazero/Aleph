//! Subprocess supervisor primitives — exponential backoff + restart window.
//!
//! The `SwiftBridge` client (bridge/client.rs) drives a single [`SpawnGate`],
//! which composes two lower-level primitives to answer one question — *may we
//! (re)spawn the helper right now?*:
//! - How long to wait before a respawn attempt (`Backoff`).
//! - Whether the helper has become chronically unreliable and further calls
//!   should be short-circuited with `DesktopError::BridgeDisabled`
//!   (`RestartWindow`). The default policy is "5 restarts in 10 minutes".
//!
//! [`SpawnGate`] is the single source of truth for respawn pacing: it holds the
//! "earliest next spawn" instant so the client never has to track it separately.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Exponential backoff ladder: 1s, 2s, 4s, 8s, 16s, then capped at 30s.
#[derive(Debug, Default)]
pub struct Backoff {
    step: u32,
}

impl Backoff {
    /// Returns the next delay and advances the ladder one step.
    pub const fn next_delay(&mut self) -> Duration {
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

    pub const fn reset(&mut self) {
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
    #[must_use]
    pub const fn new(threshold: usize, window: Duration) -> Self {
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

/// Decision returned by [`SpawnGate::poll`] when a respawn is requested.
#[derive(Debug, PartialEq, Eq)]
pub enum SpawnDecision {
    /// Clear to attempt a spawn now.
    Go,
    /// A respawn was attempted too recently — hold off for `remaining`.
    Backoff { remaining: Duration },
}

/// Single source of truth for helper respawn pacing.
///
/// Composes [`Backoff`] (how long to wait between attempts) and
/// [`RestartWindow`] (when to give up entirely) and owns the "earliest next
/// spawn" instant. The client asks [`poll`](Self::poll) before each spawn and
/// reports the outcome via [`record_success`](Self::record_success) /
/// [`record_failure`](Self::record_failure) — it no longer tracks delay state
/// itself, which is what let the old backoff ladder be computed and silently
/// discarded.
#[derive(Debug)]
pub struct SpawnGate {
    backoff: Backoff,
    window: RestartWindow,
    /// Earliest instant a spawn may be attempted. `None` = no active backoff.
    next_spawn_at: Option<Instant>,
}

impl SpawnGate {
    /// Build a gate that disables the helper after `threshold` restarts within
    /// `window`.
    #[must_use]
    pub const fn new(threshold: usize, window: Duration) -> Self {
        Self {
            backoff: Backoff { step: 0 },
            window: RestartWindow::new(threshold, window),
            next_spawn_at: None,
        }
    }

    /// May a spawn proceed now? `Go` clears the gate; `Backoff` means a recent
    /// failure is still cooling down.
    #[must_use]
    pub fn poll(&self) -> SpawnDecision {
        self.next_spawn_at.map_or(SpawnDecision::Go, |at| {
            let now = Instant::now();
            if now >= at {
                SpawnDecision::Go
            } else {
                SpawnDecision::Backoff {
                    remaining: at.saturating_duration_since(now),
                }
            }
        })
    }

    /// Record a spawn failure or helper crash. Advances the backoff ladder,
    /// arms the next-spawn gate, and returns `true` when the restart threshold
    /// has been exceeded (caller should latch the bridge into disabled mode).
    pub fn record_failure(&mut self) -> bool {
        let delay = self.backoff.next_delay();
        self.next_spawn_at = Some(Instant::now() + delay);
        self.window.record_and_should_disable()
    }

    /// Record a successful spawn. Resets the backoff ladder and clears the
    /// gate so an isolated later failure starts cooling from the first rung.
    /// The restart window is intentionally *not* cleared — a flapping helper
    /// (success → crash → success → crash) must still trip the threshold.
    pub fn record_success(&mut self) {
        self.backoff.reset();
        self.next_spawn_at = None;
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
        let mut w = RestartWindow::new(5, Duration::from_mins(10));
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

    #[test]
    fn gate_starts_clear() {
        let gate = SpawnGate::new(5, Duration::from_mins(10));
        assert_eq!(gate.poll(), SpawnDecision::Go);
    }

    #[test]
    fn gate_backs_off_after_failure() {
        let mut gate = SpawnGate::new(5, Duration::from_mins(10));
        assert!(!gate.record_failure());
        // First rung is 1s — a poll immediately after must report Backoff.
        match gate.poll() {
            SpawnDecision::Backoff { remaining } => {
                assert!(remaining <= Duration::from_secs(1) && remaining > Duration::ZERO);
            }
            SpawnDecision::Go => panic!("expected backoff immediately after failure"),
        }
    }

    #[test]
    fn gate_clears_on_success() {
        let mut gate = SpawnGate::new(5, Duration::from_mins(10));
        gate.record_failure();
        gate.record_success();
        assert_eq!(
            gate.poll(),
            SpawnDecision::Go,
            "success must clear the gate"
        );
    }

    #[test]
    fn gate_disables_after_threshold() {
        let mut gate = SpawnGate::new(5, Duration::from_mins(10));
        for _ in 0..5 {
            assert!(!gate.record_failure());
        }
        assert!(gate.record_failure(), "6th failure trips disable");
    }

    #[test]
    fn gate_backoff_window_elapses_to_go() {
        let mut gate = SpawnGate::new(5, Duration::from_mins(10));
        gate.record_failure();
        // Force the gate open by rewinding the armed instant into the past.
        gate.next_spawn_at = Some(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        assert_eq!(gate.poll(), SpawnDecision::Go);
    }
}
