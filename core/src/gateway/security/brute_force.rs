//! Brute-force detection for pairing attempts.
//!
//! Tracks failed pairing attempts per (channel, sender) and temporarily
//! blocks senders who exceed the threshold.

use dashmap::DashMap;
use std::time::{Duration, Instant};

/// Default: 5 failures in 5 minutes triggers a 30-minute block.
const MAX_FAILURES: u32 = 5;
const WINDOW: Duration = Duration::from_secs(300);
const BLOCK_DURATION: Duration = Duration::from_secs(1800);

struct AttemptRecord {
    failures: u32,
    first_failure: Instant,
    blocked_until: Option<Instant>,
}

/// Brute-force detector for pairing attempts.
pub struct BruteForceDetector {
    /// Key: "channel:sender_id"
    records: DashMap<String, AttemptRecord>,
}

impl BruteForceDetector {
    pub fn new() -> Self {
        Self {
            records: DashMap::new(),
        }
    }

    /// Check if a sender is currently blocked.
    pub fn is_blocked(&self, channel: &str, sender: &str) -> bool {
        let key = format!("{}:{}", channel, sender);
        if let Some(record) = self.records.get(&key) {
            if let Some(blocked_until) = record.blocked_until {
                if Instant::now() < blocked_until {
                    return true;
                }
            }
        }
        false
    }

    /// Record a failed pairing attempt. Returns true if the sender is now blocked.
    pub fn record_failure(&self, channel: &str, sender: &str) -> bool {
        let key = format!("{}:{}", channel, sender);
        let mut entry = self.records.entry(key).or_insert_with(|| AttemptRecord {
            failures: 0,
            first_failure: Instant::now(),
            blocked_until: None,
        });

        let record = entry.value_mut();

        // Reset window if expired
        if record.first_failure.elapsed() > WINDOW {
            record.failures = 0;
            record.first_failure = Instant::now();
            record.blocked_until = None;
        }

        record.failures += 1;

        if record.failures >= MAX_FAILURES {
            record.blocked_until = Some(Instant::now() + BLOCK_DURATION);
            return true; // Now blocked
        }

        false
    }

    /// Record a successful pairing (resets the failure counter).
    pub fn record_success(&self, channel: &str, sender: &str) {
        let key = format!("{}:{}", channel, sender);
        self.records.remove(&key);
    }

    /// Prune expired records. Returns count pruned.
    pub fn prune(&self) -> usize {
        let mut pruned = 0;
        self.records.retain(|_, record| {
            // Keep if actively blocked or has recent failures
            if let Some(blocked_until) = record.blocked_until {
                if Instant::now() >= blocked_until {
                    pruned += 1;
                    return false;
                }
                return true;
            }
            if record.first_failure.elapsed() > WINDOW {
                pruned += 1;
                return false;
            }
            true
        });
        pruned
    }
}

impl Default for BruteForceDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_blocked_initially() {
        let detector = BruteForceDetector::new();
        assert!(!detector.is_blocked("telegram", "user1"));
    }

    #[test]
    fn test_block_after_threshold() {
        let detector = BruteForceDetector::new();
        for i in 0..4 {
            let blocked = detector.record_failure("telegram", "user1");
            assert!(!blocked, "Should not be blocked after {} failures", i + 1);
        }
        // 5th failure triggers block
        let blocked = detector.record_failure("telegram", "user1");
        assert!(blocked, "Should be blocked after 5 failures");
        assert!(detector.is_blocked("telegram", "user1"));
    }

    #[test]
    fn test_success_resets() {
        let detector = BruteForceDetector::new();
        for _ in 0..4 {
            detector.record_failure("telegram", "user1");
        }
        detector.record_success("telegram", "user1");
        assert!(!detector.is_blocked("telegram", "user1"));

        // Should take 5 more failures to block again
        for _ in 0..4 {
            assert!(!detector.record_failure("telegram", "user1"));
        }
        assert!(detector.record_failure("telegram", "user1"));
    }

    #[test]
    fn test_different_senders_independent() {
        let detector = BruteForceDetector::new();
        for _ in 0..5 {
            detector.record_failure("telegram", "user1");
        }
        assert!(detector.is_blocked("telegram", "user1"));
        assert!(!detector.is_blocked("telegram", "user2"));
    }

    #[test]
    fn test_prune_fresh_records_not_removed() {
        let detector = BruteForceDetector::new();
        // Insert a fresh record
        detector.record_failure("telegram", "user1");
        let pruned = detector.prune();
        assert_eq!(pruned, 0); // Fresh record, not yet expired
    }
}
