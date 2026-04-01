//! Role types, configuration, prompt templates, and review scoring for team agents.

pub mod prompts;
pub mod review;
pub mod types;

pub use prompts::role_prompt_template;
pub use review::{Challenge, DimensionScore, ReviewScore};
pub use types::{AgentRole, Severity, TeamRoleConfig};
