//! Local tool executor for CLI client.
//!
//! Handles `tool.call` requests from Server by executing tools locally.
//! Currently only implements `shell:exec` for minimal viable validation.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

/// Parameters for shell execution
#[derive(Debug, Deserialize)]
pub struct ShellExecParams {
    /// Command to execute
    pub command: String,
    /// Working directory (optional)
    #[serde(default)]
    pub cwd: Option<String>,
    /// Timeout in seconds (default: 30)
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    30
}

/// Result of shell execution
#[derive(Debug, Serialize)]
pub struct ShellExecResult {
    /// Exit code
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Local tool executor
pub struct LocalExecutor;

impl LocalExecutor {
    /// Execute a tool call from Server
    pub async fn execute(tool_name: &str, args: Value) -> Result<Value, String> {
        info!(tool = tool_name, "Executing local tool");

        match tool_name {
            "shell:exec" | "shell_exec" | "exec" => {
                Self::execute_shell(args).await
            }
            _ => {
                warn!(tool = tool_name, "Unknown tool requested");
                Err(format!("Unknown tool: {}", tool_name))
            }
        }
    }

    /// Execute shell command
    async fn execute_shell(args: Value) -> Result<Value, String> {
        let params: ShellExecParams = serde_json::from_value(args)
            .map_err(|e| format!("Invalid shell params: {}", e))?;

        debug!(command = %params.command, "Executing shell command");

        let start = Instant::now();

        // Build command
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.args(["/C", &params.command]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", &params.command]);
            c
        };

        // Set working directory if specified
        if let Some(cwd) = &params.cwd {
            cmd.current_dir(cwd);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Execute with timeout
        let timeout = Duration::from_secs(params.timeout);
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| format!("Command timed out after {}s", params.timeout))?
            .map_err(|e| format!("Failed to execute command: {}", e))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let result = ShellExecResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            duration_ms,
        };

        info!(
            exit_code = result.exit_code,
            duration_ms = result.duration_ms,
            "Shell command completed"
        );

        Ok(json!(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shell_exec_echo() {
        let result = LocalExecutor::execute(
            "shell:exec",
            json!({"command": "echo hello"}),
        )
        .await;

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["exit_code"], 0);
        assert!(value["stdout"].as_str().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn test_shell_exec_with_cwd() {
        let result = LocalExecutor::execute(
            "shell:exec",
            json!({"command": "pwd", "cwd": "/tmp"}),
        )
        .await;

        assert!(result.is_ok());
        let value = result.unwrap();
        assert!(value["stdout"].as_str().unwrap().contains("/tmp"));
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let result = LocalExecutor::execute("unknown:tool", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown tool"));
    }
}
