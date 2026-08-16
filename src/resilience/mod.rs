//! Resilience Module — Database and Core Types
//!
//! Governance, collaboration, perception, and recovery middleware have been
//! removed as part of the agent loop migration. Only the database layer
//! (`StateDatabase`) and shared types remain.

pub mod database;
pub mod types;

pub use types::{AgentTask, Lane, RiskLevel, TaskStatus, TaskTrace, TaskTraceInfo};

pub use database::{AgentUsageTotal, StateDatabase, DEFAULT_EMBEDDING_DIM};
