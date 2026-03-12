//! Agent loop — unified tool trait and flat registry.
//!
//! Replaces the 3-trait tool hierarchy (AlephTool + AlephToolDyn + CapabilityStrategy)
//! with a single `LoopTool` trait and a flat `LoopToolRegistry`.

pub mod adapters;
pub mod factory;
mod loop_core;
mod prompt_builder;
pub mod provider_bridge;
mod safety;
mod tool;

pub use factory::LoopFactory;
pub use loop_core::{
    LoopCallback, LoopConfig, LoopMessage, LoopRunResult, AgentLoop, LoopProvider,
    NoopCallback,
};
pub use prompt_builder::{PromptBuilder, ToolInfo};
pub use safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
pub use provider_bridge::AiProviderBridge;
pub use tool::{LoopTool, LoopToolRegistry, ToolDefinition, ToolResult};
