//! Unified agent system.
//!
//! This module provides:
//!
//! ## Sub-agent delegation (`agents::`)
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents
//! - `TaskTool`: Tool for calling sub-agents
//!
//! ## Rig-core AI Agent (`agents::rig::`)
//! - `RigAgentManager`: Main AI agent implementation using rig-core
//! - `RigAgentConfig`: Configuration for RigAgentManager
//! - `ChatMessage`, `ConversationHistory`: Message history management

mod registry;
mod task_tool;
mod types;

/// Rig-core based AI agent implementation.
pub mod rig;

#[cfg(test)]
mod integration_test;

// Sub-agent delegation exports
pub use registry::{builtin_agents, AgentRegistry};
pub use task_tool::{TaskTool, TaskToolError, TaskToolResult};
pub use types::{AgentDef, AgentMode};

// Re-export rig module types for convenience
pub use rig::{
    AgentConfig, AgentResponse, BuiltinToolConfig, ChatMessage, ConversationHistory, MessageRole,
    RigAgentConfig, RigAgentManager, ToolCallInfo, ToolCallResult,
};
