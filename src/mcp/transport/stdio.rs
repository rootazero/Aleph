//! Stdio Transport for External MCP Servers
//!
//! Communicates with MCP servers via subprocess stdin/stdout using JSON-RPC.
//!
//! # I/O model
//!
//! A background reader task owns the child's stdout and continuously
//! demultiplexes every line the server emits:
//!
//! - **Responses** (a line with `id`, no `method`) are routed to the matching
//!   in-flight request via a per-id [`oneshot`] channel.
//! - **Notifications** (a line with `method`, no `id`) are delivered to the
//!   handler installed via `set_notification_handler`, which is how
//!   `tools/list_changed` and friends reach the manager.
//!
//! This means a request timeout no longer corrupts the transport: the reader
//! keeps draining stdout regardless, so later requests still resolve.

use crate::sync_primitives::{Arc, Mutex as StdMutex};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::utils::no_window::NoWindow;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::{McpTransport, NotificationCallback};

/// Default timeout for RPC calls (30 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum bytes per JSON-RPC frame read from a child MCP server's stdout.
///
/// Defends against a malicious or compromised MCP server that emits a single
/// unbounded line without a newline terminator. Without this cap, the
/// underlying `read_until(b'\n', ...)` would keep appending bytes into the
/// reader buffer until the host process is OOM-killed by the kernel — a single
/// compromised child could then take down the entire Aleph daemon. 8 MiB is
/// well above any legitimate `tools/call` payload (most responses are <1 MiB;
/// embedded image/audio blobs are bounded elsewhere by the same-kind
/// media/processor OOM pre-checks) while bounding worst-case per-reader
/// memory at ~8 MiB per connected server.
///
/// Stderr uses the same cap so a server that writes unbounded diagnostic spam
/// to stderr cannot OOM the daemon either.
const MAX_MCP_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Environment-variable keys that change how an interpreter or dynamic loader
/// bootstraps a process. A third-party MCP server's `env` block must never set
/// these against the child we spawn: they enable code-prelude injection,
/// arbitrary library preloading, and debugger/inspector attachment. We strip
/// them at spawn time so a malicious or careless server config cannot escalate
/// from "run this command" to "run this command with my prelude/loader hooks".
/// (Ported from openclaw's stdio MCP env hardening.)
const UNSAFE_ENV_KEYS: &[&str] = &[
    // Dynamic-loader hijacking (glibc / macOS dyld)
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    // Interpreter prelude / module injection
    "NODE_OPTIONS",
    "NODE_REPL_EXTERNAL_MODULE",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "PYTHONINSPECT",
    "PERL5OPT",
    "PERL5LIB",
    "RUBYOPT",
    "RUBYLIB",
    // Shell startup / tracing hooks
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "PS4",
    "GIT_EXTERNAL_DIFF",
    // JVM / Java agent injection
    "JAVA_TOOL_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "JAVA_OPTS",
    "JVM_OPTS",
];

/// Returns `true` if `key` is an interpreter/loader bootstrap variable that
/// must not be forwarded to a spawned MCP server process. Case-insensitive to
/// match OS env-var lookup semantics on case-insensitive platforms.
fn is_unsafe_env_key(key: &str) -> bool {
    UNSAFE_ENV_KEYS
        .iter()
        .any(|denied| denied.eq_ignore_ascii_case(key))
}

/// Map of in-flight request ids to the channel awaiting their response.
type PendingMap = StdMutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>;

/// Stdio transport for communicating with MCP servers via subprocess
pub struct StdioTransport {
    /// Server name for logging
    server_name: String,
    /// Request timeout
    timeout: Duration,
    /// Child process handle — kept for liveness checks and termination
    child: Mutex<Child>,
    /// Server stdin — writes are serialized through this lock
    stdin: Mutex<ChildStdin>,
    /// In-flight requests awaiting a response, keyed by JSON-RPC id
    pending: Arc<PendingMap>,
    /// Handler for server-initiated notifications (installed after connect)
    notification_handler: Arc<StdMutex<Option<NotificationCallback>>>,
    /// Background stdout reader; aborted on drop
    reader_task: JoinHandle<()>,
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
            // Capture stderr instead of discarding it: a server that fails to
            // start or errors writes its only diagnostics here. A drain task
            // (below) surfaces them through tracing.
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        for (key, value) in env {
            if is_unsafe_env_key(key) {
                tracing::warn!(
                    server = %name,
                    key = %key,
                    "Refusing to forward unsafe interpreter/loader env var to MCP server"
                );
                continue;
            }
            cmd.env(key, value);
        }

        // Strip inherited interpreter/loader hijack vars (LD_PRELOAD,
        // NODE_OPTIONS, PYTHONSTARTUP, BASH_ENV, etc.) regardless of who set
        // them. The forward-pass above only filters keys the operator
        // explicitly supplied — an attacker who can influence the Aleph
        // daemon's environment (development shell, shared host, supply-chain
        // compromise) would otherwise have their loader fire inside every
        // spawned MCP server subprocess.
        //
        // Single pass: walk `std::env::vars()` once and apply both
        // predicates (`is_secret_env` for inherited-secret stripping,
        // `is_unsafe_env_key` for case-insensitive interpreter/loader
        // hijack defense). Replaces the prior two-pass scheme which
        // re-iterated `std::env::vars()` and ran an O(m) per-row
        // `stripped_unsafe.iter().any(...)` check inside the second pass.
        let mut stripped_secrets: usize = 0;
        for (var_name, _) in std::env::vars() {
            let is_secret = crate::security::secret_env::is_secret_env(&var_name);
            let is_unsafe = is_unsafe_env_key(&var_name);
            if is_secret {
                cmd.env_remove(&var_name);
                stripped_secrets += 1;
            }
            if is_unsafe {
                cmd.env_remove(&var_name);
            }
        }

        // Also strip the canonical-unsafe keys in their declared spelling so
        // the breadcrumb below reports the canonical name even when the
        // inherited spelling differs in case (`Ld_Preload` vs
        // `LD_PRELOAD`). No-op when the canonical key is already gone.
        let mut stripped_unsafe: Vec<&'static str> = Vec::new();
        for var_name in UNSAFE_ENV_KEYS {
            if std::env::var(var_name).is_ok() {
                cmd.env_remove(var_name);
                stripped_unsafe.push(*var_name);
            }
        }

        if stripped_secrets > 0 || !stripped_unsafe.is_empty() {
            tracing::debug!(
                server = %name,
                stripped_secrets,
                stripped_unsafe = ?stripped_unsafe,
                "stripped inherited env vars before spawning MCP server"
            );
        }

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd.no_window().spawn().map_err(|e| {
            AlephError::IoError(format!(
                "Failed to spawn MCP server '{name}' ({command_str}): {e}"
            ))
        })?;

        tracing::info!(
            server = %name,
            pid = ?child.id(),
            "MCP server process started"
        );

        // Take the pipes out of the child: stdin is owned by this transport
        // for the lifetime of the connection, stdout is owned by the reader.
        let stdin = child.stdin.take().ok_or_else(|| {
            AlephError::IoError(format!("MCP server '{name}' stdin not available"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AlephError::IoError(format!("MCP server '{name}' stdout not available"))
        })?;

        // Drain stderr in the background so server diagnostics are not lost.
        // Non-essential: if the pipe is somehow unavailable we simply skip it.
        // The task ends at EOF, which arrives when the child exits
        // (`kill_on_drop` guarantees that on transport drop), so it cannot leak.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(stderr_loop(stderr, name.clone()));
        }

        let pending: Arc<PendingMap> = Arc::new(StdMutex::new(HashMap::new()));
        let notification_handler: Arc<StdMutex<Option<NotificationCallback>>> =
            Arc::new(StdMutex::new(None));

        let reader_task = tokio::spawn(reader_loop(
            stdout,
            name.clone(),
            Arc::clone(&pending),
            Arc::clone(&notification_handler),
        ));

        Ok(Self {
            server_name: name,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            notification_handler,
            reader_task,
        })
    }

    /// Set the request timeout
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Send a JSON-RPC request and wait for the matching response
    ///
    /// The response is demultiplexed by the background reader task, so
    /// concurrent requests on the same transport are safe and a timeout on
    /// one request leaves the transport usable for the rest.
    pub async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let method = &request.method;

        tracing::debug!(
            server = %self.server_name,
            id = request.id,
            method = %method,
            "Sending JSON-RPC request"
        );

        let request_json = request
            .to_json_line()
            .map_err(|e| AlephError::IoError(format!("Failed to serialize request: {e}")))?;

        // Register the response slot before writing so a fast server cannot
        // answer before the reader knows where to route the response.
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(request.id, tx);

        if let Err(e) = self.write_line(&request_json).await {
            lock(&self.pending).remove(&request.id);
            return Err(e);
        }

        match timeout(self.timeout, rx).await {
            Ok(Ok(response)) => {
                tracing::debug!(
                    server = %self.server_name,
                    id = ?response.id,
                    success = response.is_success(),
                    "Received JSON-RPC response"
                );
                Ok(response)
            }
            Ok(Err(_)) => {
                // The reader task dropped our sender: stdout reached EOF.
                Err(AlephError::IoError(format!(
                    "MCP server '{}' closed the connection before responding",
                    self.server_name
                )))
            }
            Err(_) => {
                lock(&self.pending).remove(&request.id);
                tracing::warn!(
                    server = %self.server_name,
                    method = %method,
                    timeout_secs = self.timeout.as_secs(),
                    "MCP request timed out"
                );
                Err(AlephError::McpTimeout)
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected)
    pub async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        tracing::debug!(
            server = %self.server_name,
            method = %notification.method,
            "Sending JSON-RPC notification"
        );

        let json = notification
            .to_json_line()
            .map_err(|e| AlephError::IoError(format!("Failed to serialize notification: {e}")))?;
        self.write_line(&json).await
    }

    /// Write a single pre-serialized JSON line to the server's stdin.
    async fn write_line(&self, line: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| {
            AlephError::IoError(format!(
                "Failed to write to MCP server '{}': {}",
                self.server_name, e
            ))
        })?;
        stdin.flush().await.map_err(|e| {
            AlephError::IoError(format!(
                "Failed to flush MCP server '{}' stdin: {}",
                self.server_name, e
            ))
        })?;
        Ok(())
    }

    /// Install a handler for server-initiated notifications.
    pub fn install_notification_handler(&self, handler: NotificationCallback) {
        *lock(&self.notification_handler) = Some(handler);
    }

    /// Close the transport and terminate the server process.
    ///
    /// Drains the reader task before aborting it: an in-flight `tools/call`
    /// whose response was already in the child's stdout pipe would otherwise
    /// be abandoned mid-line, and the next caller would never see the
    /// `McpTimeout` that should have surfaced. The drain is bounded by a
    /// 100 ms grace window — if the child does not close stdout by then
    /// (signalled by EOF on the read), the reader is aborted regardless.
    pub async fn close(&self) -> Result<()> {
        let mut child = self.child.lock().await;

        tracing::info!(
            server = %self.server_name,
            pid = ?child.id(),
            "Terminating MCP server"
        );

        if let Err(e) = child.kill().await {
            tracing::warn!(
                server = %self.server_name,
                error = %e,
                "Failed to kill MCP server process"
            );
        }

        // Mark the reader as aborted; the Drop impl drives the actual
        // cancellation. The 100 ms grace here is to let the child's
        // stdout pipe close (which `kill` triggers), so the reader's
        // next `read_line` returns EOF and the task exits cleanly on its
        // own without an explicit abort.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

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

/// Acquire a [`StdMutex`], recovering the guard if a previous holder panicked.
fn lock<T>(mutex: &StdMutex<T>) -> crate::sync_primitives::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A single JSON-RPC message read from a server's stdout.
enum ServerMessage {
    /// A response to one of our requests.
    Response(JsonRpcResponse),
    /// A server-initiated notification.
    Notification(JsonRpcNotification),
    /// A server-initiated request — unsupported on the stdio transport.
    ServerRequest,
    /// Valid JSON but not a recognizable JSON-RPC message.
    Malformed,
}

/// Classify one stdout line into a [`ServerMessage`].
///
/// Per JSON-RPC 2.0: responses carry `id` and no `method`, notifications carry
/// `method` and no `id`, and a message with both is a server-initiated request.
fn classify_line(line: &str) -> std::result::Result<ServerMessage, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(line)?;
    let has_id = value.get("id").is_some_and(|v| !v.is_null());
    let has_method = value.get("method").is_some();
    match (has_id, has_method) {
        (true, false) => Ok(ServerMessage::Response(serde_json::from_value(value)?)),
        (false, true) => Ok(ServerMessage::Notification(serde_json::from_value(value)?)),
        (true, true) => Ok(ServerMessage::ServerRequest),
        (false, false) => Ok(ServerMessage::Malformed),
    }
}

/// Route one classified line to the pending request or the notification handler.
fn dispatch_line(
    server_name: &str,
    line: &str,
    pending: &PendingMap,
    notification_handler: &StdMutex<Option<NotificationCallback>>,
) {
    match classify_line(line) {
        Ok(ServerMessage::Response(response)) => {
            let Some(id) = response.id else {
                tracing::warn!(server = %server_name, "response without id, dropping");
                return;
            };
            match lock(pending).remove(&id) {
                Some(tx) => {
                    let _ = tx.send(response);
                }
                None => tracing::warn!(
                    server = %server_name,
                    id,
                    "response for unknown or already-timed-out request id"
                ),
            }
        }
        Ok(ServerMessage::Notification(notification)) => {
            let guard = lock(notification_handler);
            match guard.as_ref() {
                Some(handler) => handler(notification),
                None => tracing::debug!(
                    server = %server_name,
                    method = %notification.method,
                    "notification dropped (no handler installed)"
                ),
            }
        }
        Ok(ServerMessage::ServerRequest) => tracing::warn!(
            server = %server_name,
            "ignoring server-initiated request (unsupported on stdio transport)"
        ),
        Ok(ServerMessage::Malformed) => tracing::warn!(
            server = %server_name,
            raw = %line,
            "malformed JSON-RPC message from server"
        ),
        Err(e) => tracing::warn!(
            server = %server_name,
            error = %e,
            raw = %line,
            "failed to parse line from server"
        ),
    }
}

/// Background task: drain the server's stdout and demultiplex every line.
async fn reader_loop(
    stdout: ChildStdout,
    server_name: String,
    pending: Arc<PendingMap>,
    notification_handler: Arc<StdMutex<Option<NotificationCallback>>>,
) {
    reader_loop_with_cap(
        stdout,
        server_name,
        pending,
        notification_handler,
        MAX_MCP_FRAME_BYTES,
    )
    .await
}

/// Inner reader loop with a configurable per-frame byte cap.
///
/// Extracted so tests can drive the loop with a smaller cap (and thus a
/// smaller attack payload) without changing the production wiring. The cap
/// protects against an unbounded `read_until` filling the buffer until the
/// host process is OOM-killed — see [`MAX_MCP_FRAME_BYTES`].
async fn reader_loop_with_cap(
    stdout: ChildStdout,
    server_name: String,
    pending: Arc<PendingMap>,
    notification_handler: Arc<StdMutex<Option<NotificationCallback>>>,
    max_frame_bytes: usize,
) {
    let mut reader = BufReader::new(stdout);
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => {
                tracing::debug!(server = %server_name, "MCP server closed stdout (EOF)");
                break;
            }
            Ok(n) => {
                if n > max_frame_bytes {
                    // A single frame (no newline within `max_frame_bytes`) would
                    // otherwise grow `buf` without limit. Abort the reader: the
                    // `pending` map is cleared below so every in-flight caller
                    // receives a clean error instead of hanging.
                    tracing::error!(
                        server = %server_name,
                        bytes = n,
                        max = max_frame_bytes,
                        "MCP stdout frame exceeded cap; aborting reader to prevent OOM"
                    );
                    break;
                }
                let line = match std::str::from_utf8(&buf) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            bytes = n,
                            "MCP server emitted non-UTF8 frame; dropping"
                        );
                        continue;
                    }
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                dispatch_line(&server_name, trimmed, &pending, &notification_handler);
            }
            Err(e) => {
                tracing::warn!(server = %server_name, error = %e, "MCP stdio read error");
                break;
            }
        }
    }

    // The reader is gone: fail every in-flight request so callers stop waiting.
    // Dropping each sender resolves its receiver to a `RecvError`.
    lock(&pending).clear();
    tracing::debug!(server = %server_name, "MCP stdio reader task exited");
}

/// Drain an MCP server's stderr, surfacing each line through `tracing`.
///
/// The subprocess's stderr was previously discarded (`Stdio::null()`), so a
/// server that failed to start or errored left no diagnostic trail — only a
/// generic connection failure reached the operator. Routing it through
/// `tracing` (rather than a flat shared log file, as the reference does) keeps
/// per-server context and lets the existing log filters control verbosity.
/// The task ends at EOF, which arrives when the child closes stderr on exit.
///
/// Same per-line byte cap as stdout (see [`MAX_MCP_FRAME_BYTES`]) so a server
/// that writes unbounded diagnostic spam to stderr cannot OOM the daemon
/// either.
async fn stderr_loop(stderr: ChildStderr, server_name: String) {
    let mut reader = BufReader::new(stderr);
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break, // EOF — child closed stderr
            Ok(n) => {
                if n > MAX_MCP_FRAME_BYTES {
                    tracing::warn!(
                        server = %server_name,
                        bytes = n,
                        max = MAX_MCP_FRAME_BYTES,
                        "MCP stderr line exceeded cap; aborting stderr drain"
                    );
                    break;
                }
                match std::str::from_utf8(&buf) {
                    Ok(s) => {
                        let trimmed = s.trim_end();
                        if !trimmed.is_empty() {
                            tracing::debug!(
                                server = %server_name,
                                "mcp server stderr: {}",
                                trimmed
                            );
                        }
                    }
                    Err(_) => {
                        // stderr is free-form text — don't fight the server
                        // over encoding. The cap above still bounds memory.
                    }
                }
            }
            Err(_) => break, // pipe error; nothing actionable, stop draining
        }
    }
}

/// Implementation of the `McpTransport` trait for `StdioTransport`
///
/// This adapts the existing `StdioTransport` methods to the unified transport interface,
/// enabling transport-agnostic connection management in the MCP client.
#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.send(request).await
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        Self::send_notification(self, notification).await
    }

    async fn is_alive(&self) -> bool {
        self.is_running().await
    }

    async fn close(&self) -> Result<()> {
        Self::close(self).await
    }

    fn server_name(&self) -> &str {
        self.name()
    }

    fn set_notification_handler(&self, handler: NotificationCallback) {
        self.install_notification_handler(handler);
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Stop the reader task explicitly; the child process is killed
        // separately via `kill_on_drop(true)`. The proper drain happens in
        // `close()`; the Drop fires only when the Arc count hits zero (after
        // the manager's `stop_server_internal` has already awaited close),
        // so the abort here is the second-best fallthrough.
        self.reader_task.abort();
        tracing::debug!(
            server = %self.server_name,
            "StdioTransport dropped, server will be terminated"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::{AtomicBool, Ordering};

    #[test]
    fn unsafe_env_keys_are_rejected() {
        // Loader + interpreter bootstrap vars must be filtered.
        assert!(is_unsafe_env_key("LD_PRELOAD"));
        assert!(is_unsafe_env_key("DYLD_INSERT_LIBRARIES"));
        assert!(is_unsafe_env_key("NODE_OPTIONS"));
        assert!(is_unsafe_env_key("PYTHONPATH"));
        assert!(is_unsafe_env_key("BASH_ENV"));
        // Case-insensitive match (OS env lookup semantics).
        assert!(is_unsafe_env_key("node_options"));
        assert!(is_unsafe_env_key("Ld_Preload"));
    }

    #[test]
    fn ordinary_env_keys_are_allowed() {
        assert!(!is_unsafe_env_key("PATH"));
        assert!(!is_unsafe_env_key("HOME"));
        assert!(!is_unsafe_env_key("GITHUB_TOKEN"));
        assert!(!is_unsafe_env_key("MY_SERVER_API_KEY"));
        assert!(!is_unsafe_env_key(""));
    }

    #[tokio::test]
    async fn test_spawn_echo_server() {
        // `cat` echoes stdin to stdout — exercises process spawning only.
        let transport = StdioTransport::spawn("test-echo", "cat", &[], &HashMap::new(), None).await;

        assert!(transport.is_ok());
        let transport = transport.unwrap();
        assert!(transport.is_running().await);

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
        let transport = StdioTransport::spawn("test-timeout", "cat", &[], &HashMap::new(), None)
            .await
            .unwrap();

        let transport = transport.with_timeout(Duration::from_secs(5));
        assert_eq!(transport.timeout, Duration::from_secs(5));

        transport.close().await.unwrap();
    }

    /// Test that StdioTransport correctly implements the McpTransport trait
    #[tokio::test]
    async fn test_stdio_implements_mcp_transport() {
        let transport = StdioTransport::spawn("test", "cat", &[], &HashMap::new(), None)
            .await
            .unwrap();

        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "test");

        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    /// Test that StdioTransport can be used as a trait object (dyn McpTransport)
    #[tokio::test]
    async fn test_stdio_as_trait_object() {
        use crate::sync_primitives::Arc;

        let transport: Arc<dyn McpTransport> = Arc::new(
            StdioTransport::spawn("dyn-test", "cat", &[], &HashMap::new(), None)
                .await
                .unwrap(),
        );

        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "dyn-test");

        transport.close().await.unwrap();
    }

    #[test]
    fn classify_response_line() {
        let line = r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#;
        assert!(matches!(
            classify_line(line).unwrap(),
            ServerMessage::Response(_)
        ));
    }

    #[test]
    fn classify_notification_line() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;
        match classify_line(line).unwrap() {
            ServerMessage::Notification(n) => {
                assert_eq!(n.method, "notifications/tools/list_changed")
            }
            _ => panic!("expected notification"),
        }
    }

    #[test]
    fn classify_server_request_line() {
        // A line with both id and method is a server-initiated request.
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"sampling/createMessage"}"#;
        assert!(matches!(
            classify_line(line).unwrap(),
            ServerMessage::ServerRequest
        ));
    }

    #[test]
    fn classify_null_id_is_not_a_response() {
        let line = r#"{"jsonrpc":"2.0","id":null,"result":{}}"#;
        assert!(matches!(
            classify_line(line).unwrap(),
            ServerMessage::Malformed
        ));
    }

    /// A scripted server that emits a response then a notification for every
    /// input line exercises the full demultiplexing reader path.
    #[tokio::test]
    async fn test_reader_routes_response_and_notification() {
        let script = "while read l; do \
             echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}'; \
             echo '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}'; \
             done";
        let transport = StdioTransport::spawn(
            "scripted",
            "sh",
            &["-c".to_string(), script.to_string()],
            &HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let got_notification = crate::sync_primitives::Arc::new(AtomicBool::new(false));
        let flag = crate::sync_primitives::Arc::clone(&got_notification);
        transport.install_notification_handler(Box::new(move |n: JsonRpcNotification| {
            if n.method == "notifications/tools/list_changed" {
                flag.store(true, Ordering::SeqCst);
            }
        }));

        let response = transport
            .send(&JsonRpcRequest::new(1, "tools/list"))
            .await
            .unwrap();
        assert!(response.is_success());

        // The notification is read just after the response; poll briefly.
        for _ in 0..100 {
            if got_notification.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(got_notification.load(Ordering::SeqCst));

        transport.close().await.unwrap();
    }

    /// `cat` echoes the request back unchanged. The echoed line carries both
    /// `id` and `method`, so it is a server-request, never a response — the
    /// request must time out with `McpTimeout` and leave the transport intact.
    #[tokio::test]
    async fn test_request_timeout_returns_mcp_timeout() {
        let transport = StdioTransport::spawn("timeout-srv", "cat", &[], &HashMap::new(), None)
            .await
            .unwrap()
            .with_timeout(Duration::from_millis(300));

        let err = transport
            .send(&JsonRpcRequest::new(42, "tools/list"))
            .await
            .unwrap_err();
        assert!(matches!(err, AlephError::McpTimeout));

        // A timeout must not poison the transport: the process is still alive.
        assert!(transport.is_running().await);

        transport.close().await.unwrap();
    }

    /// Render a native path for interpolation into a POSIX `sh` script.
    ///
    /// The path lands in shell *source text*, which is hostile to native paths
    /// in two independent ways:
    ///
    /// - `sh` consumes `\` as an escape, so a Windows path arrives at the child
    ///   as `C:UserszouAppDataLocalTemp.tmpXXXX` — a *relative* name. The
    ///   redirect then writes into the child's cwd (the repo root) while the
    ///   test waits on a `NamedTempFile` that never grows.
    /// - An unquoted space splits the redirect target, which breaks any host
    ///   whose temp path contains one (`C:\Users\First Last\...`).
    ///
    /// Slash-separate first, then single-quote.
    fn sh_path(path: &std::path::Path) -> String {
        let slashed = path.to_string_lossy().replace('\\', "/");
        format!("'{}'", slashed.replace('\'', r"'\''"))
    }

    #[test]
    fn sh_path_survives_separators_and_spaces() {
        assert_eq!(
            sh_path(std::path::Path::new(
                r"C:\Users\zou\AppData\Local\Temp\.tmpAb"
            )),
            "'C:/Users/zou/AppData/Local/Temp/.tmpAb'"
        );
        assert_eq!(
            sh_path(std::path::Path::new("/tmp/first last/report")),
            "'/tmp/first last/report'"
        );
    }

    /// Inherited secret-bearing env vars must not reach the spawned child —
    /// same rule `PlaywrightCliDriver` already enforces. The child reports its
    /// view via a temp file (stdout is consumed by the JSON-RPC reader loop,
    /// so a side-channel file is the only way for the test to observe what
    /// the process actually saw).
    #[tokio::test]
    #[serial_test::serial]
    async fn test_spawn_strips_inherited_secret_env() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = sh_path(tmp.path());

        std::env::set_var("ALEPH_TEST_STDIO_API_KEY", "topsecret_value");

        let script = format!(
            "if [ \"${{ALEPH_TEST_STDIO_API_KEY:-}}\" = \"topsecret_value\" ]; then \
             printf LEAKED > {path}; \
             else printf STRIPPED > {path}; \
             fi"
        );

        let transport = StdioTransport::spawn(
            "test-strip",
            "sh",
            &["-c".to_string(), script],
            &HashMap::new(),
            None,
        )
        .await
        .expect("spawn");

        for _ in 0..50 {
            if let Ok(meta) = tokio::fs::metadata(tmp.path()).await {
                if meta.len() > 0 {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let _ = transport.close().await;
        std::env::remove_var("ALEPH_TEST_STDIO_API_KEY");

        let contents = tokio::fs::read_to_string(tmp.path())
            .await
            .expect("read report");
        assert_eq!(
            contents.trim(),
            "STRIPPED",
            "inherited secret env must be stripped; got: {contents}"
        );
        assert!(!contents.contains("topsecret_value"));
    }

    /// A frame larger than the reader's per-line byte cap must abort the
    /// reader instead of letting `read_line` grow the buffer without limit.
    /// Without this cap, a single malicious MCP server could OOM the entire
    /// Aleph daemon by streaming bytes without a newline terminator. The test
    /// uses a small cap (64 KiB) and a 256 KiB payload to keep runtime +
    /// memory bounded while still crossing the cap.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_reader_aborts_on_oversized_stdout_frame() {
        let payload_size: usize = 256 * 1024;
        let cap: usize = 64 * 1024;
        // `head -c N /dev/zero | tr '\0' 'x'` emits N bytes of 'x' with no
        // newline, then closes stdout (EOF). `read_until(b'\n', ...)` will
        // keep filling the buffer past `cap`, at which point the cap check
        // must abort the reader.
        let script = format!("head -c {payload_size} /dev/zero | tr '\\0' 'x'");

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .no_window()
            .spawn()
            .expect("spawn sh");
        let stdout = child.stdout.take().expect("stdout pipe");

        let pending: Arc<PendingMap> = Arc::new(StdMutex::new(HashMap::new()));
        let notification_handler: Arc<StdMutex<Option<NotificationCallback>>> =
            Arc::new(StdMutex::new(None));

        let start = std::time::Instant::now();
        reader_loop_with_cap(
            stdout,
            "oversized-test".to_string(),
            Arc::clone(&pending),
            Arc::clone(&notification_handler),
            cap,
        )
        .await;
        let elapsed = start.elapsed();

        // Reader must abort promptly (well under the time it would take to
        // read 256 KiB on a healthy box if the cap were missing) and the
        // pending map must be cleared so no in-flight caller hangs.
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "reader did not abort on oversized frame in 5s; took {elapsed:?}"
        );
        assert!(lock(&pending).is_empty());

        let _ = child.wait().await;
    }

    /// Non-secret env vars inherited from the parent must remain visible to
    /// the spawned child — only secret-bearing keys are stripped.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_spawn_preserves_non_secret_inherited_env() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = sh_path(tmp.path());

        std::env::set_var("ALEPH_TEST_STDIO_PASSTHROUGH_ABC", "passthrough_value");

        let script =
            format!("printf '%s' \"${{ALEPH_TEST_STDIO_PASSTHROUGH_ABC-(unset)}}\" > {path}");

        let transport = StdioTransport::spawn(
            "test-passthrough",
            "sh",
            &["-c".to_string(), script],
            &HashMap::new(),
            None,
        )
        .await
        .expect("spawn");

        for _ in 0..50 {
            if let Ok(meta) = tokio::fs::metadata(tmp.path()).await {
                if meta.len() > 0 {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let _ = transport.close().await;
        std::env::remove_var("ALEPH_TEST_STDIO_PASSTHROUGH_ABC");

        let contents = tokio::fs::read_to_string(tmp.path())
            .await
            .expect("read report");
        assert_eq!(
            contents, "passthrough_value",
            "non-secret inherited env must pass through; got: {contents}"
        );
    }
}
