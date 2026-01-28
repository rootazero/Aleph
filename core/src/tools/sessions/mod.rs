//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod list;
pub mod policy;
pub mod registry;
pub mod types;
pub mod visibility;

pub use list::{SessionsListParams, SessionsListResult};
pub use policy::{A2ARule, AgentToAgentPolicy, RuleMatcher};
pub use registry::{SubagentRegistry, SubagentRun};
pub use types::{SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
