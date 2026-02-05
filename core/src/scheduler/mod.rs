//! Lane-based scheduling for sub-agent execution
//!
//! Provides resource isolation, anti-starvation, and recursion depth limits.

mod lane_config;
mod lane_state;

pub use lane_config::{LaneConfig, LaneQuota};
pub use lane_state::{LaneState, QueuedRun};
