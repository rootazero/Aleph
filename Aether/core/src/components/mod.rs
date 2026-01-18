//! Core event handler components for the agentic loop.
//!
//! This module provides the 6 core components:
//! - `IntentAnalyzer`: Input analysis and complexity detection
//! - `TaskPlanner`: LLM-based task decomposition
//! - `ToolExecutor`: Tool execution with retry logic
//! - `LoopController`: Agentic loop control with protection mechanisms
//! - `SessionRecorder`: State persistence to SQLite
//! - `SessionCompactor`: Token management and session compaction

mod intent_analyzer;
mod loop_controller;
mod session_compactor;
mod session_recorder;
mod task_planner;
mod tool_executor;
mod types;

pub use intent_analyzer::IntentAnalyzer;
pub use loop_controller::{LoopConfig, LoopController};
pub use session_compactor::SessionCompactor;
pub use session_recorder::SessionRecorder;
pub use task_planner::TaskPlanner;
pub use tool_executor::{RetryPolicy, ToolExecutor};
pub use types::*;
