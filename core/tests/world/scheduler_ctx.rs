//! Scheduler Context for BDD tests
//!
//! Provides shared state for testing lane scheduling components:
//! - LaneState queue and semaphore management
//! - LaneScheduler priority-based scheduling
//! - Anti-starvation logic
//! - Recursion depth tracking

use std::sync::Arc;

use alephcore::scheduler::LaneState;

/// Scheduler test context
#[derive(Default)]
pub struct SchedulerContext {
    // LaneState testing
    /// Current lane state under test
    pub lane_state: Option<Arc<LaneState>>,
    /// Dequeued run ID
    pub dequeued_run_id: Option<String>,
    /// Number of permits currently held (for tracking)
    pub held_permits_count: usize,
    /// Last dequeue result (success/failure)
    pub last_dequeue_result: Option<bool>,
    /// Priority boost value
    pub priority_boost: Option<i8>,
}

impl std::fmt::Debug for SchedulerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulerContext")
            .field("lane_state", &self.lane_state.as_ref().map(|_| "LaneState"))
            .field("dequeued_run_id", &self.dequeued_run_id)
            .field("held_permits_count", &self.held_permits_count)
            .field("last_dequeue_result", &self.last_dequeue_result)
            .field("priority_boost", &self.priority_boost)
            .finish()
    }
}

impl SchedulerContext {
    /// Create a new LaneState with the given max_concurrent
    pub fn create_lane_state(&mut self, max_concurrent: usize) {
        self.lane_state = Some(Arc::new(LaneState::new(max_concurrent)));
    }
}
