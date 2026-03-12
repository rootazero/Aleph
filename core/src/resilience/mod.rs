//! Resilience Module — Database and Core Types
//!
//! Governance, collaboration, perception, and recovery middleware have been
//! removed as part of the agent loop migration. Only the database layer
//! (StateDatabase) and shared types remain.

pub mod database;
pub mod types;

pub use types::{
    AgentEvent, AgentTask, Lane, RiskLevel, SessionStatus, SubagentSession, TaskStatus, TaskTrace,
    TraceRole,
};

pub use database::{MemoryStats, StateDatabase, DEFAULT_EMBEDDING_DIM};
