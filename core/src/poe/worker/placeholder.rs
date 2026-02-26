//! PlaceholderWorker for initial POE integration.

use async_trait::async_trait;
use std::path::PathBuf;

use crate::error::Result;
use crate::poe::types::WorkerOutput;

use super::{StateSnapshot, Worker};

// ============================================================================
// PlaceholderWorker (for initial integration)
// ============================================================================

/// Placeholder worker for initial POE integration.
///
/// This worker simulates execution without actually performing any work.
/// It is used for:
/// 1. Initial Gateway integration testing
/// 2. Contract signing workflow demonstration
/// 3. Development before full AgentLoopWorker is wired
///
/// Replace with AgentLoopWorker for production use.
pub struct PlaceholderWorker {
    /// Workspace directory
    workspace: PathBuf,
    /// Counter for executions
    execution_count: std::sync::atomic::AtomicU32,
}

impl PlaceholderWorker {
    /// Create a new PlaceholderWorker.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            execution_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Create with default workspace (/tmp/poe-workspace).
    pub fn with_default_workspace() -> Self {
        Self::new(PathBuf::from("/tmp/poe-workspace"))
    }

    /// Get execution count.
    pub fn execution_count(&self) -> u32 {
        self.execution_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl Worker for PlaceholderWorker {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        self.execution_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Simulate execution with placeholder output
        let mut output = WorkerOutput::completed(format!(
            "[PlaceholderWorker] Simulated execution of: {}{}",
            truncate_instruction(instruction, 100),
            previous_failure
                .map(|f| format!(" (retry after: {})", truncate_instruction(f, 50)))
                .unwrap_or_default()
        ));
        output.tokens_consumed = 50; // Simulated token usage
        output.steps_taken = 1;

        Ok(output)
    }

    async fn abort(&self) -> Result<()> {
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        Ok(StateSnapshot::new(self.workspace.clone()))
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> Result<()> {
        Ok(())
    }
}

/// Truncate instruction for logging.
fn truncate_instruction(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
