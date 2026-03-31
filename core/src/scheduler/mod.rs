//! Lane-based scheduling for sub-agent execution
//!
//! Provides resource isolation, anti-starvation, and recursion depth limits.

mod anti_starvation;
mod lane_config;
mod lane_scheduler;
mod lane_state;
mod recursion_tracker;

pub use anti_starvation::WaitTimeTracker;
pub use lane_config::{LaneConfig, LaneQuota};
pub use lane_scheduler::{LaneScheduler, LaneStats, ScheduleGuard, SchedulerStats};
pub use lane_state::{LaneState, QueuedRun};
pub use recursion_tracker::RecursionTracker;
