//! Idle detector for dreaming trigger.
//!
//! Monitors user activity and determines when the system is idle,
//! enabling background "dreaming" processes like experience clustering
//! and pattern crystallization.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

/// Configuration for idle detection.
#[derive(Debug, Clone)]
pub struct IdleConfig {
    /// Minimum idle duration in seconds before triggering dreaming.
    pub min_idle_seconds: u64,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            min_idle_seconds: 300,
        }
    }
}

/// Detects user idle periods for triggering background dreaming.
///
/// Uses an atomic timestamp to track the last user activity, allowing
/// lock-free concurrent reads and writes from any thread.
#[derive(Clone)]
pub struct IdleDetector {
    /// Last activity timestamp in milliseconds since UNIX epoch.
    last_activity: Arc<AtomicU64>,
    /// Configuration for idle thresholds.
    config: IdleConfig,
}

impl IdleDetector {
    /// Create a new idle detector with the given config.
    ///
    /// Initializes last_activity to the current time.
    pub fn new(config: IdleConfig) -> Self {
        Self {
            last_activity: Arc::new(AtomicU64::new(now_ms())),
            config,
        }
    }

    /// Record user activity, resetting the idle timer.
    pub fn record_activity(&self) {
        self.last_activity.store(now_ms(), Ordering::Relaxed);
    }

    /// Check whether the system is currently idle.
    ///
    /// Returns `true` if the elapsed time since last activity exceeds
    /// `min_idle_seconds`.
    pub fn is_idle(&self) -> bool {
        let last = self.last_activity.load(Ordering::Relaxed);
        let elapsed = now_ms().saturating_sub(last);
        elapsed > self.config.min_idle_seconds * 1000
    }
}

/// Current time in milliseconds since UNIX epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initially_idle() {
        let detector = IdleDetector::new(IdleConfig::default());
        // Force last_activity to 0 (epoch) so elapsed is huge
        detector.last_activity.store(0, Ordering::Relaxed);
        assert!(detector.is_idle());
    }

    #[test]
    fn test_activity_resets_idle() {
        let detector = IdleDetector::new(IdleConfig::default());
        detector.record_activity();
        assert!(!detector.is_idle());
    }

    #[test]
    fn test_idle_after_timeout() {
        let config = IdleConfig {
            min_idle_seconds: 1,
        };
        let detector = IdleDetector::new(config);
        // Set last_activity to 2 seconds ago
        let two_sec_ago = now_ms().saturating_sub(2000);
        detector.last_activity.store(two_sec_ago, Ordering::Relaxed);
        assert!(detector.is_idle());
    }
}
