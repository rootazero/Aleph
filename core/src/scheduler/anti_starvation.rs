//! Anti-starvation logic for lane scheduling
//!
//! Tracks wait times for queued runs and calculates priority boosts to prevent
//! low-priority tasks from being starved indefinitely.

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::agents::sub_agents::Lane;

/// Tracks wait times for queued runs to prevent starvation
///
/// This tracker monitors how long runs have been waiting in queues and
/// calculates priority boosts based on wait time. Runs that wait longer
/// than the threshold receive incremental priority boosts.
pub struct WaitTimeTracker {
    /// Map of run_id -> (lane, enqueued_timestamp_ms)
    enqueued_at: Arc<RwLock<HashMap<String, (Lane, i64)>>>,
}

impl WaitTimeTracker {
    /// Create a new WaitTimeTracker
    pub fn new() -> Self {
        Self {
            enqueued_at: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Track when a run is enqueued
    ///
    /// Records the current timestamp for the run so we can calculate wait time later.
    pub async fn track_enqueue(&self, run_id: &str, lane: Lane, timestamp: i64) {
        let mut map = self.enqueued_at.write().await;
        map.insert(run_id.to_string(), (lane, timestamp));
    }

    /// Calculate priority boost for a run based on wait time
    ///
    /// Formula: boost = min(10, (wait_time - threshold) / 30000)
    /// - Threshold: 30 seconds (30,000 ms)
    /// - Boost: +1 per 30 seconds of waiting
    /// - Maximum boost: +10
    ///
    /// Returns 0 if the run hasn't exceeded the threshold or isn't tracked.
    pub async fn calculate_boost(
        &self,
        run_id: &str,
        current_time: i64,
        threshold_ms: u64,
        boost_per_30s: i8,
    ) -> i8 {
        let map = self.enqueued_at.read().await;

        if let Some((_lane, enqueued_at)) = map.get(run_id) {
            let wait_ms = (current_time - enqueued_at).max(0) as u64;

            if wait_ms > threshold_ms {
                // Calculate boost: +boost_per_30s per 30 seconds over threshold
                let boost = (((wait_ms - threshold_ms) / 30_000).min(127) as i8).saturating_mul(boost_per_30s);
                boost.min(10)
            } else {
                0
            }
        } else {
            0
        }
    }

    /// Remove a run from tracking (called when scheduled or cancelled)
    pub async fn remove(&self, run_id: &str) {
        let mut map = self.enqueued_at.write().await;
        map.remove(run_id);
    }

    /// Get the wait time for a run in milliseconds
    ///
    /// Returns 0 if the run is not tracked.
    pub async fn get_wait_time(&self, run_id: &str, current_time: i64) -> u64 {
        let map = self.enqueued_at.read().await;

        if let Some((_lane, enqueued_at)) = map.get(run_id) {
            (current_time - enqueued_at).max(0) as u64
        } else {
            0
        }
    }

    /// Get all tracked runs with their wait times
    ///
    /// Returns a vector of (run_id, lane, wait_time_ms) tuples.
    pub async fn get_all_wait_times(&self, current_time: i64) -> Vec<(String, Lane, u64)> {
        let map = self.enqueued_at.read().await;

        map.iter()
            .map(|(run_id, (lane, enqueued_at))| {
                let wait_ms = (current_time - enqueued_at).max(0) as u64;
                (run_id.clone(), *lane, wait_ms)
            })
            .collect()
    }
}

impl Default for WaitTimeTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn current_time_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    #[tokio::test]
    async fn test_track_enqueue() {
        let tracker = WaitTimeTracker::new();
        let run_id = "run-1";
        let timestamp = 1000i64;

        tracker.track_enqueue(run_id, Lane::Main, timestamp).await;

        let wait_time = tracker.get_wait_time(run_id, 2000).await;
        assert_eq!(wait_time, 1000);
    }

    #[tokio::test]
    async fn test_calculate_boost_below_threshold() {
        let tracker = WaitTimeTracker::new();
        let run_id = "run-1";
        let enqueued_at = 1000i64;

        tracker.track_enqueue(run_id, Lane::Main, enqueued_at).await;

        // 20 seconds later (below 30s threshold)
        let boost = tracker.calculate_boost(run_id, 21_000, 30_000, 1).await;
        assert_eq!(boost, 0);
    }

    #[tokio::test]
    async fn test_calculate_boost_at_threshold() {
        let tracker = WaitTimeTracker::new();
        let run_id = "run-1";
        let enqueued_at = 1000i64;

        tracker.track_enqueue(run_id, Lane::Main, enqueued_at).await;

        // Exactly 30 seconds later (at threshold)
        let boost = tracker.calculate_boost(run_id, 31_000, 30_000, 1).await;
        assert_eq!(boost, 0); // At threshold, not over
    }

    #[tokio::test]
    async fn test_calculate_boost_above_threshold() {
        let tracker = WaitTimeTracker::new();
        let run_id = "run-1";
        let enqueued_at = 1000i64;

        tracker.track_enqueue(run_id, Lane::Main, enqueued_at).await;

        // 31 seconds later (1 second over threshold)
        let boost = tracker.calculate_boost(run_id, 32_000, 30_000, 1).await;
        assert_eq!(boost, 0); // Only 1 second over, not enough for +1 boost

        // 60 seconds later (30 seconds over threshold)
        let boost = tracker.calculate_boost(run_id, 61_000, 30_000, 1).await;
        assert_eq!(boost, 1); // +1 boost per 30 seconds

        // 90 seconds later (60 seconds over threshold)
        let boost = tracker.calculate_boost(run_id, 91_000, 30_000, 1).await;
        assert_eq!(boost, 2); // +2 boost

        // 330 seconds later (300 seconds over threshold)
        let boost = tracker.calculate_boost(run_id, 331_000, 30_000, 1).await;
        assert_eq!(boost, 10); // Capped at +10
    }

    #[tokio::test]
    async fn test_calculate_boost_not_tracked() {
        let tracker = WaitTimeTracker::new();

        // Run not tracked
        let boost = tracker.calculate_boost("unknown-run", 100_000, 30_000, 1).await;
        assert_eq!(boost, 0);
    }

    #[tokio::test]
    async fn test_remove() {
        let tracker = WaitTimeTracker::new();
        let run_id = "run-1";

        tracker.track_enqueue(run_id, Lane::Main, 1000).await;
        assert_eq!(tracker.get_wait_time(run_id, 2000).await, 1000);

        tracker.remove(run_id).await;
        assert_eq!(tracker.get_wait_time(run_id, 2000).await, 0);
    }

    #[tokio::test]
    async fn test_get_all_wait_times() {
        let tracker = WaitTimeTracker::new();

        tracker.track_enqueue("run-1", Lane::Main, 1000).await;
        tracker.track_enqueue("run-2", Lane::Subagent, 2000).await;
        tracker.track_enqueue("run-3", Lane::Cron, 3000).await;

        let wait_times = tracker.get_all_wait_times(10_000).await;
        assert_eq!(wait_times.len(), 3);

        // Check that all runs are present with correct wait times
        let run1 = wait_times.iter().find(|(id, _, _)| id == "run-1").unwrap();
        assert_eq!(run1.1, Lane::Main);
        assert_eq!(run1.2, 9000);

        let run2 = wait_times.iter().find(|(id, _, _)| id == "run-2").unwrap();
        assert_eq!(run2.1, Lane::Subagent);
        assert_eq!(run2.2, 8000);

        let run3 = wait_times.iter().find(|(id, _, _)| id == "run-3").unwrap();
        assert_eq!(run3.1, Lane::Cron);
        assert_eq!(run3.2, 7000);
    }

    #[tokio::test]
    async fn test_multiple_lanes() {
        let tracker = WaitTimeTracker::new();

        tracker.track_enqueue("main-1", Lane::Main, 1000).await;
        tracker.track_enqueue("sub-1", Lane::Subagent, 1000).await;
        tracker.track_enqueue("cron-1", Lane::Cron, 1000).await;

        // All should have same wait time
        let wait_time_main = tracker.get_wait_time("main-1", 61_000).await;
        let wait_time_sub = tracker.get_wait_time("sub-1", 61_000).await;
        let wait_time_cron = tracker.get_wait_time("cron-1", 61_000).await;

        assert_eq!(wait_time_main, 60_000);
        assert_eq!(wait_time_sub, 60_000);
        assert_eq!(wait_time_cron, 60_000);

        // All should have same boost
        let boost_main = tracker.calculate_boost("main-1", 61_000, 30_000, 1).await;
        let boost_sub = tracker.calculate_boost("sub-1", 61_000, 30_000, 1).await;
        let boost_cron = tracker.calculate_boost("cron-1", 61_000, 30_000, 1).await;

        assert_eq!(boost_main, 1);
        assert_eq!(boost_sub, 1);
        assert_eq!(boost_cron, 1);
    }
}
