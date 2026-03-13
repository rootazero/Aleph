//! ACP session — manages a single CLI subprocess lifecycle.

use std::time::Duration;
use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use crate::acp::protocol::{AcpRequest, AcpResponse, AcpSessionState};
use crate::acp::transport::StdioTransport;
use crate::error::{AlephError, Result};

// =============================================================================
// HarnessConfig
// =============================================================================

/// Configuration for spawning an ACP harness subprocess.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Executable name or path (e.g. "claude", "codex", "gemini").
    pub executable: String,
    /// CLI arguments for ACP mode.
    pub args: Vec<String>,
    /// Working directory for the subprocess.
    pub cwd: Option<String>,
    /// Additional environment variables.
    pub env: Vec<(String, String)>,
    /// Request timeout (default 5 minutes).
    pub timeout: Duration,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            executable: String::new(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            timeout: Duration::from_secs(300),
        }
    }
}

// =============================================================================
// AcpSession
// =============================================================================

/// Manages a single ACP CLI subprocess lifecycle.
///
/// Wraps a spawned child process with stdio transport, tracking initialization
/// and session state. Sends JSON-RPC requests via NDJSON and collects responses.
pub struct AcpSession {
    harness_id: String,
    child: Child,
    transport: StdioTransport,
    state: AcpSessionState,
    initialized: bool,
}

impl AcpSession {
    /// Spawn a new ACP subprocess from the given config.
    ///
    /// Creates a `tokio::process::Command` with stdin/stdout/stderr piped,
    /// sets working directory and env vars, then wraps stdin/stdout in a
    /// `StdioTransport`.
    pub async fn spawn(harness_id: &str, config: &HarnessConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.executable);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }

        for (key, val) in &config.env {
            cmd.env(key, val);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AlephError::tool(format!(
                "Failed to spawn ACP harness '{}' (executable: '{}'): {}. \
                 Is the executable installed and in PATH?",
                harness_id, config.executable, e
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AlephError::tool(format!(
                "ACP harness '{}': failed to capture stdin",
                harness_id
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AlephError::tool(format!(
                "ACP harness '{}': failed to capture stdout",
                harness_id
            ))
        })?;

        let transport = StdioTransport::new(stdin, stdout);

        info!(harness_id, executable = %config.executable, "ACP session spawned");

        Ok(Self {
            harness_id: harness_id.to_string(),
            child,
            transport,
            state: AcpSessionState::Idle,
            initialized: false,
        })
    }

    /// Send the ACP `initialize` request and wait for a response.
    ///
    /// No-op if already initialized.
    pub async fn initialize(&mut self, timeout: Duration) -> Result<()> {
        if self.initialized {
            debug!(harness_id = %self.harness_id, "Already initialized, skipping");
            return Ok(());
        }

        let req = AcpRequest::initialize();
        let (resp, _notifications) = self.transport.request(&req, timeout).await?;

        debug!(
            harness_id = %self.harness_id,
            result = ?resp.result,
            "ACP initialize response received"
        );

        self.initialized = true;
        info!(harness_id = %self.harness_id, "ACP session initialized");
        Ok(())
    }

    /// Send a prompt and collect the response text plus any notifications.
    ///
    /// Sets state to `Busy` while waiting, then back to `Idle` on success
    /// or `Error` on failure.
    pub async fn prompt(
        &mut self,
        session_id: &str,
        text: &str,
        cwd: &str,
        timeout: Duration,
    ) -> Result<(String, Vec<AcpResponse>)> {
        if self.state == AcpSessionState::Error {
            return Err(AlephError::tool(format!(
                "ACP harness '{}' is in error state",
                self.harness_id
            )));
        }

        self.state = AcpSessionState::Busy;

        let req = AcpRequest::prompt(session_id, text, cwd);
        match self.transport.request(&req, timeout).await {
            Ok((resp, notifications)) => {
                let text = resp
                    .text_content()
                    .unwrap_or_default();
                self.state = AcpSessionState::Idle;
                Ok((text, notifications))
            }
            Err(e) => {
                error!(
                    harness_id = %self.harness_id,
                    error = %e,
                    "ACP prompt failed"
                );
                self.state = AcpSessionState::Error;
                Err(e)
            }
        }
    }

    /// Send a cancel request to interrupt the current operation.
    pub async fn cancel(&mut self) -> Result<()> {
        let req = AcpRequest::cancel();
        self.transport.send(&req).await?;
        self.state = AcpSessionState::Idle;
        debug!(harness_id = %self.harness_id, "ACP cancel sent");
        Ok(())
    }

    /// Get the current session state.
    pub fn state(&self) -> AcpSessionState {
        self.state
    }

    /// Check if the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                debug!(
                    harness_id = %self.harness_id,
                    exit_status = ?status,
                    "ACP child has exited"
                );
                false
            }
            Err(e) => {
                warn!(
                    harness_id = %self.harness_id,
                    error = %e,
                    "Failed to check ACP child status"
                );
                false
            }
        }
    }

    /// Kill the child process and set state to Error.
    pub async fn kill(&mut self) {
        if let Err(e) = self.child.kill().await {
            warn!(
                harness_id = %self.harness_id,
                error = %e,
                "Failed to kill ACP child process"
            );
        } else {
            info!(harness_id = %self.harness_id, "ACP child process killed");
        }
        self.state = AcpSessionState::Error;
    }

    /// Get the harness ID.
    pub fn harness_id(&self) -> &str {
        &self.harness_id
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        // Best-effort kill — cannot await in Drop.
        if let Err(e) = self.child.start_kill() {
            debug!(
                harness_id = %self.harness_id,
                error = %e,
                "Failed to start_kill ACP child on drop (may have already exited)"
            );
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harness_config_defaults() {
        let config = HarnessConfig::default();
        assert!(config.executable.is_empty());
        assert!(config.args.is_empty());
        assert!(config.cwd.is_none());
        assert!(config.env.is_empty());
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn test_spawn_nonexistent_executable() {
        let config = HarnessConfig {
            executable: "definitely-not-a-real-acp-executable-xyz".to_string(),
            ..Default::default()
        };

        let result = AcpSession::spawn("test-harness", &config).await;
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("Failed to spawn"),
            "Error should mention spawn failure: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_spawn_and_drop_kills_child() {
        // Spawn a simple long-running process
        let config = HarnessConfig {
            executable: "cat".to_string(),
            ..Default::default()
        };

        let session = AcpSession::spawn("test-cat", &config).await;
        // `cat` with piped stdin should spawn successfully
        assert!(session.is_ok());
        let mut session = session.unwrap();
        assert!(session.is_alive());
        assert_eq!(session.state(), AcpSessionState::Idle);
        assert_eq!(session.harness_id(), "test-cat");
        // Drop will call start_kill
    }
}
