//! Worker abstraction for POE execution.
//!
//! This module defines the Worker trait and its implementations:
//! - `Worker`: Async trait for executing instructions with snapshot/restore
//! - `StateSnapshot`: Captures workspace state for rollback
//! - `AgentLoopWorker`: Placeholder implementation that will integrate with AgentLoop
//! - `MockWorker`: Test implementation with configurable behavior

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

use crate::error::Result;
use crate::poe::types::WorkerOutput;

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
/// - `AgentLoopWorker`: Integrates with the Aether agent loop
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

// ============================================================================
// AgentLoopWorker
// ============================================================================

/// Worker implementation that integrates with the Aether AgentLoop.
///
/// This is a placeholder implementation. The actual integration with AgentLoop
/// will be implemented when the POE orchestrator is connected to the agent system.
///
/// # Future Implementation
///
/// The real implementation will:
/// 1. Create an AgentLoop instance with appropriate tools
/// 2. Execute the instruction through the agent loop
/// 3. Track token usage and execution steps
/// 4. Support cancellation via abort tokens
/// 5. Compute file hashes for snapshots
pub struct AgentLoopWorker {
    /// Workspace directory where the worker operates
    workspace: PathBuf,
}

impl AgentLoopWorker {
    /// Create a new AgentLoopWorker with the given workspace.
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }
}

#[async_trait]
impl Worker for AgentLoopWorker {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        // TODO: Integrate with actual AgentLoop
        //
        // Implementation plan:
        // 1. Create AgentLoop with workspace-scoped tools
        // 2. Build prompt with instruction and optional failure context
        // 3. Execute agent loop with token budget
        // 4. Collect artifacts from file system changes
        // 5. Return WorkerOutput with execution details

        // For now, return a stub completed output
        let mut output = WorkerOutput::completed(format!(
            "Placeholder execution of: {}{}",
            instruction,
            previous_failure
                .map(|f| format!(" (retry after: {})", f))
                .unwrap_or_default()
        ));

        // Stub values
        output.tokens_consumed = 100;
        output.steps_taken = 1;

        Ok(output)
    }

    async fn abort(&self) -> Result<()> {
        // TODO: Implement abort signal to AgentLoop
        //
        // Implementation plan:
        // 1. Set abort flag on the agent loop
        // 2. Wait for graceful shutdown with timeout
        // 3. Force terminate if needed

        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        // TODO: Implement actual workspace snapshot
        //
        // Implementation plan:
        // 1. Walk the workspace directory
        // 2. Compute SHA-256 hash of each file
        // 3. Record file paths and hashes

        Ok(StateSnapshot::new(self.workspace.clone()))
    }

    async fn restore(&self, snapshot: &StateSnapshot) -> Result<()> {
        // TODO: Implement actual workspace restoration
        //
        // Implementation plan:
        // 1. Compare current state with snapshot
        // 2. Delete files not in snapshot
        // 3. Restore modified files from backup or git
        // 4. Verify final state matches snapshot hashes

        // For now, just verify workspace matches
        if snapshot.workspace != self.workspace {
            return Err(crate::error::AetherError::other(format!(
                "Snapshot workspace {} does not match worker workspace {}",
                snapshot.workspace.display(),
                self.workspace.display()
            )));
        }

        Ok(())
    }
}

// ============================================================================
// MockWorker (for testing)
// ============================================================================

/// Mock worker for testing POE orchestration logic.
///
/// This worker provides configurable behavior for testing:
/// - Success/failure outcomes
/// - Token consumption per call
/// - Custom behavior via callbacks
#[cfg(test)]
pub struct MockWorker {
    /// Whether execute() should succeed or fail
    pub should_succeed: bool,

    /// Tokens to report per execute() call
    pub tokens_per_call: u32,

    /// Workspace for snapshots
    workspace: PathBuf,

    /// Counter for number of executions
    execution_count: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl MockWorker {
    /// Create a new MockWorker with default settings (succeeds, 100 tokens).
    pub fn new() -> Self {
        Self {
            should_succeed: true,
            tokens_per_call: 100,
            workspace: PathBuf::from("/tmp/mock-workspace"),
            execution_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Create a MockWorker that always fails.
    pub fn failing() -> Self {
        Self {
            should_succeed: false,
            tokens_per_call: 50,
            workspace: PathBuf::from("/tmp/mock-workspace"),
            execution_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Set whether the worker should succeed.
    pub fn with_success(mut self, success: bool) -> Self {
        self.should_succeed = success;
        self
    }

    /// Set the tokens consumed per call.
    pub fn with_tokens(mut self, tokens: u32) -> Self {
        self.tokens_per_call = tokens;
        self
    }

    /// Set the workspace path.
    pub fn with_workspace(mut self, workspace: PathBuf) -> Self {
        self.workspace = workspace;
        self
    }

    /// Get the number of times execute() has been called.
    pub fn execution_count(&self) -> u32 {
        self.execution_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Default for MockWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[async_trait]
impl Worker for MockWorker {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        // Increment execution counter
        self.execution_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        if self.should_succeed {
            let mut output = WorkerOutput::completed(format!(
                "Mock execution of: {}{}",
                instruction,
                previous_failure
                    .map(|f| format!(" (retry after: {})", f))
                    .unwrap_or_default()
            ));
            output.tokens_consumed = self.tokens_per_call;
            output.steps_taken = 1;
            Ok(output)
        } else {
            let mut output = WorkerOutput::failed(format!(
                "Mock failure for: {}",
                instruction
            ));
            output.tokens_consumed = self.tokens_per_call;
            output.steps_taken = 1;
            Ok(output)
        }
    }

    async fn abort(&self) -> Result<()> {
        // Mock abort is always successful
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        Ok(StateSnapshot::new(self.workspace.clone()))
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> Result<()> {
        // Mock restore is always successful
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_snapshot_creation() {
        let snapshot = StateSnapshot::new(PathBuf::from("/workspace"));

        assert_eq!(snapshot.workspace, PathBuf::from("/workspace"));
        assert!(snapshot.file_hashes.is_empty());
        assert_eq!(snapshot.file_count(), 0);
    }

    #[test]
    fn test_state_snapshot_with_files() {
        let files = vec![
            (PathBuf::from("foo.rs"), "abc123".to_string()),
            (PathBuf::from("bar.rs"), "def456".to_string()),
        ];

        let snapshot = StateSnapshot::with_files(PathBuf::from("/workspace"), files);

        assert_eq!(snapshot.file_count(), 2);
        assert!(snapshot.contains_file(&PathBuf::from("foo.rs")));
        assert!(!snapshot.contains_file(&PathBuf::from("baz.rs")));
        assert_eq!(
            snapshot.get_file_hash(&PathBuf::from("foo.rs")),
            Some("abc123")
        );
        assert_eq!(
            snapshot.get_file_hash(&PathBuf::from("bar.rs")),
            Some("def456")
        );
        assert_eq!(snapshot.get_file_hash(&PathBuf::from("baz.rs")), None);
    }

    #[tokio::test]
    async fn test_agent_loop_worker_stub() {
        let worker = AgentLoopWorker::new(PathBuf::from("/workspace"));

        let output = worker.execute("test instruction", None).await.unwrap();

        assert!(matches!(
            output.final_state,
            crate::poe::types::WorkerState::Completed { .. }
        ));
        assert_eq!(output.tokens_consumed, 100);
        assert_eq!(output.steps_taken, 1);
    }

    #[tokio::test]
    async fn test_agent_loop_worker_with_previous_failure() {
        let worker = AgentLoopWorker::new(PathBuf::from("/workspace"));

        let output = worker
            .execute("retry instruction", Some("previous error"))
            .await
            .unwrap();

        match &output.final_state {
            crate::poe::types::WorkerState::Completed { summary } => {
                assert!(summary.contains("retry instruction"));
                assert!(summary.contains("previous error"));
            }
            _ => panic!("Expected Completed state"),
        }
    }

    #[tokio::test]
    async fn test_agent_loop_worker_snapshot() {
        let worker = AgentLoopWorker::new(PathBuf::from("/workspace"));

        let snapshot = worker.snapshot().await.unwrap();

        assert_eq!(snapshot.workspace, PathBuf::from("/workspace"));
    }

    #[tokio::test]
    async fn test_agent_loop_worker_restore_matching_workspace() {
        let worker = AgentLoopWorker::new(PathBuf::from("/workspace"));
        let snapshot = StateSnapshot::new(PathBuf::from("/workspace"));

        // Should succeed when workspace matches
        assert!(worker.restore(&snapshot).await.is_ok());
    }

    #[tokio::test]
    async fn test_agent_loop_worker_restore_mismatched_workspace() {
        let worker = AgentLoopWorker::new(PathBuf::from("/workspace"));
        let snapshot = StateSnapshot::new(PathBuf::from("/different-workspace"));

        // Should fail when workspace doesn't match
        assert!(worker.restore(&snapshot).await.is_err());
    }

    #[tokio::test]
    async fn test_mock_worker_success() {
        let worker = MockWorker::new().with_tokens(200);

        let output = worker.execute("test", None).await.unwrap();

        assert!(matches!(
            output.final_state,
            crate::poe::types::WorkerState::Completed { .. }
        ));
        assert_eq!(output.tokens_consumed, 200);
        assert_eq!(worker.execution_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_worker_failure() {
        let worker = MockWorker::failing();

        let output = worker.execute("test", None).await.unwrap();

        assert!(matches!(
            output.final_state,
            crate::poe::types::WorkerState::Failed { .. }
        ));
        assert_eq!(worker.execution_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_worker_multiple_executions() {
        let worker = MockWorker::new();

        worker.execute("first", None).await.unwrap();
        worker.execute("second", None).await.unwrap();
        worker.execute("third", None).await.unwrap();

        assert_eq!(worker.execution_count(), 3);
    }

    #[tokio::test]
    async fn test_mock_worker_abort() {
        let worker = MockWorker::new();

        // Abort should always succeed
        assert!(worker.abort().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_worker_snapshot_restore() {
        let worker = MockWorker::new();

        let snapshot = worker.snapshot().await.unwrap();
        assert!(worker.restore(&snapshot).await.is_ok());
    }
}
