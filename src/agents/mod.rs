//! Unified agent system.
//!
//! This module provides:
//!
//! ## Agent types and registry (`agents::`)
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs `SubAgent` distinction
//! - `AgentRegistry`: Registry for managing agents
//!
//! ## Sub-agent infrastructure (`agents::sub_agents::`)
//! - `SubAgentRequest` / `SubAgentResult`: delegation request/result types (used by A2A)

mod registry;
mod run_context;
mod types;

pub mod loader;
pub(crate) mod tool_sets;

pub mod allowlist_tool_service;
pub mod background_tracker;
pub mod forwarding_trace_sink;
pub mod progress;
pub mod runtime;
pub mod subagent_spawner;
pub mod subagent_tool;
pub mod subagent_tree_events;
pub mod teammates;

/// Thinking levels system for LLM reasoning depth control.
pub mod thinking;

/// Specialized sub-agents for task delegation.
pub mod sub_agents;

/// Swarm intelligence for horizontal agent collaboration.
pub mod swarm;

pub use forwarding_trace_sink::ForwardingTraceSink;
pub use registry::{builtin_agents, plugin_subagents, publish_plugin_subagents, AgentRegistry};
pub use run_context::{current_agent_id, with_agent_id};
pub use runtime::{AgentRuntime, AgentRuntimeConfig, SubagentTranscript, TranscriptOutcome};
pub use types::{
    AgentDef, AgentMode, AgentSource, ContextMode, IsolationMode, McpInlineConfig, McpServerSpec,
};

// Re-export sub_agents module types for convenience
pub use sub_agents::{SubAgentRequest, SubAgentResult};

// Re-export thinking module types for convenience
pub use thinking::{normalize_think_level, ThinkLevel, THINK_LEVEL_SESSION_KEY};
