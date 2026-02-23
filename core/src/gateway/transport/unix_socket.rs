//! Unix domain socket transport for bridge process communication.
//!
//! Implements [`Transport`] over a Unix domain socket using newline-delimited
//! JSON-RPC 2.0 messages. The transport connects to an already-listening
//! bridge process, sends requests, and receives both responses (correlated
//! by request id) and unsolicited event notifications.
//!
//! # Wire format
//!
//! Each message is a single JSON object terminated by `\n`:
//!
//! ```text
//! {"jsonrpc":"2.0","id":1,"method":"send_message","params":{...}}\n
//! {"jsonrpc":"2.0","id":1,"result":{...}}\n
//! {"jsonrpc":"2.0","method":"event","params":{...}}\n
//! ```

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, warn};

use super::traits::{BridgeEvent, Transport, TransportError};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 wire types (internal)
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
// UnixSocketTransport
// ---------------------------------------------------------------------------

/// A [`Transport`] implementation that communicates with a bridge process
/// over a Unix domain socket using newline-delimited JSON-RPC 2.0.
///
/// # Lifecycle
///
/// 1. Create with [`UnixSocketTransport::new`] (not yet connected).
/// 2. Call [`UnixSocketTransport::connect`] to establish the connection.
/// 3. Use [`Transport::request`] / [`Transport::next_event`] to interact.
/// 4. Call [`Transport::close`] when done.
pub struct UnixSocketTransport {
    /// Path to the Unix domain socket file.
    socket_path: PathBuf,
    /// Writer half of the connected stream, protected by a mutex.
    writer: Arc<Mutex<Option<WriteHalf<UnixStream>>>>,
    /// Pending request map: id -> oneshot sender for the response.
    /// Shared with the background read loop via Arc.
    pending: Arc<Mutex<PendingMap>>,
    /// Monotonically increasing request id counter.
    next_id: AtomicU64,
    /// Sender side of the event channel (used by the read loop).
    event_tx: mpsc::Sender<BridgeEvent>,
    /// Receiver side of the event channel (consumed by `next_event`).
    event_rx: Mutex<Option<mpsc::Receiver<BridgeEvent>>>,
    /// Whether the transport is currently connected.
    connected: Arc<AtomicBool>,
}

impl fmt::Debug for UnixSocketTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixSocketTransport")
            .field("socket_path", &self.socket_path)
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .finish()
    }
}

impl UnixSocketTransport {
    /// Create a new transport targeting the given socket path.
    ///
    /// The transport is **not** connected after creation; call
    /// [`connect`](Self::connect) to establish the connection.
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            writer: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Connect to the Unix domain socket with retry logic.
    ///
    /// Spawns a background read loop that dispatches incoming JSON-RPC
    /// responses to their pending request channels and bridge event
    /// notifications to the event channel.
    ///
    /// # Arguments
    ///
    /// * `max_retries` - Maximum number of connection attempts (0 = try once).
    /// * `retry_delay_ms` - Delay between retries in milliseconds.
    pub async fn connect(
        &self,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> Result<(), TransportError> {
        let mut last_err = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                debug!(
                    attempt,
                    max_retries,
                    path = %self.socket_path.display(),
                    "Retrying connection to bridge socket"
                );
                tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
            }

            match UnixStream::connect(&self.socket_path).await {
                Ok(stream) => {
                    let (reader, writer) = tokio::io::split(stream);
                    *self.writer.lock().await = Some(writer);
                    self.connected.store(true, Ordering::Release);

                    // Spawn the background read loop with Arc-cloned handles.
                    Self::spawn_read_loop(
                        self.event_tx.clone(),
                        Arc::clone(&self.pending),
                        reader,
                        Arc::clone(&self.connected),
                        self.socket_path.clone(),
                    );

                    debug!(
                        path = %self.socket_path.display(),
                        "Connected to bridge socket"
                    );
                    return Ok(());
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        let err = last_err.unwrap();
        Err(TransportError::ConnectionFailed(format!(
            "Failed to connect to {} after {} attempts: {}",
            self.socket_path.display(),
            max_retries + 1,
            err,
        )))
    }

    /// Spawn a background tokio task that reads newline-delimited JSON from
    /// the socket, classifying each message as either:
    ///
    /// - **Response** (has `id`): resolved via the pending request map.
    /// - **Notification** (has `method`, no `id`): forwarded as a `BridgeEvent`.
    fn spawn_read_loop(
        event_tx: mpsc::Sender<BridgeEvent>,
        pending: Arc<Mutex<PendingMap>>,
        reader: tokio::io::ReadHalf<UnixStream>,
        connected: Arc<AtomicBool>,
        socket_path: PathBuf,
    ) {
        tokio::spawn(async move {
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();

            loop {
                line.clear();
                match buf_reader.read_line(&mut line).await {
                    Ok(0) => {
                        // EOF: bridge disconnected.
                        debug!(path = %socket_path.display(), "Bridge socket EOF");
                        connected.store(false, Ordering::Release);
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
                                    "Failed to parse JSON-RPC message from bridge"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            path = %socket_path.display(),
                            "Error reading from bridge socket"
                        );
                        connected.store(false, Ordering::Release);
                        break;
                    }
                }
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
impl Transport for UnixSocketTransport {
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

        // Write to socket.
        {
            let mut writer_guard = self.writer.lock().await;
            let writer = writer_guard
                .as_mut()
                .ok_or(TransportError::NotConnected)?;
            writer.write_all(payload.as_bytes()).await.map_err(|e| {
                TransportError::Io(e)
            })?;
            writer.flush().await.map_err(TransportError::Io)?;
        }

        // Wait for response.
        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err_msg)) => Err(TransportError::RequestFailed(err_msg)),
            Err(_) => {
                // The sender was dropped (read loop exited).
                Err(TransportError::ConnectionFailed(
                    "Connection lost while waiting for response".into(),
                ))
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

        // Drop the writer to close our end of the socket.
        let mut writer_guard = self.writer.lock().await;
        *writer_guard = None;

        // Drop the event receiver.
        let mut rx_guard = self.event_rx.lock().await;
        *rx_guard = None;

        // Cancel all pending requests.
        let mut map = self.pending.lock().await;
        for (_id, sender) in map.drain() {
            let _ = sender.send(Err("Transport closed".into()));
        }

        debug!(path = %self.socket_path.display(), "Transport closed");
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

    #[test]
    fn test_unix_socket_transport_creation() {
        let transport = UnixSocketTransport::new("/tmp/test-bridge.sock");
        assert!(!transport.is_connected());
        assert_eq!(
            transport.socket_path,
            PathBuf::from("/tmp/test-bridge.sock")
        );
        assert!(format!("{:?}", transport).contains("UnixSocketTransport"));
    }

    #[tokio::test]
    async fn test_connect_nonexistent_socket() {
        let transport = UnixSocketTransport::new("/tmp/nonexistent-bridge-test.sock");
        let result = transport.connect(0, 100).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransportError::ConnectionFailed(msg) => {
                assert!(msg.contains("nonexistent-bridge-test.sock"));
                assert!(msg.contains("1 attempts"));
            }
            other => panic!("Expected ConnectionFailed, got: {other:?}"),
        }
        assert!(!transport.is_connected());
    }

    #[tokio::test]
    async fn test_connect_with_retries_nonexistent() {
        let transport = UnixSocketTransport::new("/tmp/nonexistent-bridge-retry.sock");
        let result = transport.connect(2, 10).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransportError::ConnectionFailed(msg) => {
                assert!(msg.contains("3 attempts"));
            }
            other => panic!("Expected ConnectionFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_request_when_not_connected() {
        let transport = UnixSocketTransport::new("/tmp/not-connected.sock");
        let result = transport
            .request("test", serde_json::Value::Null)
            .await;
        assert!(matches!(result, Err(TransportError::NotConnected)));
    }

    #[tokio::test]
    async fn test_close_when_not_connected() {
        let transport = UnixSocketTransport::new("/tmp/close-test.sock");
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

    #[test]
    fn test_json_rpc_message_deserialization_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, Some(1));
        assert!(msg.result.is_some());
        assert!(msg.error.is_none());
        assert!(msg.method.is_none());
    }

    #[test]
    fn test_json_rpc_message_deserialization_error() {
        let json = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32600,"message":"Invalid request"}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, Some(2));
        assert!(msg.result.is_none());
        assert!(msg.error.is_some());
        assert_eq!(msg.error.unwrap().message, "Invalid request");
    }

    #[test]
    fn test_json_rpc_message_deserialization_notification() {
        let json = r#"{"jsonrpc":"2.0","method":"event","params":{"type":"ready"}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        assert!(msg.id.is_none());
        assert_eq!(msg.method.as_deref(), Some("event"));
        assert!(msg.params.is_some());
    }
}
