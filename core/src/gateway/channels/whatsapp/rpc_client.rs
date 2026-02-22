//! Bridge RPC Client — JSON-RPC 2.0 over Unix Domain Socket
//!
//! Async client that communicates with the Go `whatsapp-bridge` binary
//! using newline-delimited JSON-RPC 2.0 over a Unix domain socket.
//!
//! # Wire Format
//!
//! Each message is a single JSON object terminated by `\n`.
//! Requests carry an `id` field; the bridge responds with the same `id`.
//! The bridge may also push **notifications** (no `id`, but a `method` field).
//!
//! # Usage
//!
//! ```ignore
//! let (event_tx, mut event_rx) = mpsc::channel(64);
//! let client = BridgeRpcClient::new("/tmp/bridge.sock", event_tx);
//! client.connect(5, 500).await?;
//!
//! let resp: PingResponse = client.call("ping", None).await?;
//! ```

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};

use super::bridge_manager::BridgeError;
use super::bridge_protocol::BridgeEvent;

// ─── Wire Types ──────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 request sent to the Go bridge.
#[derive(Debug, Serialize)]
struct RpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// JSON-RPC 2.0 response received from the Go bridge.
///
/// This type covers both normal responses (with `id`) and push notifications
/// (with `method` but no `id`).
#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<u64>,
    result: Option<Value>,
    error: Option<RpcError>,
    method: Option<String>,
    params: Option<Value>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

// ─── BridgeRpcClient ─────────────────────────────────────────────────────────

/// Async JSON-RPC client for communicating with the Go whatsapp-bridge.
///
/// The client connects to a Unix domain socket, splits the stream into
/// reader and writer halves, and spawns a background task to read responses
/// and event notifications.
pub struct BridgeRpcClient {
    /// Path to the Unix domain socket.
    socket_path: PathBuf,
    /// Writer half of the connected socket (None when disconnected).
    writer: Arc<Mutex<Option<WriteHalf<UnixStream>>>>,
    /// Pending RPC requests awaiting a response, keyed by request id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    /// Monotonically increasing request id counter.
    next_id: AtomicU64,
    /// Channel for forwarding push notifications from the bridge.
    event_tx: mpsc::Sender<BridgeEvent>,
}

impl BridgeRpcClient {
    /// Create a new `BridgeRpcClient` targeting the given socket path.
    ///
    /// The client is **not** connected after construction; call [`connect`]
    /// to establish the connection.
    pub fn new(socket_path: impl AsRef<Path>, event_tx: mpsc::Sender<BridgeEvent>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            writer: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_tx,
        }
    }

    /// Connect to the Unix domain socket with retry logic.
    ///
    /// Attempts to connect up to `max_retries` times, waiting
    /// `retry_delay_ms` milliseconds between attempts. On success the
    /// socket is split and a background read loop is spawned.
    pub async fn connect(
        &self,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> Result<(), BridgeError> {
        let mut last_error = String::new();

        for attempt in 0..=max_retries {
            match UnixStream::connect(&self.socket_path).await {
                Ok(stream) => {
                    let (reader, writer) = tokio::io::split(stream);
                    *self.writer.lock().await = Some(writer);

                    // Spawn the background read loop
                    let pending = Arc::clone(&self.pending);
                    let event_tx = self.event_tx.clone();
                    tokio::spawn(Self::read_loop(reader, pending, event_tx));

                    tracing::info!(
                        socket = ?self.socket_path,
                        attempt = attempt + 1,
                        "Connected to WhatsApp bridge RPC socket"
                    );
                    return Ok(());
                }
                Err(e) => {
                    last_error = e.to_string();
                    tracing::debug!(
                        socket = ?self.socket_path,
                        attempt = attempt + 1,
                        max_retries = max_retries,
                        error = %e,
                        "Failed to connect to bridge socket, retrying..."
                    );
                    if attempt < max_retries {
                        tokio::time::sleep(tokio::time::Duration::from_millis(retry_delay_ms))
                            .await;
                    }
                }
            }
        }

        Err(BridgeError::SocketError(format!(
            "Failed to connect to socket {:?} after {} attempts: {}",
            self.socket_path,
            max_retries + 1,
            last_error
        )))
    }

    /// Background task that reads newline-delimited JSON from the socket.
    ///
    /// - If the message has an `id`, it is matched to a pending request and
    ///   the result (or error) is sent back via the oneshot channel.
    /// - If the message has a `method` (no `id`), it is treated as a push
    ///   notification and forwarded via `event_tx`.
    async fn read_loop(
        reader: tokio::io::ReadHalf<UnixStream>,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
        event_tx: mpsc::Sender<BridgeEvent>,
    ) {
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match buf_reader.read_line(&mut line).await {
                Ok(0) => {
                    // EOF — socket closed
                    tracing::info!("Bridge RPC socket closed (EOF)");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<RpcResponse>(trimmed) {
                        Ok(resp) => {
                            if let Some(id) = resp.id {
                                // This is a response to a request
                                let result = if let Some(err) = resp.error {
                                    Err(format!(
                                        "RPC error {}: {}",
                                        err.code, err.message
                                    ))
                                } else {
                                    Ok(resp.result.unwrap_or(Value::Null))
                                };

                                let mut map = pending.lock().await;
                                if let Some(sender) = map.remove(&id) {
                                    let _ = sender.send(result);
                                } else {
                                    tracing::warn!(
                                        id = id,
                                        "Received response for unknown request id"
                                    );
                                }
                            } else if resp.method.is_some() {
                                // This is a push notification
                                let params = resp.params.unwrap_or(Value::Null);
                                match serde_json::from_value::<BridgeEvent>(params) {
                                    Ok(event) => {
                                        if event_tx.send(event).await.is_err() {
                                            tracing::debug!(
                                                "Event receiver dropped, stopping read loop"
                                            );
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            method = ?resp.method,
                                            error = %e,
                                            "Failed to parse bridge event"
                                        );
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    line = trimmed,
                                    "Received message with neither id nor method"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                line = trimmed,
                                error = %e,
                                "Failed to parse JSON-RPC response"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Error reading from bridge socket");
                    break;
                }
            }
        }

        // Clean up all pending requests on disconnect
        let mut map = pending.lock().await;
        for (_, sender) in map.drain() {
            let _ = sender.send(Err("Bridge socket disconnected".to_string()));
        }
    }

    /// Invoke a JSON-RPC method on the bridge and await the response.
    ///
    /// The response is deserialized into `T`. A 30-second timeout is applied.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeError::SocketError`] if:
    /// - The client is not connected
    /// - Writing to the socket fails
    /// - The call times out (30 seconds)
    /// - The bridge returns an RPC error
    /// - The response cannot be deserialized into `T`
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T, BridgeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = RpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let mut payload = serde_json::to_string(&request).map_err(|e| {
            BridgeError::SocketError(format!("Failed to serialize RPC request: {}", e))
        })?;
        payload.push('\n');

        // Register the pending request
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        // Write to socket
        {
            let mut writer_guard = self.writer.lock().await;
            let writer = writer_guard.as_mut().ok_or_else(|| {
                BridgeError::SocketError("Not connected to bridge socket".to_string())
            })?;

            writer.write_all(payload.as_bytes()).await.map_err(|e| {
                BridgeError::SocketError(format!("Failed to write to bridge socket: {}", e))
            })?;
            writer.flush().await.map_err(|e| {
                BridgeError::SocketError(format!("Failed to flush bridge socket: {}", e))
            })?;
        }

        // Wait for response with timeout
        let result = tokio::time::timeout(tokio::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                // Remove from pending on timeout
                let pending = Arc::clone(&self.pending);
                let id = id;
                tokio::spawn(async move {
                    let mut map = pending.lock().await;
                    map.remove(&id);
                });
                BridgeError::SocketError(format!(
                    "RPC call '{}' timed out after 30 seconds",
                    method
                ))
            })?
            .map_err(|_| {
                BridgeError::SocketError("Response channel closed unexpectedly".to_string())
            })?
            .map_err(|e| BridgeError::SocketError(e))?;

        serde_json::from_value(result).map_err(|e| {
            BridgeError::SocketError(format!("Failed to deserialize RPC response: {}", e))
        })
    }

    /// Check whether the client currently has an active socket connection.
    pub fn is_connected(&self) -> bool {
        // Use try_lock to avoid blocking; if we can't acquire, assume connected
        match self.writer.try_lock() {
            Ok(guard) => guard.is_some(),
            Err(_) => true,
        }
    }

    /// Disconnect from the bridge socket.
    ///
    /// Drops the writer half (closing the socket) and cancels all pending
    /// requests with an error.
    pub async fn disconnect(&self) {
        // Drop the writer to close the socket
        *self.writer.lock().await = None;

        // Cancel all pending requests
        let mut map = self.pending.lock().await;
        for (_, sender) in map.drain() {
            let _ = sender.send(Err("Client disconnected".to_string()));
        }

        tracing::info!(socket = ?self.socket_path, "Disconnected from bridge RPC socket");
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── RpcRequest serialization ─────────────────────────────────────

    #[test]
    fn test_rpc_request_serialization_with_params() {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "send".to_string(),
            params: Some(serde_json::json!({
                "to": "user@s.whatsapp.net",
                "text": "Hello"
            })),
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "send");
        assert_eq!(json["params"]["to"], "user@s.whatsapp.net");
        assert_eq!(json["params"]["text"], "Hello");
    }

    #[test]
    fn test_rpc_request_serialization_without_params() {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 42,
            method: "ping".to_string(),
            params: None,
        };

        let json_str = serde_json::to_string(&req).unwrap();
        // params should be omitted entirely
        assert!(!json_str.contains("params"));

        let json: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 42);
        assert_eq!(json["method"], "ping");
    }

    #[test]
    fn test_rpc_request_id_is_always_present() {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 0,
            method: "status".to_string(),
            params: None,
        };

        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("id").is_some());
        assert_eq!(json["id"], 0);
    }

    #[test]
    fn test_rpc_request_newline_delimited_format() {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "connect".to_string(),
            params: Some(serde_json::json!({})),
        };

        let mut payload = serde_json::to_string(&req).unwrap();
        payload.push('\n');

        assert!(payload.ends_with('\n'));
        // Should be a single line (no embedded newlines before the trailing one)
        assert_eq!(payload.trim().lines().count(), 1);
    }

    // ── RpcResponse deserialization ──────────────────────────────────

    #[test]
    fn test_rpc_response_success() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        assert!(resp.method.is_none());
        assert!(resp.params.is_none());

        let result = resp.result.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn test_rpc_response_error() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 2,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        }"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.id, Some(2));
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn test_rpc_response_notification() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "event.push",
            "params": {
                "type": "connected",
                "device_name": "iPhone 15",
                "phone_number": "+1234567890"
            }
        }"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();

        assert!(resp.id.is_none());
        assert!(resp.result.is_none());
        assert!(resp.error.is_none());
        assert_eq!(resp.method, Some("event.push".to_string()));
        assert!(resp.params.is_some());

        // The params should be parseable as a BridgeEvent
        let params = resp.params.unwrap();
        let event: BridgeEvent = serde_json::from_value(params).unwrap();
        match event {
            BridgeEvent::Connected {
                device_name,
                phone_number,
            } => {
                assert_eq!(device_name, "iPhone 15");
                assert_eq!(phone_number, "+1234567890");
            }
            _ => panic!("Expected Connected event"),
        }
    }

    #[test]
    fn test_rpc_response_null_result() {
        let json = r#"{"jsonrpc": "2.0", "id": 5, "result": null}"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.id, Some(5));
        // serde deserializes JSON null as None for Option<Value>
        assert!(resp.result.is_none());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_rpc_response_with_string_result() {
        let json = r#"{"jsonrpc": "2.0", "id": 3, "result": "pong"}"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.id, Some(3));
        assert_eq!(resp.result.unwrap(), "pong");
    }

    // ── RpcError deserialization ─────────────────────────────────────

    #[test]
    fn test_rpc_error_deserialization() {
        let json = r#"{"code": -32700, "message": "Parse error"}"#;
        let err: RpcError = serde_json::from_str(json).unwrap();

        assert_eq!(err.code, -32700);
        assert_eq!(err.message, "Parse error");
    }

    #[test]
    fn test_rpc_error_negative_code() {
        let json = r#"{"code": -1, "message": "Internal error"}"#;
        let err: RpcError = serde_json::from_str(json).unwrap();

        assert_eq!(err.code, -1);
        assert_eq!(err.message, "Internal error");
    }

    // ── Client state ─────────────────────────────────────────────────

    #[test]
    fn test_client_not_connected_by_default() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new("/tmp/nonexistent.sock", event_tx);

        assert!(!client.is_connected());
    }

    #[test]
    fn test_client_stores_socket_path() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new("/tmp/test-bridge.sock", event_tx);

        assert_eq!(client.socket_path, PathBuf::from("/tmp/test-bridge.sock"));
    }

    #[test]
    fn test_client_initial_id_counter() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new("/tmp/test.sock", event_tx);

        // First id should be 1
        let id = client.next_id.load(Ordering::Relaxed);
        assert_eq!(id, 1);
    }

    #[tokio::test]
    async fn test_client_pending_map_initially_empty() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new("/tmp/test.sock", event_tx);

        let pending = client.pending.lock().await;
        assert!(pending.is_empty());
    }

    // ── Connect to nonexistent socket ────────────────────────────────

    #[tokio::test]
    async fn test_connect_to_nonexistent_socket_fails() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new(
            "/tmp/nonexistent-whatsapp-bridge-test-socket-xyz.sock",
            event_tx,
        );

        let result = client.connect(0, 0).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            BridgeError::SocketError(msg) => {
                assert!(
                    msg.contains("Failed to connect"),
                    "Error should mention connection failure, got: {}",
                    msg
                );
                assert!(
                    msg.contains("nonexistent-whatsapp-bridge-test-socket-xyz.sock"),
                    "Error should mention socket path, got: {}",
                    msg
                );
            }
            other => panic!("Expected SocketError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_connect_with_retries_to_nonexistent_socket() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new(
            "/tmp/nonexistent-whatsapp-retry-test.sock",
            event_tx,
        );

        // Use 2 retries with minimal delay to keep the test fast
        let result = client.connect(2, 10).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            BridgeError::SocketError(msg) => {
                assert!(
                    msg.contains("3 attempts"),
                    "Should report total attempts (retries + 1), got: {}",
                    msg
                );
            }
            other => panic!("Expected SocketError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_client_not_connected_after_failed_connect() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new(
            "/tmp/nonexistent-whatsapp-state-test.sock",
            event_tx,
        );

        let _ = client.connect(0, 0).await;
        assert!(!client.is_connected());
    }

    // ── Disconnect ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new("/tmp/test.sock", event_tx);

        // Should not panic
        client.disconnect().await;
        assert!(!client.is_connected());
    }

    // ── Call without connection ───────────────────────────────────────

    #[tokio::test]
    async fn test_call_without_connection_fails() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let client = BridgeRpcClient::new("/tmp/test.sock", event_tx);

        let result: Result<Value, BridgeError> = client.call("ping", None).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            BridgeError::SocketError(msg) => {
                assert!(
                    msg.contains("Not connected"),
                    "Error should mention not connected, got: {}",
                    msg
                );
            }
            other => panic!("Expected SocketError, got: {:?}", other),
        }
    }

    // ── Notification event parsing ───────────────────────────────────

    #[test]
    fn test_notification_qr_event_via_rpc_response() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "event.push",
            "params": {
                "type": "qr",
                "qr_data": "2@ABC123",
                "expires_in_secs": 60
            }
        }"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.id.is_none());
        assert!(resp.method.is_some());

        let event: BridgeEvent = serde_json::from_value(resp.params.unwrap()).unwrap();
        match event {
            BridgeEvent::Qr {
                qr_data,
                expires_in_secs,
            } => {
                assert_eq!(qr_data, "2@ABC123");
                assert_eq!(expires_in_secs, 60);
            }
            _ => panic!("Expected Qr event"),
        }
    }

    #[test]
    fn test_notification_message_event_via_rpc_response() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "event.push",
            "params": {
                "type": "message",
                "from": "user@s.whatsapp.net",
                "chat_id": "user@s.whatsapp.net",
                "text": "Hello!",
                "timestamp": 1708531200,
                "message_id": "msg-001",
                "is_group": false
            }
        }"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();
        let event: BridgeEvent = serde_json::from_value(resp.params.unwrap()).unwrap();

        match event {
            BridgeEvent::Message {
                from,
                text,
                message_id,
                ..
            } => {
                assert_eq!(from, "user@s.whatsapp.net");
                assert_eq!(text, "Hello!");
                assert_eq!(message_id, "msg-001");
            }
            _ => panic!("Expected Message event"),
        }
    }

    #[test]
    fn test_notification_ready_event_via_rpc_response() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "event.push",
            "params": {
                "type": "ready"
            }
        }"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();
        let event: BridgeEvent = serde_json::from_value(resp.params.unwrap()).unwrap();
        assert_eq!(event, BridgeEvent::Ready);
    }

    #[test]
    fn test_notification_error_event_via_rpc_response() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "event.push",
            "params": {
                "type": "error",
                "message": "connection lost"
            }
        }"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();
        let event: BridgeEvent = serde_json::from_value(resp.params.unwrap()).unwrap();

        match event {
            BridgeEvent::Error { message } => {
                assert_eq!(message, "connection lost");
            }
            _ => panic!("Expected Error event"),
        }
    }
}
