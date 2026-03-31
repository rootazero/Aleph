//! Agent Loop Module
//!
//! The core think → act loop. LLM reasons, selects tools, executes them,
//! and repeats until the task is complete.

pub mod adapters;
pub mod context_budget;
pub mod factory;
mod loop_core;
pub mod model_behaviors;
mod prompt_builder;
pub mod provider_bridge;
pub mod retry;
mod safety;
pub mod subagent_tool;
mod tool;
pub mod tool_orchestrator;

#[cfg(test)]
mod integration_probe;

pub use context_budget::diagnostics::{ContextDiagnostics, DiagnosticsSnapshot};
pub use context_budget::pipeline::{CompactionPipeline, CompactionStage, PipelineResult};
pub use context_budget::pressure::PressureSensor;
pub use context_budget::{
    ContextBudget, ContextBudgetConfig, ContextPressure, LoopDirective, TurnMetrics,
};
pub use factory::LoopFactory;
pub(crate) use loop_core::NoopCallback;
pub use loop_core::{AgentLoop, LoopCallback, LoopConfig, LoopProvider, LoopRunResult};
pub use prompt_builder::{PromptBuilder, ToolInfo};
pub use provider_bridge::AiProviderBridge;
pub use safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
pub use tool::{LoopTool, LoopToolRegistry, ToolDefinition, ToolResult};
