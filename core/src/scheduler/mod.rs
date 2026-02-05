//! Lane-based scheduling for sub-agent execution
//!
//! Provides resource isolation, anti-starvation, and recursion depth limits.

mod lane_config;

pub use lane_config::{LaneConfig, LaneQuota};
