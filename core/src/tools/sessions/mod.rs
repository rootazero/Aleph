//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod policy;
pub mod visibility;

pub use policy::{A2ARule, AgentToAgentPolicy, RuleMatcher};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
