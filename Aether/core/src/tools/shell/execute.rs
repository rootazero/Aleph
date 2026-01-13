//! Shell Execute Tool
//!
//! Executes shell commands with security controls.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

use super::ShellContext;
use crate::error::{AetherError, Result};
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for shell_exec tool
#[derive(Debug, Deserialize)]
struct ShellExecParams {
    /// Shell command to execute
    command: String,
    /// Working directory (optional)
    #[serde(default)]
    cwd: Option<String>,
    /// Environment variables (optional)
    #[serde(default)]
    env: Option<std::collections::HashMap<String, String>>,
}

/// Shell execute tool
///
/// Executes shell commands with security controls:
/// - Command whitelist enforcement
/// - Blocked command filtering
/// - Timeout protection
/// - Working directory validation
///
/// This tool ALWAYS requires user confirmation.
pub struct ShellExecuteTool {
    ctx: ShellContext,
}

impl ShellExecuteTool {
    /// Create a new ShellExecuteTool with the given context
    pub fn new(ctx: ShellContext) -> Self {
        Self { ctx }
    }

    /// Execute the command and capture output
    async fn run_command(
        &self,
        command: &str,
        cwd: Option<&str>,
        env: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<ToolResult> {
        // Validate command
        self.ctx.validate_command(command)?;

        // Validate working directory if provided
        if let Some(dir) = cwd {
            self.ctx.validate_directory(dir)?;
        }

        // Build command
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        if let Some(env_vars) = env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        // Execute with timeout
        let timeout_duration = Duration::from_secs(self.ctx.timeout_seconds());

        let output = match timeout(timeout_duration, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Ok(ToolResult::error(format!("Command execution failed: {}", e)));
            }
            Err(_) => {
                return Ok(ToolResult::error(format!(
                    "Command timed out after {} seconds",
                    self.ctx.timeout_seconds()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // Format output
        let content = if output.status.success() {
            if stdout.is_empty() {
                "Command completed successfully (no output)".to_string()
            } else {
                stdout.clone()
            }
        } else {
            format!(
                "Command failed with exit code {}\n{}{}",
                exit_code,
                if !stdout.is_empty() {
                    format!("stdout:\n{}\n", stdout)
                } else {
                    String::new()
                },
                if !stderr.is_empty() {
                    format!("stderr:\n{}", stderr)
                } else {
                    String::new()
                }
            )
        };

        Ok(ToolResult::success_with_data(
            content,
            json!({
                "exit_code": exit_code,
                "success": output.status.success(),
                "stdout": stdout,
                "stderr": stderr
            }),
        ))
    }
}

#[async_trait]
impl AgentTool for ShellExecuteTool {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "shell_exec",
            "Execute a shell command. Requires explicit configuration and user confirmation.",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for command execution (optional)"
                    },
                    "env": {
                        "type": "object",
                        "description": "Environment variables to set (optional)",
                        "additionalProperties": {
                            "type": "string"
                        }
                    }
                },
                "required": ["command"]
            }),
            ToolCategory::Native,
        )
        .with_confirmation(true)
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Check if shell is enabled
        if !self.ctx.is_enabled() {
            return Ok(ToolResult::error(
                "Shell execution is disabled. Enable it in configuration to use this tool.",
            ));
        }

        // Parse parameters
        let params: ShellExecParams = serde_json::from_str(args).map_err(|e| {
            AetherError::InvalidConfig {
                message: format!("Invalid shell_exec parameters: {}", e),
                suggestion: Some("Provide a valid JSON object with 'command' field".to_string()),
            }
        })?;

        self.run_command(
            &params.command,
            params.cwd.as_deref(),
            params.env.as_ref(),
        )
        .await
    }

    fn requires_confirmation(&self) -> bool {
        // Shell commands ALWAYS require confirmation
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Native
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::shell::ShellConfig;

    fn create_test_context() -> ShellContext {
        ShellContext::new(ShellConfig::with_allowed_commands(vec![
            "echo".to_string(),
            "ls".to_string(),
            "cat".to_string(),
            "pwd".to_string(),
        ]))
    }

    fn create_disabled_context() -> ShellContext {
        ShellContext::new(ShellConfig::default())
    }

    #[tokio::test]
    async fn test_shell_exec_echo() {
        let ctx = create_test_context();
        let tool = ShellExecuteTool::new(ctx);

        let args = json!({ "command": "echo hello world" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("hello world"));

        if let Some(data) = &result.data {
            assert_eq!(data["exit_code"], 0);
            assert_eq!(data["success"], true);
        }
    }

    #[tokio::test]
    async fn test_shell_exec_with_cwd() {
        let ctx = create_test_context();
        let tool = ShellExecuteTool::new(ctx);

        let args = json!({
            "command": "pwd",
            "cwd": "/tmp"
        })
        .to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        // Output should contain /tmp or /private/tmp (macOS)
        assert!(
            result.content.contains("/tmp") || result.content.contains("/private/tmp"),
            "Expected /tmp in output, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_shell_exec_command_not_allowed() {
        let ctx = create_test_context();
        let tool = ShellExecuteTool::new(ctx);

        let args = json!({ "command": "rm -rf /" }).to_string();
        let result = tool.execute(&args).await;

        // Should return error because 'rm' is not in allowed list
        assert!(result.is_err() || !result.unwrap().success);
    }

    #[tokio::test]
    async fn test_shell_exec_disabled() {
        let ctx = create_disabled_context();
        let tool = ShellExecuteTool::new(ctx);

        let args = json!({ "command": "echo test" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("disabled"));
    }

    #[tokio::test]
    async fn test_shell_exec_exit_code() {
        let ctx = ShellContext::new(ShellConfig::with_allowed_commands(vec![
            "sh".to_string(),
            "exit".to_string(),
        ]));
        let tool = ShellExecuteTool::new(ctx);

        let args = json!({ "command": "sh -c 'exit 42'" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        // Command fails but execution succeeds
        assert!(result.success);
        if let Some(data) = &result.data {
            assert_eq!(data["exit_code"], 42);
            assert_eq!(data["success"], false);
        }
    }

    #[test]
    fn test_shell_exec_metadata() {
        let ctx = create_test_context();
        let tool = ShellExecuteTool::new(ctx);

        assert_eq!(tool.name(), "shell_exec");
        assert!(tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Native);
    }

    #[test]
    fn test_shell_exec_definition() {
        let ctx = create_test_context();
        let tool = ShellExecuteTool::new(ctx);
        let def = tool.definition();

        assert_eq!(def.name, "shell_exec");
        assert!(def.requires_confirmation);
        assert_eq!(def.category, ToolCategory::Native);
    }
}
