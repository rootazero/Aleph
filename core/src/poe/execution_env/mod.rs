//! Execution environment abstraction for POE.
//!
//! Phase 1: HostEnvironment (direct command execution)
//! Phase 3: SandboxEnvironment (container/WASM isolation)

pub mod host;

use async_trait::async_trait;
use std::path::Path;

/// Output from executing a command in an environment.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Abstraction over command execution environment.
#[async_trait]
pub trait ExecutionEnvironment: Send + Sync {
    /// Execute a command with arguments.
    async fn execute_command(
        &self,
        cmd: &str,
        args: &[String],
        timeout_ms: u64,
        working_dir: Option<&Path>,
    ) -> crate::error::Result<CommandOutput>;

    /// Name of this environment (for logging).
    fn name(&self) -> &str;
}
