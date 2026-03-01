//! Stdio transport for bridge process communication.
//!
//! Implements [`Transport`] over stdin/stdout pipes using newline-delimited
//! JSON-RPC 2.0 messages. This transport is designed for bridge processes
//! spawned as child processes, where communication happens via the child's
//! stdin (for sending requests) and stdout (for receiving responses/events).
//!
//! # Wire format
//!
//! Same as [`super::unix_socket::UnixSocketTransport`] — each message is a
//! single JSON object terminated by `\n`:
//!
//! ```text
//! {"jsonrpc":"2.0","id":1,"method":"send_message","params":{...}}\n
//! {"jsonrpc":"2.0","id":1,"result":{...}}\n
//! {"jsonrpc":"2.0","method":"event","params":{...}}\n
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use alephcore::gateway::transport::stdio::StdioTransport;
//! use tokio::process::Command;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut child = Command::new("my-bridge")
//!     .stdin(std::process::Stdio::piped())
//!     .stdout(std::process::Stdio::piped())
//!     .spawn()?;
//!
//! let stdin = child.stdin.take().unwrap();
//! let stdout = child.stdout.take().unwrap();
//!
//! let transport = StdioTransport::from_child(stdin, stdout);
//! // After handshake succeeds:
//! transport.set_connected();
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::fmt;
use crate::sync_primitives::{AtomicBool, AtomicU64, Ordering};
use crate::sync_primitives::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, warn};

use super::traits::{BridgeEvent, Transport, TransportError};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 wire types (internal, mirrored from unix_socket)
// ---------------------------------------------------------------------------

/// Outbound JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// Inbound JSON-RPC 2.0 message (response or notification).
#[derive(Debug, Deserialize)]
struct JsonRpcMessage {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// Channel capacity
// ---------------------------------------------------------------------------

/// Default capacity for the internal event channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Type alias for the pending request map.
type PendingMap = HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>;

// ---------------------------------------------------------------------------
// StdioTransport
// ---------------------------------------------------------------------------

/// A [`Transport`] implementation that communicates with a bridge process
/// over stdin/stdout using newline-delimited JSON-RPC 2.0.
///
/// This is intended for bridge processes spawned as child processes (e.g.
/// Python or Node.js scripts) where the parent writes requests to the
/// child's stdin and reads responses/events from the child's stdout.
///
/// # Lifecycle
///
/// 1. Spawn the child process and capture its stdin/stdout.
/// 2. Create with [`StdioTransport::from_child`] or [`StdioTransport::from_streams`].
/// 3. Call [`StdioTransport::set_connected`] after the handshake succeeds.
/// 4. Use [`Transport::request`] / [`Transport::next_event`] to interact.
/// 5. Call [`Transport::close`] when done.
pub struct StdioTransport {
    /// Writer (child's stdin), protected by a mutex.
    writer: Arc<Mutex<Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>>>,
    /// Pending request map: id -> oneshot sender for the response.
    /// Shared with the background read loop via Arc.
    pending: Arc<Mutex<PendingMap>>,
    /// Monotonically increasing request id counter.
    next_id: AtomicU64,
    /// Sender side of the event channel (kept alive for the read loop).
    #[allow(dead_code)]
    event_tx: mpsc::Sender<BridgeEvent>,
    /// Receiver side of the event channel (consumed by `next_event`).
    event_rx: Mutex<Option<mpsc::Receiver<BridgeEvent>>>,
    /// Whether the transport is currently connected.
    connected: Arc<AtomicBool>,
}

impl fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdioTransport")
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .finish()
    }
}

impl StdioTransport {
    /// Create a new stdio transport from a child process's stdin and stdout.
    ///
    /// The transport starts in a **disconnected** state. Call
    /// [`set_connected`](Self::set_connected) after the bridge reports ready
    /// or the handshake completes.
    ///
    /// A background read loop is spawned immediately to process incoming
    /// messages from the child's stdout.
    pub fn from_child(
        stdin: tokio::process::ChildStdin,
        stdout: tokio::process::ChildStdout,
    ) -> Self {
        Self::from_streams(Box::new(stdin), Box::new(BufReader::new(stdout)))
    }

    /// Create a new stdio transport from generic async streams.
    ///
    /// This constructor is useful for testing (e.g. with `tokio::io::duplex`)
    /// or when the writer/reader come from a non-standard source.
    ///
    /// The transport starts in a **disconnected** state. Call
    /// [`set_connected`](Self::set_connected) after the bridge reports ready.
    pub fn from_streams(
        writer: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
        reader: Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let connected = Arc::new(AtomicBool::new(false));

        // Spawn the background read loop immediately.
        Self::spawn_read_loop(
            event_tx.clone(),
            Arc::clone(&pending),
            reader,
            Arc::clone(&connected),
        );

        Self {
            writer: Arc::new(Mutex::new(Some(writer))),
            pending,
            next_id: AtomicU64::new(1),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            connected,
        }
    }

    /// Mark the transport as connected.
    ///
    /// Call this after the bridge process signals readiness (e.g. sends a
    /// `Ready` event or completes a handshake).
    pub fn set_connected(&self) {
        self.connected.store(true, Ordering::Release);
    }

    /// Spawn a background tokio task that reads newline-delimited JSON from
    /// the reader (child's stdout), classifying each message as either:
    ///
    /// - **Response** (has `id`): resolved via the pending request map.
    /// - **Notification** (has `method`, no `id`): forwarded as a `BridgeEvent`.
    fn spawn_read_loop(
        event_tx: mpsc::Sender<BridgeEvent>,
        pending: Arc<Mutex<PendingMap>>,
        reader: Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
        connected: Arc<AtomicBool>,
    ) {
        tokio::spawn(async move {
            // reader is already AsyncBufRead — use it directly (no double-wrap).
            let mut buf_reader = reader;
            let mut line = String::new();

            loop {
                line.clear();
                match buf_reader.read_line(&mut line).await {
                    Ok(0) => {
                        // EOF: bridge process exited or closed stdout.
                        debug!("Stdio bridge EOF");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                            Ok(msg) => {
                                Self::dispatch_message(msg, &pending, &event_tx).await;
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    line = trimmed,
                                    "Failed to parse JSON-RPC message from stdio bridge"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            "Error reading from stdio bridge"
                        );
                        break;
                    }
                }
            }

            // Read loop exited (EOF or IO error) — drain all pending requests
            // so callers don't hang forever waiting on a oneshot channel.
            connected.store(false, Ordering::Release);
            let mut map = pending.lock().await;
            for (_id, sender) in map.drain() {
                let _ = sender.send(Err("Bridge disconnected".into()));
            }
        });
    }

    /// Classify and dispatch a single JSON-RPC message.
    async fn dispatch_message(
        msg: JsonRpcMessage,
        pending: &Mutex<PendingMap>,
        event_tx: &mpsc::Sender<BridgeEvent>,
    ) {
        if let Some(id) = msg.id {
            // This is a response to a pending request.
            let mut map = pending.lock().await;
            if let Some(sender) = map.remove(&id) {
                let result = if let Some(err) = msg.error {
                    Err(err.message)
                } else {
                    Ok(msg.result.unwrap_or(serde_json::Value::Null))
                };
                let _ = sender.send(result);
            } else {
                warn!(id, "Received response for unknown request id");
            }
        } else if msg.method.is_some() {
            // This is a notification (event).
            let params = msg.params.unwrap_or(serde_json::Value::Null);
            match serde_json::from_value::<BridgeEvent>(params.clone()) {
                Ok(event) => {
                    if event_tx.send(event).await.is_err() {
                        debug!("Event channel closed; dropping bridge event");
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        params = %params,
                        "Failed to parse bridge event from notification params"
                    );
                }
            }
        } else {
            warn!("Received JSON-RPC message with neither id nor method");
        }
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError> {
        if !self.is_connected() {
            return Err(TransportError::NotConnected);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params: if params.is_null() { None } else { Some(params) },
        };

        let mut payload = serde_json::to_string(&request)
            .map_err(|e| TransportError::Serialization(e.to_string()))?;
        payload.push('\n');

        // Register the pending request before sending so the read loop
        // can resolve it even if the response arrives instantly.
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        // Write to child's stdin. On failure, remove the pending entry so the
        // caller doesn't leak a oneshot that will never be resolved.
        // We drop the writer guard BEFORE acquiring pending lock on error paths
        // to avoid holding both locks simultaneously.
        let write_result = {
            let mut writer_guard = self.writer.lock().await;
            let writer = match writer_guard.as_mut() {
                Some(w) => w,
                None => {
                    drop(writer_guard);
                    self.pending.lock().await.remove(&id);
                    return Err(TransportError::NotConnected);
                }
            };
            let r1 = writer.write_all(payload.as_bytes()).await;
            if r1.is_ok() {
                writer.flush().await
            } else {
                r1
            }
        }; // writer_guard dropped here

        if let Err(e) = write_result {
            self.pending.lock().await.remove(&id);
            return Err(TransportError::Io(e));
        }

        // Wait for response with a 30-second timeout.
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(err_msg))) => Err(TransportError::RequestFailed(err_msg)),
            Ok(Err(_)) => {
                // The sender was dropped (read loop exited).
                Err(TransportError::ConnectionFailed(
                    "Connection lost while waiting for response".into(),
                ))
            }
            Err(_) => {
                // Timeout — remove the pending entry so the read loop doesn't
                // try to resolve a closed channel later.
                self.pending.lock().await.remove(&id);
                Err(TransportError::Timeout)
            }
        }
    }

    async fn next_event(&self) -> Option<BridgeEvent> {
        let mut rx_guard = self.event_rx.lock().await;
        if let Some(rx) = rx_guard.as_mut() {
            rx.recv().await
        } else {
            None
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.connected.store(false, Ordering::Release);

        // Drop the writer to close our end of the pipe.
        // This causes the child process to get EOF on its stdin, and
        // the read loop to eventually get EOF on stdout, which in turn
        // drops the event_tx clone — eventually unblocking next_event().
        {
            let mut writer_guard = self.writer.lock().await;
            *writer_guard = None;
        }

        // Drain all pending requests so callers don't hang.
        {
            let mut map = self.pending.lock().await;
            for (_id, sender) in map.drain() {
                let _ = sender.send(Err("Transport closed".into()));
            }
        }

        debug!("Stdio transport closed");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    /// Helper: create a StdioTransport from duplex pairs.
    ///
    /// Returns `(transport, server_reader, server_writer)` where:
    /// - `transport` is the StdioTransport under test
    /// - `server_reader` reads what the transport writes (requests)
    /// - `server_writer` writes responses/events that the transport reads
    ///
    /// Uses two independent duplex channels:
    /// - One for requests (transport writes -> server reads)
    /// - One for responses/events (server writes -> transport reads)
    fn make_test_transport() -> (
        StdioTransport,
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
    ) {
        // Channel for transport -> bridge (requests):
        //   transport writes to `request_tx`, server reads from `request_rx`
        let (request_tx, request_rx) = duplex(4096);
        // Channel for bridge -> transport (responses/events):
        //   server writes to `response_tx`, transport reads from `response_rx`
        let (response_tx, response_rx) = duplex(4096);

        let writer: Box<dyn tokio::io::AsyncWrite + Send + Unpin> = Box::new(request_tx);
        let reader: Box<dyn tokio::io::AsyncBufRead + Send + Unpin> =
            Box::new(BufReader::new(response_rx));

        let transport = StdioTransport::from_streams(writer, reader);
        (transport, request_rx, response_tx)
    }

    #[test]
    fn test_stdio_transport_creation() {
        // We cannot call from_streams in a non-async context because it
        // spawns a tokio task. Use a runtime.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (transport, _reader, _writer) = make_test_transport();
            assert!(!transport.is_connected());
            assert!(format!("{:?}", transport).contains("StdioTransport"));
        });
    }

    #[tokio::test]
    async fn test_stdio_transport_request_response() {
        let (transport, mut server_reader, mut server_writer) = make_test_transport();
        transport.set_connected();
        assert!(transport.is_connected());

        // Spawn a task that reads the request and sends a response.
        let handle = tokio::spawn(async move {
            let mut buf = String::new();
            let mut buf_reader = BufReader::new(&mut server_reader);
            buf_reader.read_line(&mut buf).await.unwrap();

            // Parse the request to get the id.
            let req: serde_json::Value = serde_json::from_str(buf.trim()).unwrap();
            let id = req["id"].as_u64().unwrap();
            assert_eq!(req["method"].as_str().unwrap(), "test_method");

            // Write a response.
            let response = format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"ok\":true}}}}\n",
                id
            );
            server_writer
                .write_all(response.as_bytes())
                .await
                .unwrap();
            server_writer.flush().await.unwrap();
        });

        let result = transport
            .request("test_method", serde_json::json!({"key": "value"}))
            .await;
        assert!(result.is_ok(), "request failed: {:?}", result.err());
        let value = result.unwrap();
        assert_eq!(value, serde_json::json!({"ok": true}));

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_stdio_transport_event_delivery() {
        let (transport, _server_reader, mut server_writer) = make_test_transport();
        transport.set_connected();

        // Write an event notification (no id, has method).
        let event_json = "{\"jsonrpc\":\"2.0\",\"method\":\"event\",\"params\":{\"type\":\"ready\"}}\n";
        server_writer
            .write_all(event_json.as_bytes())
            .await
            .unwrap();
        server_writer.flush().await.unwrap();

        // Give the read loop a moment to process.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let event = tokio::time::timeout(Duration::from_secs(2), transport.next_event())
            .await
            .expect("next_event timed out");

        assert!(event.is_some());
        match event.unwrap() {
            BridgeEvent::Ready => {} // expected
            other => panic!("Expected Ready event, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_close_stdio_transport() {
        let (transport, _server_reader, _server_writer) = make_test_transport();
        transport.set_connected();
        assert!(transport.is_connected());

        let result = transport.close().await;
        assert!(result.is_ok());
        assert!(!transport.is_connected());
    }

    #[tokio::test]
    async fn test_request_when_not_connected() {
        let (transport, _server_reader, _server_writer) = make_test_transport();
        // Do not call set_connected().

        let result = transport
            .request("test", serde_json::Value::Null)
            .await;
        assert!(matches!(result, Err(TransportError::NotConnected)));
    }

    #[tokio::test]
    async fn test_close_when_not_connected() {
        let (transport, _reader, _writer) = make_test_transport();
        // Closing a non-connected transport should succeed gracefully.
        let result = transport.close().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 42,
            method: "send_message".into(),
            params: Some(serde_json::json!({"to": "alice", "text": "hi"})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"method\":\"send_message\""));
        assert!(json.contains("\"params\""));
    }

    #[test]
    fn test_json_rpc_request_null_params_skipped() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "ping".into(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("params"));
    }
}
