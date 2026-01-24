//! Core event handler components for the agentic loop.
//!
//! This module provides the 8 core components:
//! - `IntentAnalyzer`: Input analysis and complexity detection
//! - `TaskPlanner`: LLM-based task decomposition
//! - `ToolExecutor`: Tool execution with retry logic
//! - `LoopController`: Agentic loop control with protection mechanisms
//! - `SessionRecorder`: State persistence to SQLite
//! - `SessionCompactor`: Token management and session compaction
//! - `SubAgentHandler`: Sub-agent lifecycle management (Phase 4)
//! - `CallbackBridge`: Forwards events to Swift via FFI (Phase 5)

mod callback_bridge;
mod intent_analyzer;
mod loop_controller;
mod session_compactor;
mod session_recorder;
mod subagent_handler;
mod task_planner;
mod tool_executor;
mod types;

#[cfg(test)]
mod integration_test;

pub use callback_bridge::CallbackBridge;
pub use intent_analyzer::IntentAnalyzer;
pub use loop_controller::{LoopConfig, LoopController};
pub use session_compactor::{
    CompactionConfig, EnhancedTokenUsage, LlmCallback, ModelLimit, PruneInfo, SessionCompactor,
    TokenTracker, compaction_prompt,
};
pub use session_recorder::{RecorderError, SessionRecord, SessionRecorder};
pub use subagent_handler::SubAgentHandler;
pub use task_planner::TaskPlanner;
pub use tool_executor::{ToolExecutor, ToolRetryPolicy};
pub use types::*;
