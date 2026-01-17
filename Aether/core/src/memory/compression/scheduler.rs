//! Compression Scheduler
//!
//! Determines when to trigger memory compression based on:
//! - User idle timeout
//! - Accumulated conversation turns
//! - Background schedule interval

use crate::config::CompressionPolicy;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Compression trigger conditions
#[derive(Debug, Clone)]
pub enum CompressionTrigger {
    /// No trigger condition met
    None,
    /// User has been idle for specified duration
    IdleTimeout(Duration),
    /// Accumulated turns exceed threshold
    TurnThreshold(u32),
    /// Session has ended
    SessionEnd,
    /// Manual request from user
    ManualRequest,
    /// Background schedule interval reached
    BackgroundSchedule,
}

/// Configuration for compression scheduling
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Idle timeout in seconds (default: 300 = 5 minutes)
    pub idle_timeout_seconds: u32,
    /// Turn threshold for triggering compression (default: 20)
    pub turn_threshold: u32,
    /// Background interval in seconds (default: 3600 = 1 hour)
    pub background_interval_seconds: u32,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: 300,
            turn_threshold: 20,
            background_interval_seconds: 3600,
        }
    }
}

impl SchedulerConfig {
    /// Create a SchedulerConfig from policy configuration
    pub fn from_policy(policy: &CompressionPolicy) -> Self {
        Self {
            idle_timeout_seconds: policy.idle_timeout_seconds,
            turn_threshold: policy.turn_threshold,
            background_interval_seconds: policy.background_interval_seconds,
        }
    }
}

/// Scheduler for determining when to trigger compression
pub struct CompressionScheduler {
    config: SchedulerConfig,
    last_activity: Mutex<Instant>,
    pending_turns: AtomicU32,
}

impl CompressionScheduler {
    /// Create a new compression scheduler
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            config,
            last_activity: Mutex::new(Instant::now()),
            pending_turns: AtomicU32::new(0),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(SchedulerConfig::default())
    }

    /// Check if compression should be triggered
    ///
    /// Priority: TurnThreshold > IdleTimeout
    pub fn should_trigger_compression(&self) -> CompressionTrigger {
        let turns = self.pending_turns.load(Ordering::Relaxed);

        // Check turn threshold first (higher priority)
        if turns >= self.config.turn_threshold {
            return CompressionTrigger::TurnThreshold(turns);
        }

        // Check idle timeout
        let idle_duration = self.get_idle_duration();
        let idle_threshold = Duration::from_secs(self.config.idle_timeout_seconds as u64);

        if idle_duration >= idle_threshold && turns > 0 {
            return CompressionTrigger::IdleTimeout(idle_duration);
        }

        CompressionTrigger::None
    }

    /// Get the current idle duration
    pub fn get_idle_duration(&self) -> Duration {
        self.last_activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    /// Record new activity (resets idle timer)
    pub fn record_activity(&self) {
        let mut last = self.last_activity.lock().unwrap_or_else(|e| e.into_inner());
        *last = Instant::now();
    }

    /// Increment pending turns counter
    pub fn increment_turns(&self) {
        self.pending_turns.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment turns by specified amount
    pub fn increment_turns_by(&self, count: u32) {
        self.pending_turns.fetch_add(count, Ordering::Relaxed);
    }

    /// Get current pending turns count
    pub fn get_pending_turns(&self) -> u32 {
        self.pending_turns.load(Ordering::Relaxed)
    }

    /// Reset turns counter (after compression completes)
    pub fn reset_turns(&self) {
        self.pending_turns.store(0, Ordering::Relaxed);
    }

    /// Update scheduler configuration
    pub fn update_config(&mut self, config: SchedulerConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn get_config(&self) -> &SchedulerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_scheduler_creation() {
        let scheduler = CompressionScheduler::with_defaults();
        assert_eq!(scheduler.get_pending_turns(), 0);
    }

    #[test]
    fn test_turn_threshold_trigger() {
        let config = SchedulerConfig {
            turn_threshold: 5,
            ..Default::default()
        };
        let scheduler = CompressionScheduler::new(config);

        // Add 5 turns
        for _ in 0..5 {
            scheduler.increment_turns();
        }

        match scheduler.should_trigger_compression() {
            CompressionTrigger::TurnThreshold(turns) => assert_eq!(turns, 5),
            _ => panic!("Expected TurnThreshold trigger"),
        }
    }

    #[test]
    fn test_idle_timeout_trigger() {
        let config = SchedulerConfig {
            idle_timeout_seconds: 0, // Immediate timeout for testing
            turn_threshold: 100,
            ..Default::default()
        };
        let scheduler = CompressionScheduler::new(config);

        // Add some turns
        scheduler.increment_turns();

        // Wait a tiny bit to ensure we're past the 0-second timeout
        thread::sleep(Duration::from_millis(10));

        match scheduler.should_trigger_compression() {
            CompressionTrigger::IdleTimeout(_) => {}
            _ => panic!("Expected IdleTimeout trigger"),
        }
    }

    #[test]
    fn test_no_trigger_when_no_turns() {
        let config = SchedulerConfig {
            idle_timeout_seconds: 0,
            turn_threshold: 100,
            ..Default::default()
        };
        let scheduler = CompressionScheduler::new(config);

        // No turns added, even with 0-second timeout
        thread::sleep(Duration::from_millis(10));

        match scheduler.should_trigger_compression() {
            CompressionTrigger::None => {}
            _ => panic!("Expected None trigger when no pending turns"),
        }
    }

    #[test]
    fn test_record_activity_resets_idle() {
        let scheduler = CompressionScheduler::with_defaults();

        thread::sleep(Duration::from_millis(50));
        let idle_before = scheduler.get_idle_duration();

        scheduler.record_activity();
        let idle_after = scheduler.get_idle_duration();

        assert!(idle_after < idle_before);
    }

    #[test]
    fn test_reset_turns() {
        let scheduler = CompressionScheduler::with_defaults();

        scheduler.increment_turns_by(10);
        assert_eq!(scheduler.get_pending_turns(), 10);

        scheduler.reset_turns();
        assert_eq!(scheduler.get_pending_turns(), 0);
    }
}
