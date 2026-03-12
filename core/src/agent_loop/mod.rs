//! Agent Loop Module
//!
//! Provides the minimal agent loop: think → act.

pub mod minimal;

// Re-export minimal loop types
pub use minimal::{
    AiProviderBridge, LoopCallback, LoopConfig, LoopMessage, LoopRunResult, MinimalAgentLoop,
    MinimalLoopFactory, MinimalPromptBuilder, MinimalTool, MinimalToolRegistry, MinimalProvider,
    NoopCallback, SafetyError, SafetyGuard, ToolDefinition, ToolInfo, ToolResult,
};
