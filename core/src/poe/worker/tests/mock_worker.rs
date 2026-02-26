//! MockWorker for testing POE orchestration logic.

use async_trait::async_trait;
use std::path::PathBuf;

use crate::error::Result;
use crate::poe::types::WorkerOutput;
use crate::poe::worker::{StateSnapshot, Worker};

// ============================================================================
// MockWorker (for testing)
// ============================================================================

/// Mock worker for testing POE orchestration logic.
///
/// This worker provides configurable behavior for testing:
/// - Success/failure outcomes
/// - Token consumption per call
/// - Custom behavior via callbacks
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

impl Default for MockWorker {
    fn default() -> Self {
        Self::new()
    }
}

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
            let mut output = WorkerOutput::failed(format!("Mock failure for: {}", instruction));
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
