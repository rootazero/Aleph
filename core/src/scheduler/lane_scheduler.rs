//! LaneScheduler - Priority-based multi-lane scheduling engine
//!
//! Coordinates scheduling across multiple lanes with:
//! - Priority-based scheduling (Main > Nested > Subagent > Cron)
//! - Global concurrency limits
//! - Per-lane concurrency limits
//! - Statistics tracking

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

use crate::agents::sub_agents::Lane;
use super::{LaneConfig, LaneState};

/// Main scheduling engine for multi-lane coordination
pub struct LaneScheduler {
    /// Per-lane state (queue + semaphore)
    lanes: HashMap<Lane, Arc<LaneState>>,
    /// Global concurrency semaphore
    global_semaphore: Arc<Semaphore>,
    /// Configuration
    config: LaneConfig,
}

impl LaneScheduler {
    /// Create a new LaneScheduler with the given configuration
    pub fn new(config: LaneConfig) -> Self {
        let mut lanes = HashMap::new();

        // Initialize each lane with its quota
        for (lane, quota) in &config.quotas {
            lanes.insert(*lane, Arc::new(LaneState::new(quota.max_concurrent)));
        }

        let global_semaphore = Arc::new(Semaphore::new(config.global_max_concurrent));

        Self {
            lanes,
            global_semaphore,
            config,
        }
    }

    /// Enqueue a run to a specific lane
    pub async fn enqueue(&self, run_id: String, lane: Lane) {
        if let Some(state) = self.lanes.get(&lane) {
            state.enqueue(run_id).await;
        }
    }

    /// Try to schedule the next run from any lane
    ///
    /// Returns the run_id and lane if a run was scheduled, None otherwise.
    /// This method:
    /// 1. Checks global capacity
    /// 2. Iterates lanes by priority (highest first)
    /// 3. For each lane, checks lane capacity
    /// 4. Dequeues a run if both have capacity
    pub async fn try_schedule_next(&self) -> Option<(String, Lane)> {
        // Check if we have global capacity
        if self.global_semaphore.available_permits() == 0 {
            return None;
        }

        // Sort lanes by priority (highest first)
        let mut lanes_by_priority: Vec<_> = self.config.quotas.iter()
            .map(|(lane, quota)| (*lane, quota.priority))
            .collect();
        lanes_by_priority.sort_by(|a, b| b.1.cmp(&a.1));

        // Try each lane in priority order
        for (lane, _priority) in lanes_by_priority {
            if let Some(state) = self.lanes.get(&lane) {
                // Check if lane has capacity
                if state.available_permits() > 0 {
                    // Try to dequeue a run
                    if let Some(run_id) = state.try_dequeue().await {
                        // Acquire permits (these will be held until on_run_complete)
                        let _global_permit = self.global_semaphore.try_acquire().ok()?;
                        let _lane_permit = state.try_acquire_permit()?;

                        // Mark as running and forget the permits (they'll be released on complete)
                        state.mark_running(run_id.clone()).await;
                        std::mem::forget(_global_permit);
                        std::mem::forget(_lane_permit);

                        return Some((run_id, lane));
                    }
                }
            }
        }

        None
    }

    /// Mark a run as completed (releases permits)
    pub async fn on_run_complete(&self, run_id: &str, lane: Lane) {
        if let Some(state) = self.lanes.get(&lane) {
            state.complete(run_id).await;
            // Release permits by adding them back
            self.global_semaphore.add_permits(1);
            state.semaphore().add_permits(1);
        }
    }

    /// Get scheduler statistics
    pub async fn stats(&self) -> SchedulerStats {
        let mut stats = SchedulerStats::default();

        for (lane, state) in &self.lanes {
            let lane_stats = LaneStats {
                queued: state.queue_len().await,
                running: state.running_count().await,
                available_permits: state.available_permits(),
            };

            stats.lanes.insert(*lane, lane_stats);
            stats.total_queued += lane_stats.queued;
            stats.total_running += lane_stats.running;
        }

        stats.global_available_permits = self.global_semaphore.available_permits();

        stats
    }
}

/// Statistics for the scheduler
#[derive(Debug, Clone, Default)]
pub struct SchedulerStats {
    /// Total queued runs across all lanes
    pub total_queued: usize,
    /// Total running runs across all lanes
    pub total_running: usize,
    /// Available global permits
    pub global_available_permits: usize,
    /// Per-lane statistics
    pub lanes: HashMap<Lane, LaneStats>,
}

/// Statistics for a single lane
#[derive(Debug, Clone, Copy)]
pub struct LaneStats {
    /// Number of queued runs
    pub queued: usize,
    /// Number of running runs
    pub running: usize,
    /// Available permits
    pub available_permits: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scheduler_enqueue() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        scheduler.enqueue("run-1".to_string(), Lane::Main).await;
        scheduler.enqueue("run-2".to_string(), Lane::Subagent).await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 2);
        assert_eq!(stats.lanes.get(&Lane::Main).unwrap().queued, 1);
        assert_eq!(stats.lanes.get(&Lane::Subagent).unwrap().queued, 1);
    }

    #[tokio::test]
    async fn test_scheduler_try_schedule() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        scheduler.enqueue("run-1".to_string(), Lane::Main).await;

        let scheduled = scheduler.try_schedule_next().await;
        assert!(scheduled.is_some());
        let (run_id, lane) = scheduled.unwrap();
        assert_eq!(run_id, "run-1");
        assert_eq!(lane, Lane::Main);

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 0);
        assert_eq!(stats.total_running, 1);
    }

    #[tokio::test]
    async fn test_scheduler_priority_ordering() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Enqueue to different lanes
        scheduler.enqueue("cron-1".to_string(), Lane::Cron).await;
        scheduler.enqueue("subagent-1".to_string(), Lane::Subagent).await;
        scheduler.enqueue("main-1".to_string(), Lane::Main).await;
        scheduler.enqueue("nested-1".to_string(), Lane::Nested).await;

        // Should schedule in priority order: Main (10) > Nested (8) > Subagent (5) > Cron (0)
        let scheduled1 = scheduler.try_schedule_next().await;
        assert_eq!(scheduled1.unwrap().1, Lane::Main);

        let scheduled2 = scheduler.try_schedule_next().await;
        assert_eq!(scheduled2.unwrap().1, Lane::Nested);

        let scheduled3 = scheduler.try_schedule_next().await;
        assert_eq!(scheduled3.unwrap().1, Lane::Subagent);

        let scheduled4 = scheduler.try_schedule_next().await;
        assert_eq!(scheduled4.unwrap().1, Lane::Cron);
    }

    #[tokio::test]
    async fn test_scheduler_global_concurrency_limit() {
        // Create a config with very low global limit
        let mut config = LaneConfig::default();
        config.global_max_concurrent = 2;
        let scheduler = LaneScheduler::new(config);

        // Enqueue 5 runs
        scheduler.enqueue("run-1".to_string(), Lane::Main).await;
        scheduler.enqueue("run-2".to_string(), Lane::Main).await;
        scheduler.enqueue("run-3".to_string(), Lane::Subagent).await;
        scheduler.enqueue("run-4".to_string(), Lane::Subagent).await;
        scheduler.enqueue("run-5".to_string(), Lane::Subagent).await;

        // Should only schedule 2 (global limit)
        let scheduled1 = scheduler.try_schedule_next().await;
        assert!(scheduled1.is_some());

        let scheduled2 = scheduler.try_schedule_next().await;
        assert!(scheduled2.is_some());

        let scheduled3 = scheduler.try_schedule_next().await;
        assert!(scheduled3.is_none()); // Global limit reached

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_running, 2);
        assert_eq!(stats.total_queued, 3);
    }

    #[tokio::test]
    async fn test_scheduler_per_lane_concurrency_limit() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Main lane has limit of 2
        scheduler.enqueue("main-1".to_string(), Lane::Main).await;
        scheduler.enqueue("main-2".to_string(), Lane::Main).await;
        scheduler.enqueue("main-3".to_string(), Lane::Main).await;

        // Schedule first two
        let scheduled1 = scheduler.try_schedule_next().await;
        assert!(scheduled1.is_some());

        let scheduled2 = scheduler.try_schedule_next().await;
        assert!(scheduled2.is_some());

        // Third should fail (lane limit reached)
        let scheduled3 = scheduler.try_schedule_next().await;
        assert!(scheduled3.is_none());

        let stats = scheduler.stats().await;
        assert_eq!(stats.lanes.get(&Lane::Main).unwrap().running, 2);
        assert_eq!(stats.lanes.get(&Lane::Main).unwrap().queued, 1);
    }

    #[tokio::test]
    async fn test_scheduler_on_run_complete() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        scheduler.enqueue("run-1".to_string(), Lane::Main).await;
        let scheduled = scheduler.try_schedule_next().await;
        assert!(scheduled.is_some());

        let stats_before = scheduler.stats().await;
        assert_eq!(stats_before.total_running, 1);

        scheduler.on_run_complete("run-1", Lane::Main).await;

        let stats_after = scheduler.stats().await;
        assert_eq!(stats_after.total_running, 0);
    }

    #[tokio::test]
    async fn test_scheduler_stats() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        scheduler.enqueue("main-1".to_string(), Lane::Main).await;
        scheduler.enqueue("main-2".to_string(), Lane::Main).await;
        scheduler.enqueue("sub-1".to_string(), Lane::Subagent).await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 3);
        assert_eq!(stats.total_running, 0);
        assert_eq!(stats.global_available_permits, 16);

        // Schedule one
        scheduler.try_schedule_next().await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 2);
        assert_eq!(stats.total_running, 1);
    }
}
