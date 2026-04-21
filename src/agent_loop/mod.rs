//! Agent Loop Module
//!
//! The core think → act loop. LLM reasons, selects tools, executes them,
//! and repeats until the task is complete.
//!
//! Phase 6c: `agent_runtime`, `factory`, `integration_probe` deleted.
//! `loop_core` is retained as the canonical AgentLoop implementation used
//! by `agents::runtime::AgentRuntime` for sub-agent execution.

mod loop_core;
pub mod subagent_runner;
pub mod truncation_recovery;

// ---------------------------------------------------------------------------
// Inline module aliases — keep `super::X` references inside loop_core.rs,
// agent_runtime.rs, factory.rs compiling until those files are deleted in 2c.
// These forward to the canonical crate paths.
// ---------------------------------------------------------------------------

pub mod adapters {
    pub use crate::harness::adapters::*;
}
pub mod background_tracker {
    pub use crate::agents::background_tracker::*;
}
pub mod chain_context {
    pub use crate::harness::chain_context::*;
}
pub mod compaction {
    pub use crate::memory::compaction::*;
}
pub mod context_budget {
    pub use crate::harness::context_budget::*;
}
pub mod context_compactor {
    pub use crate::harness::context_compactor::*;
}
pub mod exec_approval {
    pub use crate::sandbox::exec_approval::*;
}
pub mod model_behaviors {
    pub use crate::providers::model_behaviors::*;
}
pub mod provider_bridge {
    pub use crate::harness::provider_bridge::*;
}
pub mod sections {
    pub use crate::harness::sections::*;
}
pub mod skill_prefetch {
    pub use crate::harness::skill_prefetch::*;
}
pub mod stop_hooks {
    pub use crate::harness::stop_hooks::*;
}
pub mod subagent_teammates {
    pub use crate::agents::teammates::*;
}
pub mod subagent_tool {
    pub use crate::agents::subagent_tool::*;
}
pub mod tool {
    pub use crate::tools::runtime::*;
}
pub mod tool_execution_context {
    pub use crate::harness::tool_execution_context::*;
}
pub mod tool_info {
    pub use crate::tools::info::*;
}
pub mod tool_orchestrator {
    pub use crate::tools::orchestrator::*;
}
pub mod tool_pipeline {
    pub use crate::tools::pipeline::*;
}
pub mod tool_refresh {
    pub use crate::tools::refresh::*;
}
pub mod tool_result_store {
    pub use crate::tools::result_store::*;
}
pub mod tool_summary {
    pub use crate::harness::tool_summary::*;
}
pub mod trace {
    pub use crate::harness::trace::*;
}
pub mod verify_stop_hook {
    pub use crate::harness::verify_stop_hook::*;
}

// ---------------------------------------------------------------------------
// Shared types defined here (not in deleted agent_runtime.rs)
// ---------------------------------------------------------------------------

/// Shared snapshot of the parent's prompt state, used by the fork path.
///
/// Defined here (not in the deleted `agent_runtime` module) so that
/// `loop_core` and `agents::runtime` can both reference it without a cycle:
/// `loop_core` imports via `super::SharedSnapshot`, `agents::runtime` imports
/// via `crate::agent_loop::SharedSnapshot`.
pub type SharedSnapshot = crate::sync_primitives::Arc<
    crate::sync_primitives::RwLock<
        Option<crate::thinker::prompt_builder::PromptSnapshot>,
    >,
>;

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------

pub use loop_core::{AgentLoop, LoopCallback, LoopConfig, LoopProvider};
pub use crate::agents::runtime::LoopRunResult;
pub use truncation_recovery::{RecoveryAction, RecoveryPhase, TruncationRecovery};

// Trace types — previously re-exported from the stub; now forwarded directly.
pub use crate::harness::trace::{
    LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState, LoopTraceTextKind,
    LoopTraceTurnMetrics, LoopTraceTurnOutcome, ToolCallEndEvent, ToolCallStartEvent,
};
// Tool types forwarded for crate::agent_loop::ToolResult etc.
pub use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolDefinition, ToolResult};
// Refresh trait forwarded for crate::agent_loop::ToolRefreshSource.
pub use crate::tools::refresh::ToolRefreshSource;
// ToolInfo forwarded for crate::agent_loop::ToolInfo.
pub use crate::tools::info::ToolInfo;
