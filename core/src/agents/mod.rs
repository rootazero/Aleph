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
//!
//! ## Specialized Sub-Agents (`agents::sub_agents::`)
//! - `SubAgent`: Trait for specialized sub-agents
//! - `McpSubAgent`: Sub-agent for MCP tool execution
//! - `SkillSubAgent`: Sub-agent for skill execution
//! - `DelegateTool`: Tool for delegating to sub-agents
//! - `SubAgentDispatcher`: Routes requests to appropriate sub-agents

mod registry;
mod task_tool;
mod types;

/// Rig-core based AI agent implementation.
pub mod rig;

/// Specialized sub-agents for task delegation.
pub mod sub_agents;

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

// Re-export sub_agents module types for convenience
pub use sub_agents::{
    DelegateTool, McpSubAgent, SkillSubAgent, SubAgent, SubAgentCapability, SubAgentDispatcher,
    SubAgentRequest, SubAgentResult, SubAgentType,
};
