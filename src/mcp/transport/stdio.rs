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

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::{McpTransport, NotificationCallback};

/// Default timeout for RPC calls (30 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 30;

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
    pending: std::sync::Arc<PendingMap>,
    /// Handler for server-initiated notifications (installed after connect)
    notification_handler: std::sync::Arc<StdMutex<Option<NotificationCallback>>>,
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
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        for (key, value) in env {
            cmd.env(key, value);
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AlephError::IoError(format!(
                "Failed to spawn MCP server '{}' ({}): {}",
                name, command_str, e
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
            AlephError::IoError(format!("MCP server '{}' stdin not available", name))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AlephError::IoError(format!("MCP server '{}' stdout not available", name))
        })?;

        let pending: std::sync::Arc<PendingMap> =
            std::sync::Arc::new(StdMutex::new(HashMap::new()));
        let notification_handler: std::sync::Arc<StdMutex<Option<NotificationCallback>>> =
            std::sync::Arc::new(StdMutex::new(None));

        let reader_task = tokio::spawn(reader_loop(
            stdout,
            name.clone(),
            std::sync::Arc::clone(&pending),
            std::sync::Arc::clone(&notification_handler),
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
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
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
            .map_err(|e| AlephError::IoError(format!("Failed to serialize request: {}", e)))?;

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
            .map_err(|e| AlephError::IoError(format!("Failed to serialize notification: {}", e)))?;
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

    /// Close the transport and terminate the server process
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
fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
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
    let has_id = value.get("id").map(|v| !v.is_null()).unwrap_or(false);
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
    pending: std::sync::Arc<PendingMap>,
    notification_handler: std::sync::Arc<StdMutex<Option<NotificationCallback>>>,
) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                tracing::debug!(server = %server_name, "MCP server closed stdout (EOF)");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(server = %server_name, error = %e, "MCP stdio read error");
                break;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        dispatch_line(&server_name, trimmed, &pending, &notification_handler);
    }

    // The reader is gone: fail every in-flight request so callers stop waiting.
    // Dropping each sender resolves its receiver to a `RecvError`.
    lock(&pending).clear();
    tracing::debug!(server = %server_name, "MCP stdio reader task exited");
}

/// Implementation of the McpTransport trait for StdioTransport
///
/// This adapts the existing StdioTransport methods to the unified transport interface,
/// enabling transport-agnostic connection management in the MCP client.
#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.send(request).await
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        StdioTransport::send_notification(self, notification).await
    }

    async fn is_alive(&self) -> bool {
        self.is_running().await
    }

    async fn close(&self) -> Result<()> {
        StdioTransport::close(self).await
    }

    fn server_name(&self) -> &str {
        self.name()
    }

    fn set_notification_handler(&self, handler: NotificationCallback) {
        self.install_notification_handler(handler);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Stop the reader task explicitly; the child process is killed
        // separately via `kill_on_drop(true)`.
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
}
