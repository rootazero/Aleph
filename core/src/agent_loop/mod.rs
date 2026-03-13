//! Agent Loop Module
//!
//! The core think → act loop. LLM reasons, selects tools, executes them,
//! and repeats until the task is complete.

pub mod adapters;
pub mod factory;
mod loop_core;
mod prompt_builder;
pub mod provider_bridge;
mod safety;
mod tool;

pub use factory::LoopFactory;
pub use loop_core::{
    AgentLoop, LoopCallback, LoopConfig, LoopMessage, LoopProvider, LoopRunResult, NoopCallback,
};
pub use prompt_builder::{PromptBuilder, ToolInfo};
pub use provider_bridge::AiProviderBridge;
pub use safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
pub use tool::{LoopTool, LoopToolRegistry, ToolDefinition, ToolResult};
