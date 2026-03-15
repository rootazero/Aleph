//! Inbound message deduplication tracker

use std::collections::HashSet;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Time window for inbound message deduplication (5 minutes)
const DEDUP_WINDOW: Duration = Duration::from_secs(300);

/// Maximum dedup entries before forced cleanup
const DEDUP_MAX_ENTRIES: usize = 10_000;

/// Tracks recently processed inbound message IDs to prevent duplicate execution
pub(super) struct InboundDedupTracker {
    /// Set of "channel_id:message_id" keys
    seen: HashSet<String>,
    /// Ordered list of (key, timestamp) for expiry
    entries: Vec<(String, Instant)>,
}

impl InboundDedupTracker {
    pub(super) fn new() -> Self {
        Self {
            seen: HashSet::new(),
            entries: Vec::new(),
        }
    }

    /// Check if message was already processed. If not, mark it as seen.
    /// Returns true if this is a NEW message (not a duplicate).
    pub(super) fn check_and_record(&mut self, key: &str) -> bool {
        // Expire old entries first
        self.expire();

        if self.seen.contains(key) {
            return false; // Duplicate
        }

        self.seen.insert(key.to_string());
        self.entries.push((key.to_string(), Instant::now()));
        true
    }

    /// Remove entries older than DEDUP_WINDOW
    fn expire(&mut self) {
        let cutoff = Instant::now() - DEDUP_WINDOW;
        let before = self.entries.len();

        self.entries.retain(|(key, ts)| {
            if *ts < cutoff {
                self.seen.remove(key);
                false
            } else {
                true
            }
        });

        if before > self.entries.len() {
            debug!(
                "Dedup tracker: expired {} entries, {} remaining",
                before - self.entries.len(),
                self.entries.len()
            );
        }

        // Safety cap: if somehow we accumulate too many, drop oldest half
        if self.entries.len() > DEDUP_MAX_ENTRIES {
            let drain_count = self.entries.len() / 2;
            for (key, _) in self.entries.drain(..drain_count) {
                self.seen.remove(&key);
            }
            warn!(
                "Dedup tracker hit max entries, forcibly dropped {} entries",
                drain_count
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_tracker_new_message() {
        let mut tracker = InboundDedupTracker::new();
        assert!(tracker.check_and_record("telegram:123"));
        assert_eq!(tracker.seen.len(), 1);
    }

    #[test]
    fn test_dedup_tracker_duplicate_blocked() {
        let mut tracker = InboundDedupTracker::new();
        assert!(tracker.check_and_record("telegram:123"));
        assert!(!tracker.check_and_record("telegram:123")); // duplicate
    }

    #[test]
    fn test_dedup_tracker_different_messages_allowed() {
        let mut tracker = InboundDedupTracker::new();
        assert!(tracker.check_and_record("telegram:123"));
        assert!(tracker.check_and_record("telegram:124"));
        assert!(tracker.check_and_record("discord:123")); // same msg_id, different channel
        assert_eq!(tracker.seen.len(), 3);
    }

    #[test]
    fn test_dedup_tracker_expire() {
        let mut tracker = InboundDedupTracker::new();
        // Insert an entry with a past timestamp
        let old_key = "telegram:old".to_string();
        tracker.seen.insert(old_key.clone());
        tracker.entries.push((old_key, Instant::now() - Duration::from_secs(600)));

        // Insert a fresh entry
        assert!(tracker.check_and_record("telegram:new"));

        // After expire, old entry should be gone
        assert_eq!(tracker.seen.len(), 1); // only "new" remains
        assert_eq!(tracker.entries.len(), 1);
    }
}
