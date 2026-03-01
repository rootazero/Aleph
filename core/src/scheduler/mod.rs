//! Lane-based scheduling for sub-agent execution
//!
//! Provides resource isolation, anti-starvation, and recursion depth limits.

mod lane_config;
mod lane_state;
mod lane_scheduler;
mod anti_starvation;
mod recursion_tracker;

pub use lane_config::{LaneConfig, LaneQuota};
pub use lane_state::{LaneState, QueuedRun};
pub use lane_scheduler::{LaneScheduler, ScheduleGuard, SchedulerStats, LaneStats};
pub use anti_starvation::WaitTimeTracker;
pub use recursion_tracker::RecursionTracker;
