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
//! ## Agent Configuration (`agents::rig::`)
//! - `RigAgentConfig`: Configuration for the agent loop
//! - `ChatMessage`, `ConversationHistory`: Message history management
//! - `BuiltinToolConfig`: Configuration for built-in tools
//! - `create_builtin_tool_server`: Create a ToolServer with built-in tools
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

/// Thinking levels system for LLM reasoning depth control.
pub mod thinking;

/// Provider-specific thinking level adapters.
pub mod thinking_adapter;

/// Rig-core based AI agent implementation.
pub mod rig;

/// Specialized sub-agents for task delegation.
pub mod sub_agents;

/// Swarm intelligence for horizontal agent collaboration.
pub mod swarm;

#[cfg(test)]
mod integration_test;

// Sub-agent delegation exports
pub use registry::{builtin_agents, AgentRegistry};
pub use task_tool::{TaskTool, TaskToolError, TaskToolResult};
pub use types::{AgentDef, AgentMode};

// Re-export rig module types for convenience
pub use rig::{
    create_builtin_tool_server, create_builtin_tools_list, AgentConfig, BuiltinToolConfig,
    ChatMessage, ConversationHistory, MessageRole, RigAgentConfig, ToolCallInfo, ToolCallResult,
};

// Re-export sub_agents module types for convenience
pub use sub_agents::{
    DelegateTool, McpSubAgent, SkillSubAgent, SubAgent, SubAgentCapability, SubAgentDispatcher,
    SubAgentRequest, SubAgentResult, SubAgentType,
};

// Re-export swarm module types for convenience
pub use swarm::{
    AgentEvent, AgentMessageBus, CriticalEvent, EventTier, ImportantEvent, InfoEvent,
};

// Re-export thinking module types for convenience
pub use thinking::{
    format_thinking_levels, get_supported_levels, is_binary_thinking_provider,
    is_level_supported, is_thinking_level_error, list_thinking_level_labels,
    normalize_think_level, supports_xhigh_thinking, ThinkLevel, ThinkingConfig,
    ThinkingFallbackState,
};
pub use thinking_adapter::ThinkingAdapter;
