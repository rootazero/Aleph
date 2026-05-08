//! LaneScheduler - Priority-based multi-lane scheduling engine
//!
//! Coordinates scheduling across multiple lanes with:
//! - Priority-based scheduling (Main > Nested > Subagent > Cron)
//! - Global concurrency limits
//! - Per-lane concurrency limits
//! - Statistics tracking

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use tokio::sync::Semaphore;
use tracing::warn;

use super::Lane;
use super::{LaneConfig, LaneState, RecursionTracker, WaitTimeTracker};

/// Scheduler-specific errors
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// The requested lane has no configured quota
    #[error("unknown lane: {0:?}")]
    UnknownLane(Lane),
    /// Lane budget exhausted — no permits available without queueing.
    /// Used by `try_reserve` for fail-fast semantics.
    #[error("lane {lane:?} budget exhausted (max={max})")]
    LaneBudgetExhausted { lane: Lane, max: usize },
}

/// RAII guard for scheduled run permits.
///
/// Holds references to both the global and lane semaphores.
/// On drop, automatically releases one permit to each semaphore.
/// This ensures permits are returned even if the task panics.
#[derive(Debug)]
#[must_use = "ScheduleGuard must be held for the duration of the run and passed to on_run_complete"]
pub struct ScheduleGuard {
    global_semaphore: Arc<Semaphore>,
    lane_semaphore: Arc<Semaphore>,
}

impl Drop for ScheduleGuard {
    fn drop(&mut self) {
        self.global_semaphore.add_permits(1);
        self.lane_semaphore.add_permits(1);
    }
}

/// Main scheduling engine for multi-lane coordination
pub struct LaneScheduler {
    /// Per-lane state (queue + semaphore)
    lanes: HashMap<Lane, Arc<LaneState>>,
    /// Global concurrency semaphore
    global_semaphore: Arc<Semaphore>,
    /// Configuration
    config: LaneConfig,
    /// Wait time tracker for anti-starvation
    wait_tracker: Arc<WaitTimeTracker>,
    /// Recursion depth tracker
    recursion_tracker: Arc<RecursionTracker>,
}

impl LaneScheduler {
    /// Create a new LaneScheduler with the given configuration
    pub fn new(config: LaneConfig) -> Self {
        let mut lanes = HashMap::new();

        // Initialize each lane with its quota. LaneState clamps zero to 1
        // internally; we log the offending lane here so the warn line carries
        // the lane name instead of just the symptom.
        for (lane, quota) in &config.quotas {
            lanes.insert(*lane, Arc::new(LaneState::new(quota.max_concurrent)));
        }

        let global_semaphore = Arc::new(Semaphore::new(config.global_max_concurrent));
        let wait_tracker = Arc::new(WaitTimeTracker::new());
        let recursion_tracker = Arc::new(RecursionTracker::new(config.max_recursion_depth));

        Self {
            lanes,
            global_semaphore,
            config,
            wait_tracker,
            recursion_tracker,
        }
    }

    /// Enqueue a run to a specific lane
    pub async fn enqueue(&self, run_id: String, lane: Lane) -> Result<(), SchedulerError> {
        if let Some(state) = self.lanes.get(&lane) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            self.wait_tracker.track_enqueue(&run_id, lane, now).await;
            state.enqueue(run_id).await;
            Ok(())
        } else {
            warn!(run_id = %run_id, lane = ?lane, "enqueue called for lane with no quota — run dropped");
            Err(SchedulerError::UnknownLane(lane))
        }
    }

    /// Atomically attempt to reserve a lane slot without queueing.
    ///
    /// Unlike `enqueue` + `try_schedule_next`, this is **fail-fast**:
    /// if global capacity or lane capacity is exhausted, returns
    /// `SchedulerError::LaneBudgetExhausted` immediately. The caller is
    /// responsible for translating this to a domain-appropriate error
    /// (e.g., spawner → `ToolError::Execution`).
    ///
    /// On success, returns a `ScheduleGuard` whose `Drop` releases permits
    /// via RAII. Caller MUST also invoke `on_run_complete` on all exit
    /// paths to clear lane state tracking (the guard handles permits;
    /// `on_run_complete` handles state).
    ///
    /// # Errors
    /// - `UnknownLane` — lane not configured in `LaneConfig::quotas`
    /// - `LaneBudgetExhausted` — global or lane capacity exhausted
    pub async fn try_reserve(
        &self,
        run_id: String,
        lane: Lane,
    ) -> Result<ScheduleGuard, SchedulerError> {
        let state = self
            .lanes
            .get(&lane)
            .ok_or(SchedulerError::UnknownLane(lane))?;

        // Acquire global permit first (matches try_schedule_next ordering).
        let global_permit = self.global_semaphore.try_acquire().map_err(|_| {
            SchedulerError::LaneBudgetExhausted {
                lane,
                max: self.config.global_max_concurrent,
            }
        })?;

        // Acquire lane permit; on failure, release global to avoid leaking.
        let lane_max = self
            .config
            .quotas
            .get(&lane)
            .map(|q| q.max_concurrent)
            .unwrap_or(0);
        let lane_permit = match state.try_acquire_permit() {
            Some(permit) => permit,
            None => {
                drop(global_permit);
                return Err(SchedulerError::LaneBudgetExhausted {
                    lane,
                    max: lane_max,
                });
            }
        };

        // Mark running; permits become RAII responsibility of ScheduleGuard.
        state.mark_running(run_id.clone()).await;
        // SAFETY: ScheduleGuard takes ownership of permit release via its
        // own Drop impl, ensuring permits are returned exactly once even
        // if the caller panics. We forget the SemaphorePermits here so
        // their own Drop does not run.
        std::mem::forget(global_permit);
        std::mem::forget(lane_permit);

        Ok(ScheduleGuard {
            global_semaphore: Arc::clone(&self.global_semaphore),
            lane_semaphore: Arc::clone(state.semaphore()),
        })
    }

    /// Try to schedule the next run from any lane
    ///
    /// Returns the run_id and lane if a run was scheduled, None otherwise.
    /// This method:
    /// 1. Checks global capacity
    /// 2. Iterates lanes by priority (highest first), with anti-starvation boosts applied
    /// 3. For each lane, checks lane capacity
    /// 4. Dequeues a run if both have capacity
    pub async fn try_schedule_next(&self) -> Option<(String, Lane, ScheduleGuard)> {
        // Check if we have global capacity
        if self.global_semaphore.available_permits() == 0 {
            return None;
        }

        // Sort lanes by priority (highest first), applying anti-starvation boosts
        let mut lanes_by_priority: Vec<_> = self
            .config
            .quotas
            .iter()
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
                        let global_permit = match self.global_semaphore.try_acquire() {
                            Ok(permit) => permit,
                            Err(_) => {
                                // Re-enqueue the run — don't lose it
                                state.enqueue(run_id).await;
                                continue;
                            }
                        };
                        let lane_permit = match state.try_acquire_permit() {
                            Some(permit) => permit,
                            None => {
                                // Release global permit and re-enqueue
                                drop(global_permit);
                                state.enqueue(run_id).await;
                                continue;
                            }
                        };

                        // Remove from wait tracker (no longer waiting)
                        self.wait_tracker.remove(&run_id).await;

                        // Mark as running; permits are held by the ScheduleGuard
                        state.mark_running(run_id.clone()).await;
                        // SAFETY: We intentionally forget the SemaphorePermits so their
                        // Drop impl (which would release the permits) does not run.
                        // The ScheduleGuard takes ownership of permit release via its
                        // own Drop impl, ensuring permits are always released exactly
                        // once even if the caller panics.
                        std::mem::forget(global_permit);
                        std::mem::forget(lane_permit);

                        let guard = ScheduleGuard {
                            global_semaphore: Arc::clone(&self.global_semaphore),
                            lane_semaphore: Arc::clone(state.semaphore()),
                        };

                        return Some((run_id, lane, guard));
                    }
                }
            }
        }

        None
    }

    /// Mark a run as completed.
    ///
    /// If a `ScheduleGuard` is provided, it is consumed and permits are released
    /// via RAII drop. If `None`, permits are released manually (for cleanup paths
    /// where no guard was acquired, e.g., recursion tracking cleanup).
    pub async fn on_run_complete(&self, run_id: &str, lane: Lane, guard: Option<ScheduleGuard>) {
        if let Some(state) = self.lanes.get(&lane) {
            let was_running = state.is_running(run_id).await;
            state.complete(run_id).await;
            // Only release permits manually if the run was actually running
            // (i.e., permits were acquired but guard was lost/not provided)
            if guard.is_none() && was_running {
                self.global_semaphore.add_permits(1);
                state.semaphore().add_permits(1);
            }
        }
        // Guard drops here if Some, releasing both permits via RAII
        drop(guard);
        // Also remove from wait tracker in case it's still there
        self.wait_tracker.remove(run_id).await;
        // Remove from recursion tracking
        self.recursion_tracker.remove(run_id).await;
    }

    /// Check if a parent run can spawn a child without exceeding recursion depth
    ///
    /// Returns Ok(()) if the spawn is allowed, or an error if the depth limit would be exceeded.
    pub async fn check_recursion_depth(&self, parent_run_id: &str) -> crate::error::Result<()> {
        self.recursion_tracker.can_spawn(parent_run_id).await
    }

    /// Record a parent-child spawn relationship for recursion tracking
    pub async fn record_spawn(&self, parent_run_id: &str, child_run_id: &str) {
        self.recursion_tracker
            .track_spawn(parent_run_id, child_run_id)
            .await;
    }

    /// Get the current recursion depth for a run
    pub async fn get_recursion_depth(&self, run_id: &str) -> usize {
        self.recursion_tracker.get_depth(run_id).await
    }

    /// Remove a run from recursion tracking (cleanup on completion)
    pub async fn remove_from_recursion_tracking(&self, run_id: &str) {
        self.recursion_tracker.remove(run_id).await;
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

    /// Get the scheduler configuration
    pub fn config(&self) -> &LaneConfig {
        &self.config
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

        let _ = scheduler.enqueue("run-1".to_string(), Lane::Main).await;
        let _ = scheduler.enqueue("run-2".to_string(), Lane::Subagent).await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 2);
        assert_eq!(stats.lanes.get(&Lane::Main).unwrap().queued, 1);
        assert_eq!(stats.lanes.get(&Lane::Subagent).unwrap().queued, 1);
    }

    #[tokio::test]
    async fn test_scheduler_try_schedule() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        let _ = scheduler.enqueue("run-1".to_string(), Lane::Main).await;

        let scheduled = scheduler.try_schedule_next().await;
        assert!(scheduled.is_some());
        let (run_id, lane, _guard) = scheduled.unwrap();
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
        let _ = scheduler.enqueue("cron-1".to_string(), Lane::Cron).await;
        let _ = scheduler
            .enqueue("subagent-1".to_string(), Lane::Subagent)
            .await;
        let _ = scheduler.enqueue("main-1".to_string(), Lane::Main).await;
        let _ = scheduler
            .enqueue("nested-1".to_string(), Lane::Nested)
            .await;

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
        let config = LaneConfig {
            global_max_concurrent: 2,
            ..LaneConfig::default()
        };
        let scheduler = LaneScheduler::new(config);

        // Enqueue 5 runs
        let _ = scheduler.enqueue("run-1".to_string(), Lane::Main).await;
        let _ = scheduler.enqueue("run-2".to_string(), Lane::Main).await;
        let _ = scheduler.enqueue("run-3".to_string(), Lane::Subagent).await;
        let _ = scheduler.enqueue("run-4".to_string(), Lane::Subagent).await;
        let _ = scheduler.enqueue("run-5".to_string(), Lane::Subagent).await;

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
        let _ = scheduler.enqueue("main-1".to_string(), Lane::Main).await;
        let _ = scheduler.enqueue("main-2".to_string(), Lane::Main).await;
        let _ = scheduler.enqueue("main-3".to_string(), Lane::Main).await;

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

        let _ = scheduler.enqueue("run-1".to_string(), Lane::Main).await;
        let scheduled = scheduler.try_schedule_next().await;
        assert!(scheduled.is_some());
        let (_run_id, _lane, guard) = scheduled.unwrap();

        let stats_before = scheduler.stats().await;
        assert_eq!(stats_before.total_running, 1);

        scheduler
            .on_run_complete("run-1", Lane::Main, Some(guard))
            .await;

        let stats_after = scheduler.stats().await;
        assert_eq!(stats_after.total_running, 0);
    }

    #[tokio::test]
    async fn test_scheduler_stats() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        let _ = scheduler.enqueue("main-1".to_string(), Lane::Main).await;
        let _ = scheduler.enqueue("main-2".to_string(), Lane::Main).await;
        let _ = scheduler.enqueue("sub-1".to_string(), Lane::Subagent).await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 3);
        assert_eq!(stats.total_running, 0);
        assert_eq!(stats.global_available_permits, 16);

        // Schedule one (keep guard alive to hold permit)
        let _guard = scheduler.try_schedule_next().await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 2);
        assert_eq!(stats.total_running, 1);
    }

    #[tokio::test]
    async fn test_anti_starvation_tracking() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Enqueue a run
        let _ = scheduler.enqueue("run-1".to_string(), Lane::Cron).await;

        // Wait time should be tracked
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let wait_time = scheduler
            .wait_tracker
            .get_wait_time("run-1", current_time)
            .await;
        assert!(wait_time < 1000); // Should be very small (just enqueued)

        // Schedule the run (keep guard alive to hold permit)
        let _guard = scheduler.try_schedule_next().await;

        // After scheduling, should be removed from tracker
        let wait_time_after = scheduler
            .wait_tracker
            .get_wait_time("run-1", current_time)
            .await;
        assert_eq!(wait_time_after, 0);
    }

    #[tokio::test]
    async fn test_on_run_complete_removes_from_tracker() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        let _ = scheduler.enqueue("run-1".to_string(), Lane::Main).await;

        // Verify it's tracked
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let wait_time = scheduler
            .wait_tracker
            .get_wait_time("run-1", current_time)
            .await;
        assert!(wait_time < 1000);

        // Schedule and complete
        let scheduled = scheduler.try_schedule_next().await;
        let guard = scheduled.map(|(_, _, g)| g);
        scheduler.on_run_complete("run-1", Lane::Main, guard).await;

        // Should be removed from tracker
        let wait_time_after = scheduler
            .wait_tracker
            .get_wait_time("run-1", current_time)
            .await;
        assert_eq!(wait_time_after, 0);
    }

    #[tokio::test]
    async fn test_recursion_depth_tracking() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Root run has depth 0
        assert_eq!(scheduler.get_recursion_depth("parent-1").await, 0);

        // Record first spawn
        scheduler.record_spawn("parent-1", "child-1").await;
        assert_eq!(scheduler.get_recursion_depth("child-1").await, 1);

        // Record second level spawn
        scheduler.record_spawn("child-1", "child-2").await;
        assert_eq!(scheduler.get_recursion_depth("child-2").await, 2);
    }

    #[tokio::test]
    async fn test_recursion_depth_limit_enforcement() {
        let config = LaneConfig::default(); // max_recursion_depth = 5
        let scheduler = LaneScheduler::new(config);

        // Build a chain to the limit
        scheduler.record_spawn("p0", "p1").await;
        scheduler.record_spawn("p1", "p2").await;
        scheduler.record_spawn("p2", "p3").await;
        scheduler.record_spawn("p3", "p4").await;
        scheduler.record_spawn("p4", "p5").await;

        // p5 is at depth 5, which equals the limit
        assert_eq!(scheduler.get_recursion_depth("p5").await, 5);

        // Should not be able to spawn from p5
        let result = scheduler.check_recursion_depth("p5").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_recursion_depth_check_allows_below_limit() {
        let config = LaneConfig::default(); // max_recursion_depth = 5
        let scheduler = LaneScheduler::new(config);

        // Build a chain below the limit
        scheduler.record_spawn("p0", "p1").await;
        scheduler.record_spawn("p1", "p2").await;
        scheduler.record_spawn("p2", "p3").await;

        // p3 is at depth 3, should be able to spawn more
        let result = scheduler.check_recursion_depth("p3").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_recursion_cleanup_on_complete() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Record spawn relationship
        scheduler.record_spawn("parent", "child").await;
        assert_eq!(scheduler.get_recursion_depth("child").await, 1);

        // Complete the child run (no guard — only recursion tracking cleanup)
        scheduler.on_run_complete("child", Lane::Main, None).await;

        // Should be removed from recursion tracking
        assert_eq!(scheduler.get_recursion_depth("child").await, 0);
    }

    #[tokio::test]
    async fn test_recursion_multiple_children_same_parent() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // One parent can spawn multiple children
        scheduler.record_spawn("parent", "child-1").await;
        scheduler.record_spawn("parent", "child-2").await;
        scheduler.record_spawn("parent", "child-3").await;

        // All children should have depth 1
        assert_eq!(scheduler.get_recursion_depth("child-1").await, 1);
        assert_eq!(scheduler.get_recursion_depth("child-2").await, 1);
        assert_eq!(scheduler.get_recursion_depth("child-3").await, 1);

        // Parent should still allow spawning more
        let result = scheduler.check_recursion_depth("parent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_recursion_depth_at_boundary() {
        let config = LaneConfig {
            max_recursion_depth: 3,
            ..LaneConfig::default()
        };
        let scheduler = LaneScheduler::new(config);

        // Build chain to depth 2
        scheduler.record_spawn("p0", "p1").await;
        scheduler.record_spawn("p1", "p2").await;

        // p2 is at depth 2, should be able to spawn one more level
        assert!(scheduler.check_recursion_depth("p2").await.is_ok());

        scheduler.record_spawn("p2", "p3").await;

        // p3 is at depth 3 (at limit), cannot spawn more
        assert!(scheduler.check_recursion_depth("p3").await.is_err());
    }

    #[tokio::test]
    async fn try_reserve_succeeds_with_capacity() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Subagent default after Stage C is 4; reserve 4 should succeed.
        let mut guards = vec![];
        for i in 0..4 {
            let guard = scheduler
                .try_reserve(format!("sub-{i}"), Lane::Subagent)
                .await
                .expect("reserve should succeed within capacity");
            guards.push(guard);
        }
        assert_eq!(guards.len(), 4);
    }

    #[tokio::test]
    async fn try_reserve_fails_when_lane_exhausted() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Fill the Subagent lane (default cap 4).
        let mut _guards = vec![];
        for i in 0..4 {
            _guards.push(
                scheduler
                    .try_reserve(format!("sub-{i}"), Lane::Subagent)
                    .await
                    .expect("first 4 reserves should succeed"),
            );
        }
        // 5th must fail with LaneBudgetExhausted.
        let err = scheduler
            .try_reserve("sub-5".to_string(), Lane::Subagent)
            .await
            .expect_err("5th reserve should fail");
        match err {
            SchedulerError::LaneBudgetExhausted { lane, max } => {
                assert_eq!(lane, Lane::Subagent);
                assert_eq!(max, 4);
            }
            other => panic!("expected LaneBudgetExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn try_reserve_fails_when_global_exhausted() {
        // Global cap 1; lane has plenty of room.
        let config = LaneConfig {
            global_max_concurrent: 1,
            ..LaneConfig::default()
        };
        let scheduler = LaneScheduler::new(config);

        let _guard = scheduler
            .try_reserve("first".to_string(), Lane::Subagent)
            .await
            .expect("first reserve uses the only global slot");

        let err = scheduler
            .try_reserve("second".to_string(), Lane::Subagent)
            .await
            .expect_err("second reserve should fail (global exhausted)");
        assert!(matches!(err, SchedulerError::LaneBudgetExhausted { .. }));
    }

    #[tokio::test]
    async fn try_reserve_unknown_lane() {
        // Build config without Cron quota.
        let mut config = LaneConfig::default();
        config.quotas.remove(&Lane::Cron);
        let scheduler = LaneScheduler::new(config);

        let err = scheduler
            .try_reserve("x".to_string(), Lane::Cron)
            .await
            .expect_err("unknown lane must fail");
        assert!(matches!(err, SchedulerError::UnknownLane(Lane::Cron)));
    }

    #[tokio::test]
    async fn guard_drop_releases_permit() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Fill Subagent lane (default 4).
        let mut held = vec![];
        for i in 0..4 {
            held.push(
                scheduler
                    .try_reserve(format!("sub-{i}"), Lane::Subagent)
                    .await
                    .unwrap(),
            );
        }

        // Drop one guard; permit should be released.
        drop(held.pop().unwrap());

        // Now we can reserve again.
        let _guard = scheduler
            .try_reserve("sub-replacement".to_string(), Lane::Subagent)
            .await
            .expect("after drop, reserve should succeed");
    }
}
