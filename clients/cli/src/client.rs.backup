//! WebSocket client for Aleph Gateway
//!
//! This module provides a JSON-RPC 2.0 client over WebSocket,
//! using only types from `aleph-protocol`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aleph_protocol::{
    jsonrpc::TOOL_ERROR,
    ClientCapabilities, ClientEnvironment, ClientManifest, ExecutionConstraints,
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, StreamEvent,
};
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, warn};

use crate::config::CliConfig;
use crate::error::{CliError, CliResult};
use crate::executor::LocalExecutor;

/// Pending RPC request
struct PendingRequest {
    tx: oneshot::Sender<Result<Value, JsonRpcError>>,
}

/// Server request parameters (tool.call)
#[derive(Debug, Deserialize)]
struct ToolCallRequest {
    /// Tool name (Server uses "tool" field)
    #[serde(alias = "tool_name")]
    tool: String,
    /// Tool arguments (Server uses "args" field)
    #[serde(alias = "params")]
    args: Value,
}

/// Type alias for WebSocket write half
type WsWriter = Arc<Mutex<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>;

/// WebSocket client for Aleph Gateway
pub struct AlephClient {
    /// WebSocket write half
    write: WsWriter,
    /// Pending requests waiting for response
    pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
    /// Request ID counter
    id_counter: Arc<std::sync::atomic::AtomicU64>,
    /// Stream event channel
    event_tx: mpsc::Sender<StreamEvent>,
    /// Whether client is connected
    connected: Arc<std::sync::atomic::AtomicBool>,
    /// Authentication token
    auth_token: Arc<RwLock<Option<String>>>,
}

impl AlephClient {
    /// Connect to Aleph Gateway
    pub async fn connect(url: &str) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
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
            event_tx: event_tx.clone(),
            connected: connected.clone(),
            auth_token: Arc::new(RwLock::new(None)),
        };

        // Spawn read task with write access for responding to Server requests
        let pending_clone = pending.clone();
        let event_tx_clone = event_tx.clone();
        let connected_clone = connected.clone();
        let write_clone = write.clone();

        tokio::spawn(async move {
            Self::read_loop(read, pending_clone, event_tx_clone, connected_clone, write_clone).await;
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
        // Log all incoming messages for debugging
        debug!("Received raw message: {}", &text[..text.len().min(500)]);

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
                if let Some(req) = pending_guard.remove(&id) {
                    let result = if let Some(error) = response.error {
                        Err(error)
                    } else {
                        Ok(response.result.unwrap_or(Value::Null))
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
            let is_request = match &request.id {
                Some(Value::Null) => false,  // null id means notification
                Some(_) => true,              // non-null id means request
                None => false,                // no id means notification
            };

            if is_request {
                // This is a request from Server that needs a response
                let id = request.id.clone().unwrap();
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

    /// Handle a request from Server (e.g., tool.call)
    async fn handle_server_request(request: &JsonRpcRequest, id: Value, write: &WsWriter) {
        let response = match request.method.as_str() {
            "tool.call" => {
                Self::handle_tool_call(request.params.clone()).await
            }
            _ => {
                warn!(method = %request.method, "Unknown method from Server");
                Err(JsonRpcError::method_not_found(&request.method))
            }
        };

        // Build response
        let rpc_response = match response {
            Ok(result) => JsonRpcResponse::success(id, result),
            Err(error) => JsonRpcResponse::error(id, error),
        };

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

    /// Handle tool.call request from Server
    async fn handle_tool_call(params: Option<Value>) -> Result<Value, JsonRpcError> {
        let params = params.ok_or_else(|| {
            JsonRpcError::invalid_params("Missing params for tool.call")
        })?;

        let tool_req: ToolCallRequest = serde_json::from_value(params)
            .map_err(|e| JsonRpcError::invalid_params(format!("Invalid tool.call params: {}", e)))?;

        info!(tool = %tool_req.tool, "Executing local tool for Server");

        // Execute the tool locally
        match LocalExecutor::execute(&tool_req.tool, tool_req.args).await {
            Ok(result) => {
                info!(tool = %tool_req.tool, "Tool execution succeeded");
                Ok(result)
            }
            Err(e) => {
                error!(tool = %tool_req.tool, error = %e, "Tool execution failed");
                Err(JsonRpcError::with_data(
                    TOOL_ERROR,
                    format!("Tool execution failed: {}", e),
                    serde_json::json!({"tool": tool_req.tool}),
                ))
            }
        }
    }

    /// Generate next request ID
    fn next_id(&self) -> String {
        let id = self.id_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        id.to_string()
    }

    /// Send a JSON-RPC request and wait for response
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> CliResult<R> {
        self.call_with_timeout(method, params, Duration::from_secs(30)).await
    }

    /// Send a JSON-RPC request with custom timeout
    pub async fn call_with_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
        timeout: Duration,
    ) -> CliResult<R> {
        if !self.connected.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CliError::Disconnected);
        }

        let id = self.next_id();
        let params_value = params
            .map(|p| serde_json::to_value(p))
            .transpose()?;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params_value,
            id: Some(Value::String(id.clone())),
        };

        let (tx, rx) = oneshot::channel();

        // Register pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(id.clone(), PendingRequest { tx });
        }

        // Send request
        let json = serde_json::to_string(&request)?;
        debug!("Sending: {}", json);

        {
            let mut write = self.write.lock().await;
            write.send(Message::Text(json.into())).await?;
        }

        // Wait for response with timeout
        let result = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| {
                // Remove pending request on timeout
                let pending = self.pending.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    pending.write().await.remove(&id);
                });
                CliError::Timeout
            })?
            .map_err(|_| CliError::Disconnected)?;

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

    /// Send a notification (no response expected)
    pub async fn notify<P: Serialize>(&self, method: &str, params: Option<P>) -> CliResult<()> {
        if !self.connected.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CliError::Disconnected);
        }

        let params_value = params
            .map(|p| serde_json::to_value(p))
            .transpose()?;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params_value,
            id: None,
        };

        let json = serde_json::to_string(&request)?;
        debug!("Sending notification: {}", json);

        let mut write = self.write.lock().await;
        write.send(Message::Text(json.into())).await?;

        Ok(())
    }

    /// Connect and authenticate with the server
    pub async fn authenticate(&self, config: &CliConfig) -> CliResult<String> {
        // Build client manifest
        let manifest = ClientManifest {
            client_type: "cli".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: ClientCapabilities {
                tool_categories: config.manifest.tool_categories.clone(),
                specific_tools: config.manifest.specific_tools.clone(),
                excluded_tools: config.manifest.excluded_tools.clone(),
                constraints: ExecutionConstraints::default(),
                granted_scopes: None,
            },
            environment: ClientEnvironment {
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                sandbox: false,
            },
        };

        #[derive(Serialize)]
        struct ConnectParams {
            device_id: String,
            device_name: String,
            manifest: ClientManifest,
            #[serde(skip_serializing_if = "Option::is_none")]
            token: Option<String>,
        }

        #[derive(serde::Deserialize)]
        struct ConnectResult {
            token: String,
            #[allow(dead_code)]
            manifest_accepted: bool,
        }

        let params = ConnectParams {
            device_id: config.device_id.clone(),
            device_name: config.device_name.clone(),
            manifest,
            token: config.auth_token.clone(),
        };

        let result: ConnectResult = self.call("connect", Some(params)).await?;

        // Store token
        *self.auth_token.write().await = Some(result.token.clone());

        Ok(result.token)
    }

    /// Check if client is connected
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get current auth token
    pub async fn auth_token(&self) -> Option<String> {
        self.auth_token.read().await.clone()
    }

    /// Close the connection
    pub async fn close(&self) -> CliResult<()> {
        let mut write = self.write.lock().await;
        write.send(Message::Close(None)).await?;
        self.connected.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}
