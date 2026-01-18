//! Agent system for sub-agent delegation.
//!
//! This module provides:
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents (Task 2)
//! - `TaskTool`: Tool for calling sub-agents (Task 3)

mod types;

// Placeholder modules - will be implemented in subsequent tasks
// mod registry;
// mod task_tool;

pub use types::{AgentDef, AgentMode};
