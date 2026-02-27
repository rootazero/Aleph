//! Execution Engine
//!
//! Bridges the Gateway with the existing agent_loop infrastructure.
//! Manages run lifecycle, emits events, and handles cancellation.
//!
//! # Module structure
//!
//! - `engine` - Full `ExecutionEngine<P,R>` with AgentLoop integration
//! - `simple` - `SimpleExecutionEngine` for when providers/tools are not available

mod engine;
mod simple;

#[cfg(test)]
mod tests;

pub use engine::ExecutionEngine;
pub use simple::SimpleExecutionEngine;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use tokio::sync::mpsc;

use super::router::SessionKey;

/// Configuration for the execution engine
#[derive(Debug, Clone)]
pub struct ExecutionEngineConfig {
    /// Maximum concurrent runs per agent
    pub max_concurrent_runs: usize,
    /// Default timeout for runs (seconds)
    pub default_timeout_secs: u64,
    /// Enable detailed tracing
    pub enable_tracing: bool,
}

impl Default for ExecutionEngineConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 5,
            default_timeout_secs: 300,
            enable_tracing: true,
        }
    }
}

/// A run request
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// Unique run ID
    pub run_id: String,
    /// Input message
    pub input: String,
    /// Session key for context
    pub session_key: SessionKey,
    /// Optional timeout override
    pub timeout_secs: Option<u64>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Run state
#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    /// Run is queued
    Queued,
    /// Run is executing
    Running,
    /// Run is paused (waiting for user input)
    Paused { reason: String },
    /// Run completed successfully
    Completed,
    /// Run was cancelled
    Cancelled,
    /// Run failed
    Failed { error: String },
}

/// Run status information
#[derive(Debug, Clone)]
pub struct RunStatus {
    pub run_id: String,
    pub state: RunState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub steps_completed: u32,
    pub current_tool: Option<String>,
}

/// Internal run tracking
pub(crate) struct ActiveRun {
    pub(crate) request: RunRequest,
    pub(crate) state: RunState,
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    pub(crate) steps_completed: u32,
    pub(crate) current_tool: Option<String>,
    pub(crate) cancel_tx: Option<mpsc::Sender<()>>,
    pub(crate) seq_counter: AtomicU64,
    pub(crate) chunk_counter: AtomicU32,
}

impl ActiveRun {
    pub(crate) fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn next_chunk(&self) -> u32 {
        self.chunk_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Execution errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Too many concurrent runs: {0}")]
    TooManyRuns(String),

    #[error("Agent is busy: {0}")]
    AgentBusy(String),

    #[error("Run not found: {0}")]
    RunNotFound(String),

    #[error("Run is not active: {0}")]
    RunNotActive(String),

    #[error("Run was cancelled")]
    Cancelled,

    #[error("Run timed out")]
    Timeout,

    #[error("Execution failed: {0}")]
    Failed(String),
}
