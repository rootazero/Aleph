//! Agent system for sub-agent delegation.
//!
//! This module provides:
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents
//! - `TaskTool`: Tool for calling sub-agents (Task 3)

mod registry;
mod types;

// Placeholder module - will be implemented in subsequent task
// mod task_tool;

pub use registry::{builtin_agents, AgentRegistry};
pub use types::{AgentDef, AgentMode};
