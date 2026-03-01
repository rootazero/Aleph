//! Bash Operations Handler
//!
//! Implements shell command execution with timeout and security checks

use std::path::PathBuf;
use crate::sync_primitives::Arc;
use std::time::Duration;
use async_trait::async_trait;
use tokio::process::Command;
use crate::error::{AlephError, Result};

use super::{BashOps, ExecutorContext, AtomicResult};

/// Bash operations handler
///
/// Handles shell command execution with timeout control.
pub struct BashOpsHandler {
    /// Shared execution context
    context: Arc<ExecutorContext>,

    /// Command execution timeout
    command_timeout: Duration,
}

impl BashOpsHandler {
    /// Create a new bash operations handler
    ///
    /// # Arguments
    ///
    /// * `context` - Shared execution context
    /// * `command_timeout` - Maximum command execution time (default: 30s)
    pub fn new(context: Arc<ExecutorContext>, command_timeout: Duration) -> Self {
        Self {
            context,
            command_timeout,
        }
    }
}

#[async_trait]
impl BashOps for BashOpsHandler {
    async fn execute(&self, command: &str, cwd: Option<&str>) -> Result<AtomicResult> {
        // Security gate: validate command through exec parser before execution
        let analysis = crate::exec::parser::analyze_shell_command(command, None, None);
        if !analysis.ok {
            let reason = analysis.reason.unwrap_or_else(|| "command rejected by security check".to_string());
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!("Security: {}", reason)),
            });
        }

        let work_dir = if let Some(cwd) = cwd {
            PathBuf::from(cwd)
        } else {
            self.context.working_dir.clone()
        };

        // Execute command with timeout
        let output = tokio::time::timeout(
            self.command_timeout,
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&work_dir)
                .output(),
        )
        .await
        .map_err(|_| AlephError::tool(format!("Command timeout after {:?}", self.command_timeout)))?
        .map_err(|e| AlephError::tool(format!("Failed to execute command: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(AtomicResult {
                success: true,
                output: stdout,
                error: None,
            })
        } else {
            Ok(AtomicResult {
                success: false,
                output: stdout,
                error: Some(stderr),
            })
        }
    }
}
