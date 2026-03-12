//! Minimal agent loop — unified tool trait and flat registry.
//!
//! Replaces the 3-trait tool hierarchy (AlephTool + AlephToolDyn + CapabilityStrategy)
//! with a single `MinimalTool` trait and a flat `MinimalToolRegistry`.

pub mod adapters;
pub mod factory;
mod loop_core;
mod prompt_builder;
pub mod provider_bridge;
mod safety;
mod tool;

pub use factory::MinimalLoopFactory;
pub use loop_core::{
    LoopCallback, LoopConfig, LoopMessage, LoopRunResult, MinimalAgentLoop, MinimalProvider,
    NoopCallback,
};
pub use prompt_builder::{MinimalPromptBuilder, ToolInfo};
pub use safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
pub use provider_bridge::AiProviderBridge;
pub use tool::{MinimalTool, MinimalToolRegistry, ToolDefinition, ToolResult};
