//! WebSocket client for Aleph Gateway
//!
//! This module provides a JSON-RPC 2.0 client over WebSocket,
//! using only types from `aleph-protocol`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aleph_protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, StreamEvent};
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info, warn};

use crate::config::CliConfig;
use crate::error::{CliError, CliResult};

/// Pending RPC request
struct PendingRequest {
    tx: oneshot::Sender<Result<Value, JsonRpcError>>,
}

/// Type alias for WebSocket write half
type WsWriter = Arc<
    Mutex<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
>;

/// WebSocket client for Aleph Gateway
pub struct AlephClient {
    /// WebSocket write half
    write: WsWriter,
    /// Pending requests waiting for response
    pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
    /// Request ID counter
    id_counter: Arc<std::sync::atomic::AtomicU64>,
    /// Stream event channel (kept alive as ownership anchor for the Receiver)
    _event_tx: mpsc::Sender<StreamEvent>,
    /// Whether client is connected
    connected: Arc<std::sync::atomic::AtomicBool>,
    /// Server-assigned role from the `connect` handshake.
    role: String,
}

impl AlephClient {
    /// Connect to Aleph Gateway and complete the `connect` handshake.
    ///
    /// The handshake is not optional and is deliberately not a separate public
    /// step: the gateway enforces that the first frame on a connection is
    /// `connect` and *closes the socket* otherwise (`server::handler`), so a
    /// client that skipped it could never issue a single working call. Folding
    /// it in here means a caller cannot hold an un-handshaken `AlephClient` —
    /// twenty of twenty-eight CLI command modules used to skip it because the
    /// two steps were separable, and every one of their subcommands was dead
    /// against a live server.
    pub async fn connect(
        url: &str,
        config: &CliConfig,
    ) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
        let (mut client, events) = Self::open(url).await?;
        client.role = client.handshake(config).await?;
        Ok((client, events))
    }

    /// Open the socket and spawn the read loop, without handshaking.
    async fn open(url: &str) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
        info!("Connecting to {}", url);

        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| CliError::Connection(e.to_string()))?;

        let (write, read) = ws_stream.split();

        let (event_tx, event_rx) = mpsc::channel(100);
        let pending = Arc::new(RwLock::new(HashMap::new()));
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let write = Arc::new(Mutex::new(write));

        let client = Self {
            write: write.clone(),
            pending: pending.clone(),
            id_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            _event_tx: event_tx.clone(),
            connected: connected.clone(),
            role: String::new(),
        };

        // Spawn read task with write access for responding to Server requests
        let pending_clone = pending;
        let event_tx_clone = event_tx;
        let connected_clone = connected;
        let write_clone = write;

        tokio::spawn(async move {
            Self::read_loop(
                read,
                pending_clone,
                event_tx_clone,
                connected_clone,
                write_clone,
            )
            .await;
        });

        info!("Connected to Gateway");
        Ok((client, event_rx))
    }

    /// Read loop for incoming messages
    async fn read_loop(
        mut read: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
        event_tx: mpsc::Sender<StreamEvent>,
        connected: Arc<std::sync::atomic::AtomicBool>,
        write: WsWriter,
    ) {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    Self::handle_message(&text, &pending, &event_tx, &write).await;
                }
                Ok(Message::Close(_)) => {
                    info!("Server closed connection");
                    break;
                }
                Ok(Message::Ping(data)) => {
                    debug!("Received ping");
                    // Pong is handled automatically by tungstenite
                    let _ = data;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        connected.store(false, std::sync::atomic::Ordering::SeqCst);
        info!("Read loop ended");
    }

    /// Handle incoming message
    async fn handle_message(
        text: &str,
        pending: &Arc<RwLock<HashMap<String, PendingRequest>>>,
        event_tx: &mpsc::Sender<StreamEvent>,
        write: &WsWriter,
    ) {
        // Log all incoming messages for debugging.
        // Truncate on a UTF-8 char boundary — a byte slice (`&text[..n]`) panics
        // when byte `n` lands inside a multi-byte char (common with CJK/emoji),
        // which would kill the read loop and silently hang every pending request.
        let preview_end = text.char_indices().nth(500).map_or(text.len(), |(i, _)| i);
        debug!("Received raw message: {}", &text[..preview_end]);

        // Try to parse as response first (response to our request)
        // Only treat as response if id is a valid string or number (not null)
        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(text) {
            debug!("Parsed as JsonRpcResponse with id: {:?}", response.id);
            let id = match &response.id {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Null => {
                    // id is null, this is a notification, not a response
                    debug!("Response has null id, treating as notification");
                    // Fall through to try parsing as request
                    String::new()
                }
                _ => return,
            };

            // Only process as response if we have a valid id
            if !id.is_empty() {
                let mut pending_guard = pending.write().await;
                let maybe_req = pending_guard.remove(&id);
                drop(pending_guard);
                if let Some(req) = maybe_req {
                    let result = if let Some(error) = response.error {
                        Err(error)
                    } else if response.result.is_some() {
                        Ok(response.result.unwrap())
                    } else {
                        Err(JsonRpcError {
                            code: -32600,
                            message: "Invalid Request: missing both result and error".into(),
                            data: None,
                        })
                    };
                    let _ = req.tx.send(result);
                }
                return;
            }
        } else {
            debug!("Message is not a JsonRpcResponse, trying JsonRpcRequest");
        }

        // Try to parse as request (from Server)
        if let Ok(request) = serde_json::from_str::<JsonRpcRequest>(text) {
            // Check if this is a request (has non-null id) or notification (no id or null id)
            let id = match &request.id {
                Some(Value::Null) | None => None, // null/no id means notification
                Some(id) => Some(id.clone()),     // non-null id means request
            };

            if let Some(id) = id {
                // This is a request from Server that needs a response
                debug!(method = %request.method, "Received request from Server");
                Self::handle_server_request(&request, id, write).await;
                return;
            }

            // This is a notification (no response expected)
            if let Some(params) = request.params {
                debug!(method = %request.method, "Received notification");
                match serde_json::from_value::<StreamEvent>(params.clone()) {
                    Ok(event) => {
                        debug!("Parsed event: {:?}", event);
                        let _ = event_tx.send(event).await;
                    }
                    Err(e) => {
                        debug!("Failed to parse event: {} - params: {}", e, params);
                    }
                }
            }
        } else {
            debug!("Message is not a JsonRpcRequest either, ignoring");
        }
    }

    /// Handle a request from Server
    async fn handle_server_request(request: &JsonRpcRequest, id: Value, write: &WsWriter) {
        warn!(method = %request.method, "Unknown method from Server");
        let rpc_response =
            JsonRpcResponse::error(id, JsonRpcError::method_not_found(&request.method));

        // Send response
        let json = match serde_json::to_string(&rpc_response) {
            Ok(j) => j,
            Err(e) => {
                error!("Failed to serialize response: {}", e);
                return;
            }
        };

        debug!("Sending response to Server: {}", json);
        let mut write_guard = write.lock().await;
        if let Err(e) = write_guard.send(Message::Text(json.into())).await {
            error!("Failed to send response: {}", e);
        }
    }

    /// Generate next request ID
    fn next_id(&self) -> String {
        let id = self
            .id_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        id.to_string()
    }

    /// Send a JSON-RPC request and wait for response
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> CliResult<R> {
        self.call_with_timeout(method, params, Duration::from_secs(30))
            .await
    }

    /// Send a JSON-RPC request with custom timeout
    pub async fn call_with_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
        timeout: Duration,
    ) -> CliResult<R> {
        if !self.connected.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CliError::Disconnected(
                "Connection closed by peer".to_string(),
            ));
        }

        let id = self.next_id();
        let params_value = params.map(|p| serde_json::to_value(p)).transpose()?;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params_value,
            id: Some(Value::String(id.clone())),
        };

        // Serialise BEFORE registering the pending entry: a serde failure
        // here used to leak the oneshot sender (registered next) for the
        // lifetime of the client because the `?` returned while the entry
        // was still in the map.
        let json = serde_json::to_string(&request)?;
        debug!("Sending: {}", json);

        let (tx, rx) = oneshot::channel();

        // Register pending request (only after serialise has succeeded).
        {
            let mut pending = self.pending.write().await;
            pending.insert(id.clone(), PendingRequest { tx });
        }

        // Send request. Hold the write lock only for the send, then release it
        // before touching `pending` so we never nest `write` -> `pending` and
        // invert the lock order used by `handle_message` (`pending` -> `write`).
        let send_result = {
            let mut write = self.write.lock().await;
            write.send(Message::Text(json.into())).await
        };
        if let Err(e) = send_result {
            // Send failed after the pending entry was registered: drop it so
            // the pending map doesn't leak across repeated failures, and mark
            // the connection dead so subsequent calls fail fast.
            self.pending.write().await.remove(&id);
            self.connected
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err(e.into());
        }

        // Wait for response with timeout
        let result = match tokio::time::timeout(timeout, rx).await {
            Ok(result) => result,
            Err(e) => {
                // Remove pending request on timeout so the map doesn't leak
                // across repeated failures.
                self.pending.write().await.remove(&id);
                return Err(CliError::Timeout(e.to_string()));
            }
        }
        .map_err(|e| CliError::Disconnected(e.to_string()))?;

        match result {
            Ok(value) => {
                let result: R = serde_json::from_value(value)?;
                Ok(result)
            }
            Err(error) => Err(CliError::Rpc {
                code: error.code,
                message: error.message,
            }),
        }
    }

    /// Perform the `connect` handshake with the server.
    ///
    /// LAN-trust model: the gateway has no authentication. `connect` carries
    /// no credentials — it only declares a surface identity (`device_name`)
    /// and receives the session baseline back: `{ role, state_version,
    /// keepalive }`. No token is minted, stored, or replayed. Returns the
    /// server-assigned role (always `"operator"` under LAN-trust), which
    /// [`Self::connect`] latches into [`Self::role`].
    ///
    /// Private on purpose — see [`Self::connect`].
    async fn handshake(&self, config: &CliConfig) -> CliResult<String> {
        #[derive(Serialize)]
        struct ConnectParams<'a> {
            device_name: &'a str,
        }

        #[derive(serde::Deserialize)]
        struct ConnectResult {
            #[serde(default)]
            role: String,
        }

        let params = ConnectParams {
            device_name: &config.device_name,
        };

        let result: ConnectResult = self.call("connect", Some(params)).await?;
        Ok(result.role)
    }

    /// The server-assigned role latched at the `connect` handshake.
    ///
    /// Always `"operator"` under LAN-trust; surfaced so `aleph connect` can
    /// report what the server granted.
    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    /// Whether the WebSocket connection is still live.
    ///
    /// Reads the atomic the read loop clears when the socket drops (and that
    /// `call()` checks before sending). A pure read of existing state — no I/O,
    /// no server round-trip. Lets a UI reflect a disconnect even while idle.
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Close the connection
    pub async fn close(&self) -> CliResult<()> {
        let mut write = self.write.lock().await;
        write.send(Message::Close(None)).await?;
        drop(write);
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// The gateway rejects and *closes* any connection whose first frame is
    /// not `connect`, so a client that hands back a live handle before
    /// handshaking is useless — every later call dies on a dead socket. This
    /// pins the wire-level invariant that made twenty CLI command modules
    /// silently non-functional when the handshake was a separate opt-in step.
    #[tokio::test]
    async fn connect_handshakes_before_handing_back_the_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // Bounded so a regression that drops the handshake fails the test
            // instead of hanging it: with nothing sent, `next()` never returns.
            let first = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("client sent no frame — the handshake was skipped")
                .unwrap()
                .unwrap();
            let req: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
            let reply = serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": { "role": "operator" },
            });
            ws.send(Message::Text(reply.to_string().into()))
                .await
                .unwrap();
            req
        });

        let mut config = CliConfig::default();
        config.device_name = "test-device".to_string();
        let (client, _events) = AlephClient::connect(&format!("ws://{addr}"), &config)
            .await
            .unwrap();

        let first = server.await.unwrap();
        assert_eq!(
            first["method"], "connect",
            "the first frame on the wire must be the handshake"
        );
        assert_eq!(
            first["params"]["device_name"], "test-device",
            "the handshake must carry the configured device name"
        );
        assert_eq!(
            client.role(),
            "operator",
            "the server-assigned role must be latched for callers to read"
        );
    }
}
