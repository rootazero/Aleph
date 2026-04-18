//! ACP session — manages a single CLI subprocess lifecycle.

use std::time::Duration;
use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use crate::acp::protocol::{AcpRequest, AcpResponse, AcpSessionState};
use crate::acp::transport::StdioTransport;
use crate::acp::AcpChunkCallback;
use crate::error::{AlephError, Result};

// =============================================================================
// AdapterConfig
// =============================================================================

/// Configuration for spawning an ACP harness subprocess.
#[derive(Debug, Clone)]
pub struct AdapterConfig {
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

impl Default for AdapterConfig {
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
// PersistedAcpSession
// =============================================================================

/// Minimal state for restoring an ACP session after restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedAcpSession {
    pub harness_id: String,
    pub acp_session_id: String,
    pub cwd: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: chrono::DateTime<chrono::Utc>,
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
    /// ACP session ID returned by `session/new`.
    acp_session_id: Option<String>,
}

impl AcpSession {
    /// Spawn a new ACP subprocess from the given config.
    pub async fn spawn(harness_id: &str, config: &AdapterConfig) -> Result<Self> {
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
            acp_session_id: None,
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

    /// Create an ACP session via `session/new` and store the returned session ID.
    ///
    /// The ACP protocol requires `session/new` after `initialize` before prompts.
    pub async fn create_acp_session(&mut self, cwd: &str, timeout: Duration) -> Result<String> {
        if let Some(ref sid) = self.acp_session_id {
            return Ok(sid.clone());
        }

        let req = AcpRequest::new_session(cwd);
        let (resp, _notifications) = self.transport.request(&req, timeout).await?;

        let session_id = resp
            .result
            .as_ref()
            .and_then(|r| r.get("sessionId"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AlephError::tool(format!(
                    "ACP harness '{}': session/new response missing sessionId",
                    self.harness_id
                ))
            })?
            .to_string();

        info!(harness_id = %self.harness_id, session_id = %session_id, "ACP session created");
        self.acp_session_id = Some(session_id.clone());
        Ok(session_id)
    }

    /// Try to restore an existing ACP session via `session/load`.
    ///
    /// Returns Ok(session_id) on success, Err on failure (caller should fall back to session/new).
    pub async fn load_acp_session(
        &mut self,
        session_id: &str,
        cwd: &str,
        timeout: Duration,
    ) -> Result<String> {
        let req = AcpRequest::load_session(session_id, cwd);
        match self.transport.request(&req, timeout).await {
            Ok((resp, _)) => {
                let sid = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(session_id)
                    .to_string();
                info!(harness_id = %self.harness_id, session_id = %sid, "ACP session loaded");
                self.acp_session_id = Some(sid.clone());
                Ok(sid)
            }
            Err(e) => {
                debug!(harness_id = %self.harness_id, error = %e, "session/load failed, will fall back to session/new");
                Err(e)
            }
        }
    }

    /// Send a prompt and collect the full response text from streaming chunks.
    ///
    /// Two execution paths:
    /// - **Streaming** (on_chunk provided): Uses `request_streaming()` to forward chunks in real-time
    /// - **Legacy** (no on_chunk): Uses `request()` to collect all notifications, extracts text after
    pub async fn prompt(
        &mut self,
        text: &str,
        cwd: &str,
        timeout: Duration,
        on_chunk: Option<&AcpChunkCallback>,
    ) -> Result<(String, Vec<AcpResponse>)> {
        if self.state == AcpSessionState::Error {
            return Err(AlephError::tool(format!(
                "ACP harness '{}' is in error state",
                self.harness_id
            )));
        }

        self.state = AcpSessionState::Busy;

        // Ensure we have an ACP session
        let session_id = self.create_acp_session(cwd, timeout).await?;
        let req = AcpRequest::prompt(&session_id, text);

        if let Some(cb) = on_chunk {
            // Streaming path: forward chunks in real-time via callback
            let cb = cb.clone();
            let accumulated = std::sync::Mutex::new(String::new());

            let result = self
                .transport
                .request_streaming(&req, timeout, |notif| {
                    if let Some(chunk) = notif.streaming_text() {
                        cb(&chunk);
                        if let Ok(mut acc) = accumulated.lock() {
                            acc.push_str(&chunk);
                        }
                    }
                })
                .await;

            match result {
                Ok(resp) => {
                    let acc_text = accumulated.into_inner().unwrap_or_default();
                    let result_text = if acc_text.is_empty() {
                        resp.text_content().unwrap_or_default()
                    } else {
                        acc_text
                    };
                    self.state = AcpSessionState::Idle;
                    Ok((result_text, vec![]))
                }
                Err(e) => {
                    error!(harness_id = %self.harness_id, error = %e, "ACP prompt failed");
                    self.state = AcpSessionState::Error;
                    Err(e)
                }
            }
        } else {
            // Legacy path: collect all notifications, extract text after
            match self.transport.request(&req, timeout).await {
                Ok((resp, notifications)) => {
                    let mut text_parts: Vec<String> = Vec::new();
                    for notif in &notifications {
                        if let Some(chunk) = notif.streaming_text() {
                            text_parts.push(chunk);
                        }
                    }
                    let result_text = if !text_parts.is_empty() {
                        text_parts.join("")
                    } else {
                        resp.text_content().unwrap_or_default()
                    };
                    self.state = AcpSessionState::Idle;
                    Ok((result_text, notifications))
                }
                Err(e) => {
                    error!(harness_id = %self.harness_id, error = %e, "ACP prompt failed");
                    self.state = AcpSessionState::Error;
                    Err(e)
                }
            }
        }
    }

    /// Send a cancel request to interrupt the current operation.
    ///
    /// No-op if no ACP session has been created yet (nothing to cancel).
    pub async fn cancel(&mut self) -> Result<()> {
        let session_id = match self.acp_session_id.as_deref() {
            Some(id) => id,
            None => {
                debug!(harness_id = %self.harness_id, "ACP cancel: no session yet, skipping");
                self.state = AcpSessionState::Idle;
                return Ok(());
            }
        };
        let req = AcpRequest::cancel(session_id);
        if let Err(e) = self.transport.send(&req).await {
            self.state = AcpSessionState::Error;
            return Err(e);
        }
        self.state = AcpSessionState::Idle;
        debug!(harness_id = %self.harness_id, "ACP cancel sent");
        Ok(())
    }

    /// Get the current session state.
    pub fn state(&self) -> AcpSessionState {
        self.state
    }

    /// Get the ACP session ID, if one has been created.
    pub fn acp_session_id(&self) -> Option<&str> {
        self.acp_session_id.as_deref()
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
        let config = AdapterConfig::default();
        assert!(config.executable.is_empty());
        assert!(config.args.is_empty());
        assert!(config.cwd.is_none());
        assert!(config.env.is_empty());
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn test_spawn_nonexistent_executable() {
        let config = AdapterConfig {
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
        let config = AdapterConfig {
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
        assert!(session.acp_session_id().is_none());
        // Drop will call start_kill
    }
}
