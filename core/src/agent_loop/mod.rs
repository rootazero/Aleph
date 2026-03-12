//! Agent Loop Module
//!
//! Provides the core agent loop: think → act.

// Implementation lives in the `minimal` submodule (historical name, kept for file layout).
mod minimal;

// Re-export all public types at the agent_loop level
pub use minimal::{
    AiProviderBridge, LoopCallback, LoopConfig, LoopMessage, LoopRunResult, AgentLoop,
    LoopFactory, PromptBuilder, LoopTool, LoopToolRegistry, LoopProvider,
    NoopCallback, SafetyError, SafetyGuard, ToolDefinition, ToolInfo, ToolResult,
};

// Re-export submodules for direct access
pub use minimal::adapters;
pub use minimal::provider_bridge;
