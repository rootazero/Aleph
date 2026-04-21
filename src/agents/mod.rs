//! Unified agent system.
//!
//! This module provides:
//!
//! ## Agent types and registry (`agents::`)
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents
//!
//! ## Agent Configuration (`agents::rig::`)
//! - `RigAgentConfig`: Configuration for the agent loop
//! - `ChatMessage`, `ConversationHistory`: Message history management
//! - `BuiltinToolConfig`: Configuration for built-in tools
//! - `create_builtin_tool_server`: Create a ToolServer with built-in tools
//!
//! ## Sub-agent infrastructure (`agents::sub_agents::`)
//! - `SubAgent`: Trait for specialized sub-agents (used by A2A)

mod registry;
mod types;

pub mod allowlist_tool_service;
pub mod background_tracker;
pub mod runtime;
pub mod subagent_spawner;
pub mod subagent_tool;
pub mod teammates;

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

pub use registry::{builtin_agents, AgentRegistry};
pub use runtime::{
    AgentRuntime, AgentRuntimeConfig, SafetyGuardFactory, SubagentTranscript,
    ToolRegistryFactory, TranscriptOutcome,
};
pub use types::{AgentDef, AgentMode, ContextMode};

// Re-export rig module types for convenience
pub use rig::{
    create_builtin_tool_server, create_builtin_tools_list, AgentConfig, BuiltinToolConfig,
    ChatMessage, ConversationHistory, MessageRole, RigAgentConfig, ToolCallInfo, ToolCallResult,
};

// Re-export sub_agents module types for convenience
pub use sub_agents::{SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult};

// Re-export swarm module types for convenience
pub use swarm::{AgentEvent, AgentMessageBus, CriticalEvent, EventTier, ImportantEvent, InfoEvent};

// Re-export thinking module types for convenience
pub use thinking::{
    format_thinking_levels, get_supported_levels, is_binary_thinking_provider, is_level_supported,
    is_thinking_level_error, list_thinking_level_labels, normalize_think_level,
    supports_xhigh_thinking, ThinkLevel, ThinkingConfig, ThinkingFallbackState,
};
pub use thinking_adapter::ThinkingAdapter;
