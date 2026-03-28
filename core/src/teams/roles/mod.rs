//! Role types, configuration, and review scoring for team agents.

pub mod review;
pub mod types;

pub use review::{Challenge, DimensionScore, ReviewScore};
pub use types::{AgentRole, Severity, TeamRoleConfig};
