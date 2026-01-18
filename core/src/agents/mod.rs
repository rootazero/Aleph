//! Agent system for sub-agent delegation.
//!
//! This module provides:
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents
//! - `TaskTool`: Tool for calling sub-agents

mod registry;
mod task_tool;
mod types;

#[cfg(test)]
mod integration_test;

pub use registry::{builtin_agents, AgentRegistry};
pub use task_tool::{TaskTool, TaskToolError, TaskToolResult};
pub use types::{AgentDef, AgentMode};
