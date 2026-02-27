//! Worker abstraction for POE execution.
//!
//! This module defines the Worker trait and its implementations:
//! - `Worker`: Async trait for executing instructions with snapshot/restore
//! - `StateSnapshot`: Captures workspace state for rollback
//! - `AgentLoopWorker`: Real implementation that integrates with AgentLoop
//! - `MockWorker`: Test implementation with configurable behavior

mod agent_loop_worker;
mod callback;
mod gateway;
mod placeholder;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

use crate::error::Result;
use crate::poe::types::WorkerOutput;

// Re-exports
pub use agent_loop_worker::AgentLoopWorker;
pub use gateway::{GatewayAgentLoopWorker, create_gateway_worker};
pub use placeholder::PlaceholderWorker;

#[cfg(test)]
pub use tests::mock_worker::MockWorker;

// ============================================================================
// StateSnapshot
// ============================================================================

/// A snapshot of workspace state that can be used for rollback.
///
/// StateSnapshot captures the state of the workspace at a point in time,
/// allowing the orchestrator to restore to a known good state if execution
/// fails or needs to be retried with a different approach.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    /// When this snapshot was taken
    pub timestamp: DateTime<Utc>,

    /// Root directory of the workspace
    pub workspace: PathBuf,

    /// List of files and their content hashes at snapshot time.
    /// The hash is SHA-256 of file contents.
    pub file_hashes: Vec<(PathBuf, String)>,
}

impl StateSnapshot {
    /// Create a new empty snapshot.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            timestamp: Utc::now(),
            workspace,
            file_hashes: Vec::new(),
        }
    }

    /// Create a snapshot with the given file hashes.
    pub fn with_files(workspace: PathBuf, file_hashes: Vec<(PathBuf, String)>) -> Self {
        Self {
            timestamp: Utc::now(),
            workspace,
            file_hashes,
        }
    }

    /// Check if a file is tracked in this snapshot.
    pub fn contains_file(&self, path: &PathBuf) -> bool {
        self.file_hashes.iter().any(|(p, _)| p == path)
    }

    /// Get the hash of a specific file, if tracked.
    pub fn get_file_hash(&self, path: &PathBuf) -> Option<&str> {
        self.file_hashes
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, hash)| hash.as_str())
    }

    /// Get the number of tracked files.
    pub fn file_count(&self) -> usize {
        self.file_hashes.len()
    }
}

// ============================================================================
// Worker Trait
// ============================================================================

/// Trait for workers that execute instructions in the POE framework.
///
/// Workers are responsible for:
/// 1. Executing natural language instructions (via AI agent loops)
/// 2. Supporting abort/cancellation
/// 3. Creating and restoring snapshots for rollback
///
/// The Worker trait is designed to be implemented by different backends:
/// - `AgentLoopWorker`: Integrates with the Aleph agent loop
/// - `MockWorker`: For testing POE orchestration logic
#[async_trait]
pub trait Worker: Send + Sync {
    /// Execute an instruction, optionally with feedback from a previous failure.
    ///
    /// # Arguments
    /// * `instruction` - Natural language instruction to execute
    /// * `previous_failure` - Optional feedback from a previous failed attempt,
    ///   which the worker can use to adjust its approach
    ///
    /// # Returns
    /// * `Ok(WorkerOutput)` - Execution completed (may have succeeded or failed)
    /// * `Err(_)` - Execution could not be attempted (infrastructure error)
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput>;

    /// Abort the current execution.
    ///
    /// This should interrupt any ongoing work and return as quickly as possible.
    /// The worker may be in an inconsistent state after abort.
    async fn abort(&self) -> Result<()>;

    /// Take a snapshot of current workspace state.
    ///
    /// The snapshot can be used to restore the workspace to this point
    /// if a subsequent operation fails and needs to be rolled back.
    async fn snapshot(&self) -> Result<StateSnapshot>;

    /// Restore the workspace from a previous snapshot.
    ///
    /// # Arguments
    /// * `snapshot` - The snapshot to restore from
    ///
    /// # Errors
    /// Returns an error if restoration fails (e.g., files have been deleted
    /// that can't be recreated, or permissions issues).
    async fn restore(&self, snapshot: &StateSnapshot) -> Result<()>;
}
