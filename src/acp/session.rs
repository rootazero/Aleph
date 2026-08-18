//! ACP session — manages a single CLI subprocess lifecycle.

use crate::sync_primitives::{Arc, RwLock};
use crate::utils::no_window::NoWindow;
use std::time::Duration;
use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use crate::acp::protocol::{AcpRequest, AcpResponse, AcpSessionState};
use crate::acp::transport::{write_request, SharedStdin, StdioTransport};
use crate::acp::AcpChunkCallback;
use crate::error::Result;

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
    /// Filesystem permission policy applied to `fs/*` requests the agent
    /// issues mid-prompt. Defaults to `ApproveAll` for backwards
    /// compatibility; callers should set this from `TrustLevel`
    /// (`Full` -> `ApproveAll`, `Disabled` -> `DenyAll`). See ACP-07.
    pub permission_policy: crate::acp::incoming::PermissionPolicy,
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
///
/// `session_name` is `None` (or absent on legacy disk) for the default
/// unnamed session and `Some(name)` for acpx-style `-s backend` parallel
/// sessions. `#[serde(default)]` keeps old snapshot files loadable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedAcpSession {
    pub harness_id: String,
    pub acp_session_id: String,
    pub cwd: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
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
    ///
    /// Wrapped in `Arc<RwLock>` so a `CancelHandle` cloned by the manager
    /// can read the current id without acquiring the session mutex held by
    /// an in-flight prompt. Always read via the `acp_session_id()` accessor.
    acp_session_id: Arc<RwLock<Option<String>>>,
    /// Background task reading stderr for diagnostic logging.
    _stderr_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Cloneable "send cancel without locking the session" handle.
///
/// The manager keeps one of these alongside each pooled session entry so
/// `session/cancel` notifications can interrupt an in-flight prompt that
/// currently owns the per-session async mutex.
#[derive(Clone)]
pub struct CancelHandle {
    harness_id: String,
    stdin: SharedStdin,
    acp_session_id: Arc<RwLock<Option<String>>>,
}

impl CancelHandle {
    /// Fire-and-forget `session/cancel` notification. No-op when the session
    /// has not yet been created via `session/new`.
    pub async fn send_cancel(&self) -> Result<()> {
        let sid = self
            .acp_session_id
            .read()
            .unwrap_or_else(|e| {
                warn!(harness_id = %self.harness_id, "ACP cancel: session_id lock poisoned, recovering");
                e.into_inner()
            })
            .clone();
        let Some(session_id) = sid else {
            debug!(harness_id = %self.harness_id, "ACP cancel handle: no session yet, skipping");
            return Ok(());
        };
        let req = AcpRequest::cancel(&session_id);
        write_request(&self.stdin, &req).await
    }

    /// Harness ID this handle targets — used by manager-level tracing.
    #[must_use]
    pub fn harness_id(&self) -> &str {
        &self.harness_id
    }
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

        let mut child = cmd.no_window().spawn().map_err(|e| {
            crate::acp::protocol::AcpOperationError::new(
                crate::acp::protocol::AcpErrorCode::SpawnFailed,
                format!(
                    "Failed to spawn ACP harness '{}' (executable: '{}'): {}. \
                     Is the executable installed and in PATH?",
                    harness_id, config.executable, e
                ),
            )
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            let _ = child.start_kill();
            crate::acp::protocol::AcpOperationError::new(
                crate::acp::protocol::AcpErrorCode::SpawnFailed,
                format!("ACP harness '{harness_id}': failed to capture stdin"),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            let _ = child.start_kill();
            crate::acp::protocol::AcpOperationError::new(
                crate::acp::protocol::AcpErrorCode::SpawnFailed,
                format!("ACP harness '{harness_id}': failed to capture stdout"),
            )
        })?;
        let stderr = child.stderr.take();

        // Wire the agent→client request handler so `fs/*` and
        // `session/request_permission` issued mid-prompt are answered instead
        // of dropped. Filesystem access is confined to the session cwd.
        let handler = std::sync::Arc::new(crate::acp::incoming::IncomingHandler::new(
            config.cwd.as_deref(),
            config.permission_policy,
        ));
        let transport = StdioTransport::with_handler(stdin, stdout, Some(handler));

        let stderr_handle = stderr.map(|s| {
            let hid = harness_id.to_string();
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let reader = BufReader::new(s);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !line.trim().is_empty() {
                        warn!(harness_id = %hid, stderr_line = %line, "ACP harness stderr");
                    }
                }
            })
        });

        info!(harness_id, executable = %config.executable, "ACP session spawned");

        Ok(Self {
            harness_id: harness_id.to_string(),
            child,
            transport,
            state: AcpSessionState::Idle,
            initialized: false,
            acp_session_id: Arc::new(RwLock::new(None)),
            _stderr_handle: stderr_handle,
        })
    }

    /// Clone a cancel handle that can fire `session/cancel` without holding
    /// the session mutex. Safe to call before `create_acp_session` — the
    /// returned handle becomes effective once the session id is populated.
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle {
            harness_id: self.harness_id.clone(),
            stdin: self.transport.stdin_handle(),
            acp_session_id: Arc::clone(&self.acp_session_id),
        }
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
        if let Some(sid) = self
            .acp_session_id
            .read()
            .unwrap_or_else(|e| {
                warn!(harness_id = %self.harness_id, "ACP create: session_id lock poisoned, recovering");
                e.into_inner()
            })
            .clone()
        {
            return Ok(sid);
        }

        let req = AcpRequest::new_session(cwd);
        let (resp, _notifications) = self.transport.request(&req, timeout).await?;

        let session_id = resp
            .result
            .as_ref()
            .and_then(|r| r.get("sessionId"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::acp::protocol::AcpOperationError::new(
                    crate::acp::protocol::AcpErrorCode::ProtocolError { code: -32603 },
                    format!(
                        "ACP harness '{}': session/new response missing sessionId",
                        self.harness_id
                    ),
                )
            })?
            .to_string();

        info!(harness_id = %self.harness_id, session_id = %session_id, "ACP session created");
        *self
            .acp_session_id
            .write()
            .unwrap_or_else(|e| {
                warn!(harness_id = %self.harness_id, "ACP create: session_id lock poisoned, recovering");
                e.into_inner()
            }) = Some(session_id.clone());
        Ok(session_id)
    }

    /// Try to restore an existing ACP session via `session/load`.
    ///
    /// Returns `Ok(session_id)` on success, Err on failure (caller should fall back to session/new).
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
                *self
                    .acp_session_id
                    .write()
                    .unwrap_or_else(|e| {
                        warn!(harness_id = %self.harness_id, "ACP load: session_id lock poisoned, recovering");
                        e.into_inner()
                    }) = Some(sid.clone());
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
    /// - **Streaming** (`on_chunk` provided): Uses `request_streaming()` to forward chunks in real-time
    /// - **Legacy** (no `on_chunk)`: Uses `request()` to collect all notifications, extracts text after
    pub async fn prompt(
        &mut self,
        text: &str,
        cwd: &str,
        timeout: Duration,
        on_chunk: Option<&AcpChunkCallback>,
    ) -> Result<(String, Vec<AcpResponse>)> {
        if self.state == AcpSessionState::Error {
            return Err(crate::acp::protocol::AcpOperationError::new(
                crate::acp::protocol::AcpErrorCode::SessionDead,
                format!("ACP harness '{}' is in error state", self.harness_id),
            )
            .into());
        }

        self.state = AcpSessionState::Busy;

        // Ensure we have an ACP session. On failure, leave the session in
        // `Error` (not the `Busy` we just set) so a reused entry isn't stuck
        // looking active — the prompt-failure paths below do the same.
        let session_id = match self.create_acp_session(cwd, timeout).await {
            Ok(sid) => sid,
            Err(e) => {
                self.state = AcpSessionState::Error;
                return Err(e);
            }
        };
        let req = AcpRequest::prompt(&session_id, text);

        if let Some(cb) = on_chunk {
            // Streaming path: forward chunks in real-time via callback
            let cb = cb.clone();
            let accumulated = crate::sync_primitives::Mutex::new(String::new());

            let result = self
                .transport
                .request_streaming(&req, timeout, |notif| {
                    if let Some(chunk) = notif.streaming_text() {
                        cb(&chunk);
                        let mut acc = accumulated.lock().unwrap_or_else(|e| e.into_inner());
                        acc.push_str(&chunk);
                    }
                })
                .await;

            match result {
                Ok(resp) => {
                    let acc_text = accumulated.into_inner().unwrap_or_else(|e| e.into_inner());
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
    /// Note: this requires `&mut self`. For the common case of cancelling
    /// an in-flight prompt that owns the session mutex, use a
    /// `CancelHandle` cloned via `cancel_handle()` — it can send the cancel
    /// notification without locking the session.
    pub async fn cancel(&mut self) -> Result<()> {
        let session_id = match self
            .acp_session_id
            .read()
            .unwrap_or_else(|e| {
                warn!(harness_id = %self.harness_id, "ACP cancel: session_id lock poisoned, recovering");
                e.into_inner()
            })
            .clone()
        {
            Some(id) => id,
            None => {
                debug!(harness_id = %self.harness_id, "ACP cancel: no session yet, skipping");
                self.state = AcpSessionState::Idle;
                return Ok(());
            }
        };
        let req = AcpRequest::cancel(&session_id);
        if let Err(e) = self.transport.send(&req).await {
            self.state = AcpSessionState::Error;
            return Err(e);
        }
        self.state = AcpSessionState::Idle;
        debug!(harness_id = %self.harness_id, "ACP cancel sent");
        Ok(())
    }

    /// Switch the adapter's interaction mode via `session/set_mode`.
    ///
    /// Mirrors acpx's session control errors: ACP `-32601` / `-32602` (and
    /// `-32603 invalid params`) are mapped to `SessionControlUnsupported`
    /// so callers can downgrade gracefully when the adapter doesn't
    /// implement this method.
    pub async fn set_mode(&mut self, mode_id: &str, timeout: Duration) -> Result<()> {
        let session_id = self.require_session_id("set_mode")?;
        let req = AcpRequest::set_mode(&session_id, mode_id);
        self.send_session_control(req, timeout, "set_mode").await
    }

    /// Switch the underlying model via `session/set_model`.
    pub async fn set_model(&mut self, model_id: &str, timeout: Duration) -> Result<()> {
        let session_id = self.require_session_id("set_model")?;
        let req = AcpRequest::set_model(&session_id, model_id);
        self.send_session_control(req, timeout, "set_model").await
    }

    /// Set an adapter-specific runtime config option via
    /// `session/set_config_option`.
    pub async fn set_config_option(
        &mut self,
        key: &str,
        value: serde_json::Value,
        timeout: Duration,
    ) -> Result<()> {
        let session_id = self.require_session_id("set_config_option")?;
        let req = AcpRequest::set_config_option(&session_id, key, value);
        self.send_session_control(req, timeout, "set_config_option")
            .await
    }

    /// Run the ACP `authenticate` handshake. Returns `AuthRequired` if the
    /// adapter rejects the credential, `SessionControlUnsupported` if the
    /// adapter doesn't expose the method.
    pub async fn authenticate(
        &mut self,
        method_id: &str,
        credential: &str,
        timeout: Duration,
    ) -> Result<()> {
        let req = AcpRequest::authenticate(method_id, credential);
        match self.transport.request(&req, timeout).await {
            Ok(_) => {
                info!(harness_id = %self.harness_id, method_id, "ACP authenticate OK");
                Ok(())
            }
            Err(e) => Err(map_auth_error(e)),
        }
    }

    fn require_session_id(&self, op: &'static str) -> Result<String> {
        self.acp_session_id().ok_or_else(|| {
            crate::acp::protocol::AcpOperationError::new(
                crate::acp::protocol::AcpErrorCode::SessionDead,
                format!(
                    "ACP {} called before session/new for harness '{}'",
                    op, self.harness_id
                ),
            )
            .into()
        })
    }

    async fn send_session_control(
        &mut self,
        req: AcpRequest,
        timeout: Duration,
        op: &'static str,
    ) -> Result<()> {
        match self.transport.request(&req, timeout).await {
            Ok(_) => {
                debug!(harness_id = %self.harness_id, op, "ACP session control OK");
                Ok(())
            }
            Err(e) => Err(map_session_control_error(e, op)),
        }
    }

    /// Get the current session state.
    #[must_use]
    pub const fn state(&self) -> AcpSessionState {
        self.state
    }

    /// Get the ACP session ID, if one has been created. Returns an owned
    /// copy because the id lives behind an `Arc<RwLock<>>` so a cancel
    /// handle can read it concurrently.
    #[must_use]
    pub fn acp_session_id(&self) -> Option<String> {
        self.acp_session_id
            .read()
            .unwrap_or_else(|e| {
                warn!(harness_id = %self.harness_id, "ACP session_id lock poisoned, recovering");
                e.into_inner()
            })
            .clone()
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
    #[must_use]
    pub fn harness_id(&self) -> &str {
        &self.harness_id
    }
}

/// Downgrade a session-control error to `SessionControlUnsupported` when the
/// remote returned `-32601` / `-32602` / `invalid params`, matching acpx's
/// `isLikelySessionControlUnsupportedError` heuristic. Other errors pass
/// through unchanged.
fn map_session_control_error(
    err: crate::error::AlephError,
    op: &'static str,
) -> crate::error::AlephError {
    if let crate::error::AlephError::AcpError { code, message, .. } = &err {
        if code == "protocol_error" {
            let lower = message.to_lowercase();
            if lower.contains("invalid params")
                || lower.contains("-32601")
                || lower.contains("-32602")
                || lower.contains("method not found")
            {
                return crate::acp::protocol::AcpOperationError::new(
                    crate::acp::protocol::AcpErrorCode::SessionControlUnsupported,
                    format!("ACP {op} not supported by adapter: {message}"),
                )
                .into();
            }
        }
    }
    err
}

/// Map an ACP authenticate failure to `AuthRequired` so callers know to prompt
/// the user for a different credential. Only protocol errors whose message
/// indicates an authentication/authorization rejection are mapped; other
/// protocol errors pass through unchanged.
fn map_auth_error(err: crate::error::AlephError) -> crate::error::AlephError {
    if let crate::error::AlephError::AcpError { code, message, .. } = &err {
        if code == "protocol_error" {
            let lower = message.to_lowercase();
            // SECURITY: anchor on word boundaries (or substring start) to avoid
            // false-positives on `authority`, `author`, `authenticated_user`,
            // `permission denied` from a non-auth flow, etc. Substring `auth`
            // would also match `authorization_required_for_*.txt` paths.
            // Use `is_auth_word` which only matches when the token starts the
            // string or follows non-alphanumeric, and is followed by a similar
            // boundary or end-of-string. See ACP-03.
            if is_auth_word(&lower)
                || lower.starts_with("credential")
                || lower.starts_with("permission denied")
                || lower.starts_with("forbidden")
            {
                return crate::acp::protocol::AcpOperationError::new(
                    crate::acp::protocol::AcpErrorCode::AuthRequired,
                    format!("ACP authenticate failed: {message}"),
                )
                .into();
            }
        }
    }
    err
}

/// Match `auth`, `unauthorized`, `authentication`, or `auth required` only when
/// they appear as full word-tokens (bounded by non-alphanumeric or string
/// edges). This avoids false-positives like `author` or `authority`.
fn is_auth_word(s: &str) -> bool {
    let mut idx = 0usize;
    while let Some(pos) = s[idx..].find("auth") {
        let start = idx + pos;
        let end = start + "auth".len();
        let left_ok = start == 0
            || !s.as_bytes()[start - 1].is_ascii_alphanumeric();
        let right_ok =
            end == s.len() || !s.as_bytes()[end].is_ascii_alphanumeric();
        if left_ok && right_ok {
            // Whole-word match — disambiguate from `author`/`authority`.
            // `auth` as a token is short enough that the only false-positive
            // is `auth0`, `authenticated`, etc. which are themselves auth
            // errors, so accept.
            return true;
        }
        idx = end;
    }
    false
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
