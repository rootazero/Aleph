//! Gateway client implementation
//!
//! Main client type that coordinates transport, RPC, and authentication.

use crate::{ClientError, Result, Transport, RpcClient, WsWriter, WsReader, ConfigStore, AuthToken};
use aleph_protocol::{JsonRpcRequest, JsonRpcResponse, StreamEvent, ClientManifest, ClientCapabilities, ClientEnvironment, ExecutionConstraints};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};
use futures_util::StreamExt;

#[cfg(feature = "tracing")]
use tracing::{debug, info, warn, error};

/// Gateway client
///
/// High-level client for connecting to Aleph Gateway.
/// Manages WebSocket connection, JSON-RPC protocol, and event streaming.
pub struct GatewayClient {
    url: String,
    transport: Transport,
    rpc: RpcClient,
    writer: Arc<Mutex<Option<WsWriter>>>,
    event_tx: Arc<Mutex<Option<mpsc::Sender<StreamEvent>>>>,
    connected: Arc<AtomicBool>,
    auth_token: Arc<RwLock<Option<AuthToken>>>,
}

impl GatewayClient {
    /// Create a new gateway client
    ///
    /// # Example
    ///
    /// ```no_run
    /// use aleph_client_sdk::GatewayClient;
    ///
    /// let client = GatewayClient::new("ws://127.0.0.1:18789");
    /// ```
    pub fn new(url: &str) -> Self {
        #[cfg(feature = "tracing")]
        info!("Creating GatewayClient for {}", url);

        Self {
            url: url.to_string(),
            transport: Transport::new(url.to_string()),
            rpc: RpcClient::new(),
            writer: Arc::new(Mutex::new(None)),
            event_tx: Arc::new(Mutex::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            auth_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the gateway URL
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Connect to the gateway
    ///
    /// Returns a receiver for stream events (notifications from server).
    /// The connection is maintained in the background.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use aleph_client_sdk::GatewayClient;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = GatewayClient::new("ws://127.0.0.1:18789");
    /// let mut events = client.connect().await?;
    ///
    /// // Handle events in background
    /// tokio::spawn(async move {
    ///     while let Some(event) = events.recv().await {
    ///         println!("Event: {:?}", event);
    ///     }
    /// });
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(&self) -> Result<mpsc::Receiver<StreamEvent>> {
        #[cfg(feature = "tracing")]
        info!("Connecting to gateway at {}", self.url);

        // Connect transport
        let (writer, reader) = self.transport.connect().await?;

        // Create event channel
        let (event_tx, event_rx) = mpsc::channel(100);

        // Store writer and event sender
        *self.writer.lock().await = Some(writer.clone());
        *self.event_tx.lock().await = Some(event_tx.clone());
        self.connected.store(true, Ordering::SeqCst);

        // Spawn read loop
        let rpc = self.rpc.clone();
        let event_tx_clone = event_tx.clone();
        let connected = self.connected.clone();
        let writer_clone = writer.clone();

        tokio::spawn(async move {
            Self::read_loop(reader, rpc, event_tx_clone, connected, writer_clone).await;
        });

        #[cfg(feature = "tracing")]
        info!("Connected to gateway");

        Ok(event_rx)
    }

    /// Read loop for incoming messages
    ///
    /// Runs in background and routes messages to appropriate handlers:
    /// - JSON-RPC responses → RpcClient
    /// - JSON-RPC requests from server → handle_server_request
    /// - Notifications → event channel
    async fn read_loop(
        mut reader: WsReader,
        rpc: RpcClient,
        event_tx: mpsc::Sender<StreamEvent>,
        connected: Arc<AtomicBool>,
        writer: WsWriter,
    ) {
        use tokio_tungstenite::tungstenite::Message;

        while let Some(msg) = reader.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    Self::handle_message(&text, &rpc, &event_tx, &writer).await;
                }
                Ok(Message::Close(_)) => {
                    #[cfg(feature = "tracing")]
                    info!("Server closed connection");
                    break;
                }
                Ok(Message::Ping(_)) => {
                    #[cfg(feature = "tracing")]
                    debug!("Received ping (auto-ponged by tungstenite)");
                }
                Err(_e) => {
                    #[cfg(feature = "tracing")]
                    error!("WebSocket error: {}", _e);
                    break;
                }
                _ => {}
            }
        }

        connected.store(false, Ordering::SeqCst);
        #[cfg(feature = "tracing")]
        info!("Read loop ended");
    }

    /// Handle incoming message
    ///
    /// Routes message to appropriate handler based on JSON-RPC structure
    async fn handle_message(
        text: &str,
        rpc: &RpcClient,
        event_tx: &mpsc::Sender<StreamEvent>,
        writer: &WsWriter,
    ) {
        #[cfg(feature = "tracing")]
        debug!("Received message: {} bytes", text.len());

        // Try to parse as response first (response to our request)
        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(text) {
            let id = match &response.id {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Null => {
                    #[cfg(feature = "tracing")]
                    debug!("Response has null id, treating as notification");
                    String::new()
                }
                _ => return,
            };

            // Only process as response if we have a valid id
            if !id.is_empty() {
                #[cfg(feature = "tracing")]
                debug!("Routing response with id {} to RpcClient", id);
                rpc.handle_response(response).await;
                return;
            }
        }

        // Try to parse as request (from server)
        if let Ok(request) = serde_json::from_str::<JsonRpcRequest>(text) {
            let is_request = match &request.id {
                Some(Value::Null) => false,  // null id = notification
                Some(_) => true,              // non-null id = request
                None => false,                // no id = notification
            };

            if is_request {
                // This is a request from server that needs a response
                let id = request.id.clone().unwrap();
                #[cfg(feature = "tracing")]
                warn!(
                    method = %request.method,
                    "Received request from server, but no handler registered. \
                    Consider implementing a request handler or using LocalExecutor."
                );

                // Send method not found error
                use aleph_protocol::JsonRpcError;
                let error = JsonRpcError::method_not_found(&request.method);
                let response = JsonRpcResponse::error(id, error);

                if let Ok(json) = serde_json::to_string(&response) {
                    use futures_util::SinkExt;
                    use tokio_tungstenite::tungstenite::Message;
                    let mut write = writer.lock().await;
                    let _ = write.send(Message::Text(json)).await;
                }
                return;
            }

            // This is a notification (no response expected)
            if let Some(params) = request.params {
                #[cfg(feature = "tracing")]
                debug!(method = %request.method, "Received notification");

                match serde_json::from_value::<StreamEvent>(params.clone()) {
                    Ok(event) => {
                        #[cfg(feature = "tracing")]
                        debug!("Parsed stream event: {:?}", event);
                        let _ = event_tx.send(event).await;
                    }
                    Err(_e) => {
                        #[cfg(feature = "tracing")]
                        debug!("Failed to parse as StreamEvent: {}", _e);
                    }
                }
            }
        }
    }

    /// Send RPC call and wait for response
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use aleph_client_sdk::GatewayClient;
    /// # use serde_json::Value;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = GatewayClient::new("ws://127.0.0.1:18789");
    /// client.connect().await?;
    ///
    /// let result: Value = client.call("ping", None::<Value>).await?;
    /// println!("Pong: {:?}", result);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<R> {
        self.call_with_timeout(method, params, Duration::from_secs(30)).await
    }

    /// Send RPC call with custom timeout
    pub async fn call_with_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
        timeout: Duration,
    ) -> Result<R> {
        if !self.is_connected() {
            return Err(ClientError::ConnectionClosed);
        }

        #[cfg(feature = "tracing")]
        debug!("Calling RPC method: {}", method);

        // Generate ID
        let id = self.rpc.next_id();

        // Build request
        let request = self.rpc.build_request(method, params, Some(id.clone()))?;

        // Register pending request
        let rx = self.rpc.register_pending_async(id.clone()).await;

        // Send request
        let json = serde_json::to_string(&request)
            .map_err(|e| ClientError::SerializationError(e.to_string()))?;

        {
            let writer_opt = self.writer.lock().await;
            let writer = writer_opt.as_ref()
                .ok_or(ClientError::ConnectionClosed)?;

            self.transport.send(writer, json).await?;
        }

        // Wait for response
        self.rpc.call_with_timeout(rx, timeout, id).await
    }

    /// Send notification (no response expected)
    pub async fn notify<P: Serialize>(&self, method: &str, params: Option<P>) -> Result<()> {
        if !self.is_connected() {
            return Err(ClientError::ConnectionClosed);
        }

        #[cfg(feature = "tracing")]
        debug!("Sending notification: {}", method);

        let request = self.rpc.build_request(method, params, None)?;
        let json = serde_json::to_string(&request)
            .map_err(|e| ClientError::SerializationError(e.to_string()))?;

        let writer_opt = self.writer.lock().await;
        let writer = writer_opt.as_ref()
            .ok_or(ClientError::ConnectionClosed)?;

        self.transport.send(writer, json).await
    }

    /// Close the connection
    pub async fn close(&self) -> Result<()> {
        if !self.is_connected() {
            return Ok(());
        }

        #[cfg(feature = "tracing")]
        info!("Closing connection");

        let writer_opt = self.writer.lock().await;
        if let Some(writer) = writer_opt.as_ref() {
            self.transport.close(writer).await?;
        }

        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Authenticate with the gateway
    ///
    /// Uses ConfigStore to load/save authentication tokens.
    /// Sends client manifest with capabilities and environment info.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use aleph_client_sdk::{GatewayClient, ConfigStore, Result};
    /// # use async_trait::async_trait;
    /// # struct MyConfig;
    /// # #[async_trait]
    /// # impl ConfigStore for MyConfig {
    /// #     async fn load_token(&self) -> Result<Option<String>> { Ok(None) }
    /// #     async fn save_token(&self, _token: &str) -> Result<()> { Ok(()) }
    /// #     async fn clear_token(&self) -> Result<()> { Ok(()) }
    /// #     async fn get_or_create_device_id(&self) -> String { "test".to_string() }
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> Result<()> {
    /// let client = GatewayClient::new("ws://127.0.0.1:18789");
    /// client.connect().await?;
    ///
    /// let config = MyConfig;
    /// let token = client.authenticate(&config, "my-client", vec![], None).await?;
    /// println!("Authenticated with token: {}", token);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn authenticate<C: ConfigStore>(
        &self,
        config: &C,
        client_type: &str,
        tool_categories: Vec<String>,
        specific_tools: Option<Vec<String>>,
    ) -> Result<AuthToken> {
        #[cfg(feature = "tracing")]
        info!("Authenticating with gateway");

        // Load existing token if available
        let existing_token = config.load_token().await?;

        // Get device ID
        let device_id = config.get_or_create_device_id().await;

        // Build client manifest
        let manifest = ClientManifest {
            client_type: client_type.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: ClientCapabilities {
                tool_categories,
                specific_tools: specific_tools.unwrap_or_default(),
                excluded_tools: Vec::new(),
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

        #[derive(Deserialize)]
        struct ConnectResult {
            token: String,
            #[allow(dead_code)]
            manifest_accepted: bool,
        }

        let params = ConnectParams {
            device_id: device_id.clone(),
            device_name: device_id, // Use device_id as default name
            manifest,
            token: existing_token,
        };

        // Call connect RPC method
        let result: ConnectResult = self.call("connect", Some(params)).await?;

        // Save token
        config.save_token(&result.token).await?;
        *self.auth_token.write().await = Some(result.token.clone());

        #[cfg(feature = "tracing")]
        info!("Authentication successful");

        Ok(result.token)
    }

    /// Get current auth token
    pub async fn auth_token(&self) -> Option<AuthToken> {
        self.auth_token.read().await.clone()
    }
}
