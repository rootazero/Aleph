//! # Aleph Protocol
//!
//! Pure type definitions for Aleph protocol communication.
//!
//! This crate contains only data types with no runtime dependencies,
//! making it suitable for use by any client implementation.
//!
//! ## Modules
//!
//! - [`jsonrpc`] - JSON-RPC 2.0 protocol types
//! - [`events`] - Streaming event types
//! - [`thinking`] - Reasoning and confidence types
//! - [`auth`] - Authentication and authorization types
//! - [`invitation`] - Guest invitation types
//! - [`discovery`] - Service discovery types

pub mod auth;
pub mod canvas_format;
pub mod desktop_bridge;
pub mod discovery;
pub mod events;
pub mod invitation;
mod ids;
pub mod jsonrpc;
pub mod subagent_tree;
pub mod thinking;
pub mod trace_presentation;
pub mod trace_replay;
pub mod voice_text;

// Re-export commonly used types at crate root
pub use auth::{GuestScope, IdentityContext, Role};
pub use discovery::DiscoveredInstance;
pub use events::{
    AgentTraceEvent, AgentTraceSessionOutcome, AgentTraceState, AgentTraceTextKind,
    AgentTraceToolCallEnd, AgentTraceToolCallStart, AgentTraceToolResult, AgentTraceTurnMetrics,
    AgentTraceTurnOutcome, ConfigChangedEvent, ModelInfo, RunSummary, StreamEvent,
    TokenBreakdownView, ToolErrorItem, ToolResult, ToolSummaryItem, UncertaintyAction,
};
pub use invitation::{ActivateInvitationRequest, CreateInvitationRequest, GuestToken, Invitation};
pub use jsonrpc::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, ToolCallContext, ToolCallParams, ToolCallResult,
};
pub use subagent_tree::{
    build_tree, NodeLifecycle, Rollup, SubagentNode, SubagentTreeEvent, TreeNode,
};
pub use thinking::{ConfidenceLevel, ReasoningStepType};
pub use trace_presentation::{
    present_agent_trace_event, present_agent_trace_event_with_labels_and_preset,
    present_agent_trace_event_with_preset, summarize_tool_input, summarize_tool_output,
    summarize_tool_result, AgentTracePresentation, AgentTracePresentationLabels,
    AgentTracePresentationOptions, AgentTracePresentationPreset, AgentTracePresentationStatus,
};
pub use trace_replay::{
    AgentTraceReplay, AgentTraceReplayEntry, AgentTraceReplayListItem, AgentTraceTaskSummary,
};
