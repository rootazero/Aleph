//! Multi-Agent Resilience Module
//!
//! Provides database operations for the Multi-Agent Resilience architecture:
//! - Agent task tracking with recovery support
//! - Execution traces for Shadow Replay
//! - Tiered event persistence (Skeleton & Pulse)
//! - Subagent session management (Session-as-a-Service)

mod events;
mod sessions;
mod tasks;
mod traces;
mod types;

pub use types::{
    AgentEvent, AgentTask, Lane, RiskLevel, SessionStatus, SubagentSession, TaskStatus, TaskTrace,
    TraceRole,
};
