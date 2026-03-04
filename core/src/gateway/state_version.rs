//! Monotonic version tracker for gateway state domains.
//!
//! Each state domain (presence, health, config) has an independent version counter.
//! Clients can compare their last-seen version with the current version to skip
//! redundant event processing.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks monotonically increasing version numbers for distinct state domains.
///
/// Each domain counter is independent — bumping `presence` does not affect
/// `health` or `config`. All operations use `SeqCst` ordering for simplicity
/// and cross-domain consistency.
pub struct StateVersionTracker {
    presence: AtomicU64,
    health: AtomicU64,
    config: AtomicU64,
}

/// A point-in-time snapshot of all domain versions.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct StateVersion {
    pub presence: u64,
    pub health: u64,
    pub config: u64,
}

impl StateVersionTracker {
    /// Create a new tracker with all versions starting at 0.
    pub fn new() -> Self {
        Self {
            presence: AtomicU64::new(0),
            health: AtomicU64::new(0),
            config: AtomicU64::new(0),
        }
    }

    /// Bump the presence version and return the new value.
    pub fn bump_presence(&self) -> u64 {
        self.presence.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Bump the health version and return the new value.
    pub fn bump_health(&self) -> u64 {
        self.health.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Bump the config version and return the new value.
    pub fn bump_config(&self) -> u64 {
        self.config.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Take a consistent snapshot of all domain versions.
    pub fn snapshot(&self) -> StateVersion {
        StateVersion {
            presence: self.presence.load(Ordering::SeqCst),
            health: self.health.load(Ordering::SeqCst),
            config: self.config.load(Ordering::SeqCst),
        }
    }
}

impl Default for StateVersionTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_versions_are_zero() {
        let tracker = StateVersionTracker::new();
        let snap = tracker.snapshot();
        assert_eq!(snap.presence, 0);
        assert_eq!(snap.health, 0);
        assert_eq!(snap.config, 0);
    }

    #[test]
    fn test_bump_increments_version() {
        let tracker = StateVersionTracker::new();

        assert_eq!(tracker.bump_presence(), 1);
        assert_eq!(tracker.bump_presence(), 2);

        assert_eq!(tracker.bump_health(), 1);
        assert_eq!(tracker.bump_health(), 2);

        assert_eq!(tracker.bump_config(), 1);
        assert_eq!(tracker.bump_config(), 2);
    }

    #[test]
    fn test_independent_version_domains() {
        let tracker = StateVersionTracker::new();

        tracker.bump_presence();
        tracker.bump_presence();
        tracker.bump_presence(); // presence = 3

        tracker.bump_health(); // health = 1

        // config untouched = 0

        let snap = tracker.snapshot();
        assert_eq!(snap.presence, 3);
        assert_eq!(snap.health, 1);
        assert_eq!(snap.config, 0);
    }
}
