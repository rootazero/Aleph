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

/// A helper that spawns cleanly and then survives at least this long is treated
/// as a *stable* session: a later crash is an isolated incident, so the backoff
/// ladder resets and the first respawn is fast. A helper that dies sooner is a
/// crash loop, so the ladder keeps climbing (1s→2s→…→30s) to space respawns out.
const STABLE_UPTIME: Duration = Duration::from_secs(10);

/// Single source of truth for helper respawn pacing.
///
/// Composes [`Backoff`] (how long to wait between attempts) and
/// [`RestartWindow`] (when to give up entirely) and owns the "earliest next
/// spawn" instant. The client asks [`poll`](Self::poll) before each spawn and
/// reports the outcome via [`record_spawn`](Self::record_spawn) /
/// [`record_failure`](Self::record_failure) — it no longer tracks delay state
/// itself, which is what let the old backoff ladder be computed and silently
/// discarded.
///
/// The backoff ladder resets on a *stable* session, not on a bare fork: a
/// forked helper that exits milliseconds later would otherwise reset the ladder
/// on every attempt, so a crash loop paced itself at a flat 1s forever instead
/// of climbing. Liveness is proxied by uptime ([`STABLE_UPTIME`]) rather than a
/// handshake ping, keeping the gate self-contained.
#[derive(Debug)]
pub struct SpawnGate {
    backoff: Backoff,
    window: RestartWindow,
    /// Earliest instant a spawn may be attempted. `None` = no active backoff.
    next_spawn_at: Option<Instant>,
    /// When the current helper was spawned. `None` = none running (never
    /// spawned, or the last one already crashed). Used to distinguish a stable
    /// session's death from a crash loop.
    spawned_at: Option<Instant>,
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
            spawned_at: None,
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
    ///
    /// If the helper that just died had been alive past [`STABLE_UPTIME`], the
    /// ladder resets first so this isolated death cools from the first rung; a
    /// helper that died sooner (or never spawned — a bare spawn failure) leaves
    /// the ladder climbing so a crash loop actually spaces out.
    pub fn record_failure(&mut self) -> bool {
        let was_stable = self
            .spawned_at
            .take()
            .is_some_and(|at| at.elapsed() >= STABLE_UPTIME);
        if was_stable {
            self.backoff.reset();
        }
        let delay = self.backoff.next_delay();
        self.next_spawn_at = Some(Instant::now() + delay);
        self.window.record_and_should_disable()
    }

    /// Record that a helper process was just spawned (fork + stdio takeover
    /// succeeded). Clears the next-spawn gate — a spawn was allowed to proceed —
    /// and stamps the spawn instant, but does **not** reset the backoff ladder:
    /// a bare fork is not proof of liveness, so the ladder only resets once the
    /// helper survives past [`STABLE_UPTIME`] (see [`record_failure`]). The
    /// restart window is intentionally never cleared here — a flapping helper
    /// (spawn → crash → spawn → crash) must still trip the threshold.
    pub fn record_spawn(&mut self) {
        self.next_spawn_at = None;
        self.spawned_at = Some(Instant::now());
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
    fn gate_clears_on_spawn() {
        let mut gate = SpawnGate::new(5, Duration::from_mins(10));
        gate.record_failure();
        gate.record_spawn();
        assert_eq!(
            gate.poll(),
            SpawnDecision::Go,
            "a spawn must clear the gate"
        );
    }

    #[test]
    fn rapid_crash_loop_climbs_the_ladder() {
        // Regression: a helper that forks OK then dies immediately used to reset
        // the ladder on every spawn, pacing the loop at a flat 1s forever. A
        // sub-STABLE_UPTIME death must instead keep the ladder climbing.
        let mut gate = SpawnGate::new(100, Duration::from_mins(10));
        let expected = [1u64, 2, 4, 8, 16, 30];
        for want in expected {
            gate.record_spawn(); // fork OK, but the helper dies right away…
            gate.record_failure(); // …well under STABLE_UPTIME → do not reset.
            match gate.poll() {
                SpawnDecision::Backoff { remaining } => {
                    assert!(
                        remaining <= Duration::from_secs(want) && remaining > Duration::ZERO,
                        "expected ≤{want}s backoff, got {remaining:?}"
                    );
                }
                SpawnDecision::Go => panic!("expected backoff after a rapid crash"),
            }
        }
    }

    #[test]
    fn stable_session_death_resets_the_ladder() {
        let mut gate = SpawnGate::new(100, Duration::from_mins(10));
        // Climb the ladder a few rungs via rapid crashes.
        for _ in 0..3 {
            gate.record_spawn();
            gate.record_failure();
        }
        // Now simulate a helper that spawned and stayed alive past STABLE_UPTIME.
        gate.record_spawn();
        gate.spawned_at = Some(
            Instant::now()
                .checked_sub(STABLE_UPTIME + Duration::from_secs(1))
                .unwrap(),
        );
        gate.record_failure();
        // The ladder reset, so the first respawn cools from the 1s rung again.
        match gate.poll() {
            SpawnDecision::Backoff { remaining } => {
                assert!(remaining <= Duration::from_secs(1) && remaining > Duration::ZERO);
            }
            SpawnDecision::Go => panic!("expected 1s backoff after stable death"),
        }
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
