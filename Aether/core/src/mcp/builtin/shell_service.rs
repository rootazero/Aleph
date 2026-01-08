//! Shell Command MCP Service
//!
//! Provides shell command execution with security controls.
//! Unlike other services, this does NOT use shared foundation modules
//! due to its security-sensitive nature.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

use super::BuiltinMcpService;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};

/// Shell service configuration
#[derive(Debug, Clone)]
pub struct ShellServiceConfig {
    /// Whether shell service is enabled (default: false for security)
    pub enabled: bool,
    /// Command timeout in seconds
    pub timeout_seconds: u64,
    /// Allowed commands whitelist (empty = all commands blocked)
    pub allowed_commands: Vec<String>,
}

impl Default for ShellServiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_seconds: 30,
            allowed_commands: vec![],
        }
    }
}

/// Shell command MCP service
///
/// Executes shell commands with strict security controls:
/// - Command whitelist enforcement
/// - Timeout protection
/// - Always requires user confirmation
pub struct ShellService {
    config: ShellServiceConfig,
}

impl ShellService {
    /// Create a new ShellService
    pub fn new(config: ShellServiceConfig) -> Self {
        Self { config }
    }

    /// Check if a command is allowed
    fn is_command_allowed(&self, command: &str) -> bool {
        if self.config.allowed_commands.is_empty() {
            return false;
        }

        // Extract the program name (first word)
        let program = command.split_whitespace().next().unwrap_or("");

        self.config.allowed_commands.iter().any(|allowed| {
            allowed == program || allowed == "*"
        })
    }
}

#[async_trait]
impl BuiltinMcpService for ShellService {
    fn name(&self) -> &str {
        "builtin:shell"
    }

    fn description(&self) -> &str {
        "Execute shell commands (requires explicit configuration)"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Ok(vec![])
    }

    async fn read_resource(&self, _uri: &str) -> Result<String> {
        Err(AetherError::NotFound("Shell service has no resources".to_string()))
    }

    fn list_tools(&self) -> Vec<McpTool> {
        if !self.config.enabled {
            return vec![];
        }

        vec![
            McpTool {
                name: "shell_exec".to_string(),
                description: "Execute a shell command".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute"
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Working directory (optional)"
                        }
                    },
                    "required": ["command"]
                }),
                requires_confirmation: true,
            },
        ]
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        if !self.config.enabled {
            return Ok(McpToolResult::error("Shell service is disabled"));
        }

        match name {
            "shell_exec" => {
                let command = args.get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AetherError::InvalidConfig {
                        message: "Missing 'command' argument".to_string(),
                        suggestion: None,
                    })?;

                // Check whitelist
                if !self.is_command_allowed(command) {
                    return Ok(McpToolResult::error(format!(
                        "Command not in whitelist: {}",
                        command.split_whitespace().next().unwrap_or("(empty)")
                    )));
                }

                let cwd = args.get("cwd").and_then(|v| v.as_str());

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

                // Execute with timeout
                let timeout_duration = Duration::from_secs(self.config.timeout_seconds);

                let output = match timeout(timeout_duration, cmd.output()).await {
                    Ok(Ok(output)) => output,
                    Ok(Err(e)) => {
                        return Ok(McpToolResult::error(format!("Command failed: {}", e)));
                    }
                    Err(_) => {
                        return Err(AetherError::McpTimeout);
                    }
                };

                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                Ok(McpToolResult::success(json!({
                    "exit_code": output.status.code().unwrap_or(-1),
                    "stdout": stdout,
                    "stderr": stderr,
                })))
            }

            _ => Ok(McpToolResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn requires_confirmation(&self, _tool_name: &str) -> bool {
        // Shell commands ALWAYS require confirmation
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_service() -> ShellService {
        ShellService::new(ShellServiceConfig {
            enabled: true,
            timeout_seconds: 5,
            allowed_commands: vec!["echo".to_string(), "ls".to_string()],
        })
    }

    #[tokio::test]
    async fn test_shell_exec() {
        let service = create_test_service();

        let result = service.call_tool("shell_exec", json!({
            "command": "echo hello"
        })).await.unwrap();

        assert!(result.success);
        assert!(result.content["stdout"].as_str().unwrap().contains("hello"));
        assert_eq!(result.content["exit_code"], 0);
    }

    #[tokio::test]
    async fn test_command_whitelist() {
        let service = create_test_service();

        // Blocked command
        let result = service.call_tool("shell_exec", json!({
            "command": "rm -rf /"
        })).await.unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("not in whitelist"));
    }

    #[tokio::test]
    async fn test_disabled_service() {
        let service = ShellService::new(ShellServiceConfig::default());

        // Service is disabled by default
        assert!(service.list_tools().is_empty());

        let result = service.call_tool("shell_exec", json!({
            "command": "echo test"
        })).await.unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("disabled"));
    }

    #[test]
    fn test_always_requires_confirmation() {
        let service = create_test_service();
        assert!(service.requires_confirmation("shell_exec"));
        assert!(service.requires_confirmation("any_tool"));
    }
}
