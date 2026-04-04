//! Agent Loop Module
//!
//! The core think → act loop. LLM reasons, selects tools, executes them,
//! and repeats until the task is complete.

pub mod adapters;
pub mod compaction;
pub mod background_tracker;
pub mod chain_context;
pub mod context_budget;
pub mod context_compactor;
pub mod factory;
mod loop_core;
pub mod model_behaviors;
mod prompt_builder;
pub mod prompt_sections;
pub mod provider_bridge;
pub mod retry;
mod safety;
pub mod sections;
pub mod skill_prefetch;
pub mod stop_hooks;
pub mod streaming_bridge;
pub mod subagent_runner;
pub mod subagent_teammates;
pub mod subagent_tool;
mod tool;
pub mod tool_orchestrator;
pub mod tool_pipeline;
pub mod tool_refresh;
pub mod tool_summary;
pub mod trace;
pub mod truncation_recovery;
pub mod verify_stop_hook;

#[cfg(test)]
mod integration_probe;

pub use chain_context::ChainContext;
pub use context_budget::diagnostics::{ContextDiagnostics, DiagnosticsSnapshot};
pub use context_budget::pipeline::{CompactionPipeline, CompactionStage, PipelineResult};
pub use context_budget::pressure::PressureSensor;
pub use context_budget::{
    ContextBudget, ContextBudgetConfig, ContextPressure, LoopDirective, TurnMetrics,
};
pub use context_compactor::{CompactResult, CompactStrategy, CompactorConfig, ContextCompactor};
pub use factory::LoopFactory;
pub use loop_core::{AgentLoop, LoopCallback, LoopConfig, LoopProvider, LoopRunResult};
pub use prompt_builder::{
    PromptBudget, PromptBuilder, PromptResult, PromptSection, Stability, ToolInfo,
};
pub use provider_bridge::AiProviderBridge;
pub use retry::{
    backoff_delay, classify_error, classify_exhausted_error, parse_token_gap, retry_async,
    RetryVerdict,
};
pub use safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
pub use sections::SessionContext;
pub use skill_prefetch::{SkillDiscoverySource, SkillInfo, SkillPrefetcher};
pub use stop_hooks::{
    ShellStopHook, StopHookAggregateResult, StopHookContext, StopHookHandler, StopHookVerdict,
};
pub use streaming_bridge::{StreamingToolBridge, StreamingToolExecutor};
pub use tool::{LoopTool, LoopToolRegistry, ToolDefinition, ToolResult};
pub use tool_pipeline::{PipelineOutcome, ToolPipeline};
pub use tool_refresh::ToolRefreshSource;
pub use tool_summary::{generate_tool_summary, ToolSummaryInput};
pub use trace::{
    LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState, LoopTraceTextKind,
    LoopTraceTurnMetrics, LoopTraceTurnOutcome, ToolCallEndEvent, ToolCallStartEvent,
};
pub use truncation_recovery::{RecoveryAction, RecoveryPhase, TruncationRecovery};
pub use verify_stop_hook::{VerifyStopHook, VerifyStopHookConfig};
