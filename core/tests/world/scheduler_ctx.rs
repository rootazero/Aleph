//! Scheduler Context for BDD tests
//!
//! Provides shared state for testing lane scheduling components:
//! - LaneState queue and semaphore management
//! - LaneScheduler priority-based scheduling
//! - Anti-starvation logic
//! - Recursion depth tracking

use std::sync::Arc;

use alephcore::scheduler::{LaneState, LaneScheduler, LaneConfig};
use alephcore::agents::sub_agents::Lane;

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

    // LaneScheduler testing
    /// Current lane scheduler under test
    pub lane_scheduler: Option<Arc<LaneScheduler>>,
    /// Last scheduled run (run_id, lane)
    pub last_scheduled: Option<(String, Lane)>,
    /// Number of runs that received anti-starvation boost
    pub anti_starvation_boost_count: usize,
    /// Last recursion depth check result
    pub recursion_check_result: Option<Result<(), String>>,
    /// Counter for generating unique run IDs
    pub run_counter: usize,
}

impl std::fmt::Debug for SchedulerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulerContext")
            .field("lane_state", &self.lane_state.as_ref().map(|_| "LaneState"))
            .field("dequeued_run_id", &self.dequeued_run_id)
            .field("held_permits_count", &self.held_permits_count)
            .field("last_dequeue_result", &self.last_dequeue_result)
            .field("priority_boost", &self.priority_boost)
            .field("lane_scheduler", &self.lane_scheduler.as_ref().map(|_| "LaneScheduler"))
            .field("last_scheduled", &self.last_scheduled)
            .field("anti_starvation_boost_count", &self.anti_starvation_boost_count)
            .field("recursion_check_result", &self.recursion_check_result)
            .field("run_counter", &self.run_counter)
            .finish()
    }
}

impl SchedulerContext {
    /// Create a new LaneState with the given max_concurrent
    pub fn create_lane_state(&mut self, max_concurrent: usize) {
        self.lane_state = Some(Arc::new(LaneState::new(max_concurrent)));
    }

    /// Create a new LaneScheduler with default config
    pub fn create_lane_scheduler(&mut self) {
        let config = LaneConfig::default();
        self.lane_scheduler = Some(Arc::new(LaneScheduler::new(config)));
    }

    /// Create a new LaneScheduler with custom config
    pub fn create_lane_scheduler_with_config(&mut self, config: LaneConfig) {
        self.lane_scheduler = Some(Arc::new(LaneScheduler::new(config)));
    }

    /// Generate a unique run ID
    pub fn generate_run_id(&mut self) -> String {
        self.run_counter += 1;
        format!("run-{}", self.run_counter)
    }

    /// Parse lane from string
    pub fn parse_lane(lane_str: &str) -> Lane {
        match lane_str {
            "Main" => Lane::Main,
            "Nested" => Lane::Nested,
            "Subagent" => Lane::Subagent,
            "Cron" => Lane::Cron,
            _ => panic!("Unknown lane: {}", lane_str),
        }
    }
}
