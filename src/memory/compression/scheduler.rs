//! Compression Scheduler
//!
//! Tracks accumulated conversation turns so `CompressionService` can trigger
//! compression when the turn threshold is crossed.
//!
//! ## Why turn-threshold is the only trigger
//!
//! Earlier revisions carried `IdleTimeout` / `SessionEnd` / `ManualRequest` /
//! `BackgroundSchedule` trigger variants plus an idle-timer. None of them had
//! a live production path: the manual/session-end/background flows all call
//! `CompressionService::compress()` directly (bypassing the scheduler), and
//! the idle branch was unreachable — `should_trigger_compression` is only
//! invoked from the turn-threshold spawn, at which point the turn check wins
//! first. The idle timer itself was reset by `compress()`, so it measured
//! "time since last compression", not user activity. All of it was removed
//! per YAGNI; this module is now just the turn counter.

use crate::config::CompressionPolicy;
use crate::sync_primitives::{AtomicU32, Ordering};

/// Compression trigger conditions
#[derive(Debug, Clone)]
pub enum CompressionTrigger {
    /// No trigger condition met
    None,
    /// Accumulated turns exceed threshold
    TurnThreshold(u32),
}

/// Configuration for compression scheduling
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Turn threshold for triggering compression (default: 20)
    pub turn_threshold: u32,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self { turn_threshold: 20 }
    }
}

impl SchedulerConfig {
    /// Create a `SchedulerConfig` from policy configuration
    #[must_use]
    pub const fn from_policy(policy: &CompressionPolicy) -> Self {
        Self {
            turn_threshold: policy.turn_threshold,
        }
    }
}

/// Scheduler for determining when to trigger compression
pub struct CompressionScheduler {
    config: SchedulerConfig,
    pub(crate) pending_turns: AtomicU32,
}

impl CompressionScheduler {
    /// Create a new compression scheduler
    #[must_use]
    pub const fn new(config: SchedulerConfig) -> Self {
        Self {
            config,
            pending_turns: AtomicU32::new(0),
        }
    }

    /// Check if compression should be triggered
    pub fn should_trigger_compression(&self) -> CompressionTrigger {
        let turns = self.pending_turns.load(Ordering::Acquire);
        if turns >= self.config.turn_threshold {
            return CompressionTrigger::TurnThreshold(turns);
        }
        CompressionTrigger::None
    }

    /// Get current pending turns count
    pub fn get_pending_turns(&self) -> u32 {
        self.pending_turns.load(Ordering::Acquire)
    }

    /// Reset turns counter (after compression completes)
    pub fn reset_turns(&self) {
        self.pending_turns.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_creation() {
        let scheduler = CompressionScheduler::new(SchedulerConfig::default());
        assert_eq!(scheduler.get_pending_turns(), 0);
    }

    #[test]
    fn test_turn_threshold_trigger() {
        let config = SchedulerConfig { turn_threshold: 5 };
        let scheduler = CompressionScheduler::new(config);

        // Add 5 turns
        for _ in 0..5 {
            scheduler.pending_turns.fetch_add(1, Ordering::Release);
        }

        match scheduler.should_trigger_compression() {
            CompressionTrigger::TurnThreshold(turns) => assert_eq!(turns, 5),
            CompressionTrigger::None => panic!("Expected TurnThreshold trigger"),
        }
    }

    #[test]
    fn test_no_trigger_below_threshold() {
        let config = SchedulerConfig {
            turn_threshold: 100,
        };
        let scheduler = CompressionScheduler::new(config);
        scheduler.pending_turns.fetch_add(1, Ordering::Release);

        match scheduler.should_trigger_compression() {
            CompressionTrigger::None => {}
            CompressionTrigger::TurnThreshold(_) => {
                panic!("Expected None trigger below the threshold")
            }
        }
    }

    #[test]
    fn test_reset_turns() {
        let scheduler = CompressionScheduler::new(SchedulerConfig::default());

        for _ in 0..10 {
            scheduler.pending_turns.fetch_add(1, Ordering::Release);
        }
        assert_eq!(scheduler.get_pending_turns(), 10);

        scheduler.reset_turns();
        assert_eq!(scheduler.get_pending_turns(), 0);
    }
}
