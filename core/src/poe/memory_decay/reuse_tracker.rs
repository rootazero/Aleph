//! Reuse tracking for POE experiences.
//!
//! Records when experiences are reused and whether the reuse
//! led to success, enabling performance-based decay.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Record of an experience being reused in a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReuseRecord {
    /// ID of the experience that was reused
    pub experience_id: String,
    /// When the reuse occurred (Unix timestamp seconds)
    pub reused_at: i64,
    /// Whether the reuse led to task success
    pub led_to_success: bool,
    /// ID of the task where the experience was reused
    pub task_id: String,
}

/// In-memory tracker for experience reuse history.
///
/// Tracks how often each experience is reused and whether
/// those reuses lead to success. This data feeds into the
/// performance factor of memory decay.
pub struct InMemoryReuseTracker {
    records: HashMap<String, Vec<ReuseRecord>>,
}

impl InMemoryReuseTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

    /// Record a reuse event.
    pub fn record_reuse(&mut self, record: ReuseRecord) {
        self.records
            .entry(record.experience_id.clone())
            .or_default()
            .push(record);
    }

    /// Get recent reuse records for an experience (newest first).
    ///
    /// Returns up to `limit` records, sorted by `reused_at` descending.
    pub fn get_recent(&self, experience_id: &str, limit: usize) -> Vec<&ReuseRecord> {
        let Some(records) = self.records.get(experience_id) else {
            return Vec::new();
        };
        let mut sorted: Vec<&ReuseRecord> = records.iter().collect();
        sorted.sort_by(|a, b| b.reused_at.cmp(&a.reused_at));
        sorted.truncate(limit);
        sorted
    }

    /// Calculate success rate over the last `window` reuses.
    ///
    /// Returns 1.0 if no reuse history (benefit of the doubt).
    pub fn success_rate(&self, experience_id: &str, window: usize) -> f32 {
        let recent = self.get_recent(experience_id, window);
        if recent.is_empty() {
            return 1.0;
        }
        let successes = recent.iter().filter(|r| r.led_to_success).count();
        successes as f32 / recent.len() as f32
    }

    /// Total reuse count for an experience.
    pub fn reuse_count(&self, experience_id: &str) -> usize {
        self.records
            .get(experience_id)
            .map_or(0, |records| records.len())
    }
}

impl Default for InMemoryReuseTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(exp_id: &str, task_id: &str, success: bool, timestamp: i64) -> ReuseRecord {
        ReuseRecord {
            experience_id: exp_id.into(),
            reused_at: timestamp,
            led_to_success: success,
            task_id: task_id.into(),
        }
    }

    #[test]
    fn test_record_and_retrieve() {
        let mut tracker = InMemoryReuseTracker::new();
        tracker.record_reuse(make_record("exp-1", "task-1", true, 1000));
        tracker.record_reuse(make_record("exp-1", "task-2", false, 2000));

        let recent = tracker.get_recent("exp-1", 10);
        assert_eq!(recent.len(), 2);
        // Newest first
        assert_eq!(recent[0].reused_at, 2000);
        assert_eq!(recent[1].reused_at, 1000);
    }

    #[test]
    fn test_success_rate_mixed() {
        let mut tracker = InMemoryReuseTracker::new();
        tracker.record_reuse(make_record("exp-1", "t1", true, 1000));
        tracker.record_reuse(make_record("exp-1", "t2", false, 2000));
        tracker.record_reuse(make_record("exp-1", "t3", true, 3000));
        tracker.record_reuse(make_record("exp-1", "t4", true, 4000));

        let rate = tracker.success_rate("exp-1", 10);
        assert!((rate - 0.75).abs() < 0.01);
    }

    #[test]
    fn test_success_rate_window_limiting() {
        let mut tracker = InMemoryReuseTracker::new();
        // Old failures
        tracker.record_reuse(make_record("exp-1", "t1", false, 1000));
        tracker.record_reuse(make_record("exp-1", "t2", false, 2000));
        // Recent successes
        tracker.record_reuse(make_record("exp-1", "t3", true, 3000));
        tracker.record_reuse(make_record("exp-1", "t4", true, 4000));

        // Window of 2 should only see recent successes
        let rate = tracker.success_rate("exp-1", 2);
        assert!((rate - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_empty_history_returns_one() {
        let tracker = InMemoryReuseTracker::new();
        let rate = tracker.success_rate("nonexistent", 5);
        assert!((rate - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_multiple_experiences_independent() {
        let mut tracker = InMemoryReuseTracker::new();
        tracker.record_reuse(make_record("exp-1", "t1", true, 1000));
        tracker.record_reuse(make_record("exp-2", "t2", false, 2000));

        assert_eq!(tracker.reuse_count("exp-1"), 1);
        assert_eq!(tracker.reuse_count("exp-2"), 1);
        assert!((tracker.success_rate("exp-1", 5) - 1.0).abs() < f32::EPSILON);
        assert!(tracker.success_rate("exp-2", 5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_reuse_count() {
        let mut tracker = InMemoryReuseTracker::new();
        assert_eq!(tracker.reuse_count("exp-1"), 0);

        tracker.record_reuse(make_record("exp-1", "t1", true, 1000));
        tracker.record_reuse(make_record("exp-1", "t2", true, 2000));
        tracker.record_reuse(make_record("exp-1", "t3", false, 3000));

        assert_eq!(tracker.reuse_count("exp-1"), 3);
    }

    #[test]
    fn test_get_recent_empty() {
        let tracker = InMemoryReuseTracker::new();
        let recent = tracker.get_recent("nonexistent", 5);
        assert!(recent.is_empty());
    }
}
