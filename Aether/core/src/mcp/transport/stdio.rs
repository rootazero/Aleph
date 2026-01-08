//! Stdio Transport for External MCP Servers
//!
//! Communicates with MCP servers via subprocess stdin/stdout using JSON-RPC.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::error::{AetherError, Result};
use crate::mcp::jsonrpc::{JsonRpcRequest, JsonRpcResponse};

/// Default timeout for RPC calls (30 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Stdio transport for communicating with MCP servers via subprocess
pub struct StdioTransport {
    /// Child process handle
    child: Mutex<Child>,
    /// Server name for logging
    server_name: String,
    /// Request timeout
    timeout: Duration,
}

impl StdioTransport {
    /// Spawn a new MCP server process
    ///
    /// # Arguments
    /// * `name` - Server name for logging
    /// * `command` - Command to execute
    /// * `args` - Command arguments
    /// * `env` - Environment variables
    /// * `cwd` - Working directory (optional)
    pub async fn spawn(
        name: impl Into<String>,
        command: impl AsRef<str>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&PathBuf>,
    ) -> Result<Self> {
        let name = name.into();
        let command_str = command.as_ref();

        tracing::info!(
            server = %name,
            command = %command_str,
            args = ?args,
            "Spawning MCP server"
        );

        let mut cmd = Command::new(command_str);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Set environment variables
        for (key, value) in env {
            cmd.env(key, value);
        }

        // Set working directory if specified
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let child = cmd.spawn().map_err(|e| {
            AetherError::IoError(format!(
                "Failed to spawn MCP server '{}' ({}): {}",
                name, command_str, e
            ))
        })?;

        tracing::info!(
            server = %name,
            pid = ?child.id(),
            "MCP server process started"
        );

        Ok(Self {
            child: Mutex::new(child),
            server_name: name,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        })
    }

    /// Set the request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Send a JSON-RPC request and wait for response
    pub async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let request_id = request.id;
        let method = &request.method;

        tracing::debug!(
            server = %self.server_name,
            id = request_id,
            method = %method,
            "Sending JSON-RPC request"
        );

        let mut child = self.child.lock().await;

        // Take stdin and stdout - we need both for communication
        // If either is missing, the process is likely dead
        let (mut stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            (stdin_opt, stdout_opt) => {
                // Put back whatever we took
                child.stdin = stdin_opt;
                child.stdout = stdout_opt;
                return Err(AetherError::IoError(format!(
                    "MCP server '{}' stdin/stdout not available",
                    self.server_name
                )));
            }
        };

        // Serialize and send request
        let request_json = match request.to_json_line() {
            Ok(json) => json,
            Err(e) => {
                // Put handles back
                child.stdin = Some(stdin);
                child.stdout = Some(stdout);
                return Err(AetherError::IoError(format!(
                    "Failed to serialize request: {}",
                    e
                )));
            }
        };

        if let Err(e) = stdin.write_all(request_json.as_bytes()).await {
            // Put handles back before returning error
            child.stdin = Some(stdin);
            child.stdout = Some(stdout);
            return Err(AetherError::IoError(format!(
                "Failed to write to MCP server '{}': {}",
                self.server_name, e
            )));
        }

        if let Err(e) = stdin.flush().await {
            // Put handles back before returning error
            child.stdin = Some(stdin);
            child.stdout = Some(stdout);
            return Err(AetherError::IoError(format!(
                "Failed to flush MCP server '{}' stdin: {}",
                self.server_name, e
            )));
        }

        // Put stdin back - we're done writing
        child.stdin = Some(stdin);

        // Read response with timeout
        let mut reader = BufReader::new(stdout);
        let mut response_line = String::new();

        let read_result = timeout(self.timeout, reader.read_line(&mut response_line)).await;

        // Put stdout back
        child.stdout = Some(reader.into_inner());

        match read_result {
            Ok(Ok(0)) => {
                Err(AetherError::IoError(format!(
                    "MCP server '{}' closed connection",
                    self.server_name
                )))
            }
            Ok(Ok(_)) => {
                let response: JsonRpcResponse = serde_json::from_str(response_line.trim())
                    .map_err(|e| {
                        AetherError::IoError(format!(
                            "Failed to parse response from '{}': {} (raw: {})",
                            self.server_name, e, response_line.trim()
                        ))
                    })?;

                tracing::debug!(
                    server = %self.server_name,
                    id = response.id,
                    success = response.is_success(),
                    "Received JSON-RPC response"
                );

                Ok(response)
            }
            Ok(Err(e)) => {
                Err(AetherError::IoError(format!(
                    "Failed to read from MCP server '{}': {}",
                    self.server_name, e
                )))
            }
            Err(_) => {
                tracing::warn!(
                    server = %self.server_name,
                    method = %method,
                    timeout_secs = self.timeout.as_secs(),
                    "MCP request timed out"
                );
                Err(AetherError::McpTimeout)
            }
        }
    }

    /// Close the transport and terminate the server process
    pub async fn close(&self) -> Result<()> {
        let mut child = self.child.lock().await;

        tracing::info!(
            server = %self.server_name,
            pid = ?child.id(),
            "Terminating MCP server"
        );

        // Try to kill gracefully first
        if let Err(e) = child.kill().await {
            tracing::warn!(
                server = %self.server_name,
                error = %e,
                "Failed to kill MCP server process"
            );
        }

        Ok(())
    }

    /// Check if the server process is still running
    pub async fn is_running(&self) -> bool {
        let mut child = self.child.lock().await;
        match child.try_wait() {
            Ok(Some(_)) => false, // Process has exited
            Ok(None) => true,     // Still running
            Err(_) => false,      // Error checking, assume dead
        }
    }

    /// Get the server name
    pub fn name(&self) -> &str {
        &self.server_name
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // The child process will be killed automatically due to kill_on_drop(true)
        tracing::debug!(
            server = %self.server_name,
            "StdioTransport dropped, server will be terminated"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_echo_server() {
        // Use echo as a simple "server" that echoes back input
        // Note: This doesn't actually test JSON-RPC, just process spawning
        let transport = StdioTransport::spawn(
            "test-echo",
            "cat", // cat echoes stdin to stdout
            &[],
            &HashMap::new(),
            None,
        )
        .await;

        assert!(transport.is_ok());
        let transport = transport.unwrap();
        assert!(transport.is_running().await);

        // Clean up
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_spawn_nonexistent_command() {
        let result = StdioTransport::spawn(
            "test-fail",
            "/nonexistent/command/that/does/not/exist",
            &[],
            &HashMap::new(),
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_timeout_configuration() {
        let transport = StdioTransport::spawn(
            "test-timeout",
            "cat",
            &[],
            &HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let transport = transport.with_timeout(Duration::from_secs(5));
        assert_eq!(transport.timeout, Duration::from_secs(5));

        transport.close().await.unwrap();
    }
}
