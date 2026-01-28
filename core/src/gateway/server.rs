//! WebSocket Gateway Server
//!
//! Handles WebSocket connections and dispatches JSON-RPC 2.0 requests
//! to registered handlers.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use futures_util::{StreamExt, SinkExt};
use tracing::{info, warn, error, debug};

use super::protocol::{JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};
use super::event_bus::GatewayEventBus;
use super::handlers::HandlerRegistry;

/// State for an individual WebSocket connection
#[allow(dead_code)]
struct ConnectionState {
    /// Whether the connection has been authenticated
    authenticated: bool,
    /// Event topics this connection is subscribed to
    subscriptions: Vec<String>,
    /// Connection metadata
    metadata: HashMap<String, String>,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            authenticated: false,
            subscriptions: vec![],
            metadata: HashMap::new(),
        }
    }
}

/// Configuration for the Gateway server
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Whether to require authentication
    pub require_auth: bool,
    /// Connection timeout in seconds
    pub timeout_secs: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            max_connections: 1000,
            require_auth: false,
            timeout_secs: 300,
        }
    }
}

/// WebSocket Gateway Server
///
/// The main entry point for running a Gateway server. Creates a TCP listener,
/// accepts WebSocket connections, and dispatches JSON-RPC requests to handlers.
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::gateway::GatewayServer;
/// use std::net::SocketAddr;
///
/// #[tokio::main]
/// async fn main() {
///     let addr: SocketAddr = "127.0.0.1:18789".parse().unwrap();
///     let server = GatewayServer::new(addr);
///     server.run().await.unwrap();
/// }
/// ```
pub struct GatewayServer {
    addr: SocketAddr,
    config: GatewayConfig,
    handlers: Arc<HandlerRegistry>,
    event_bus: Arc<GatewayEventBus>,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
}

impl GatewayServer {
    /// Create a new Gateway server with default configuration
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            config: GatewayConfig::default(),
            handlers: Arc::new(HandlerRegistry::new()),
            event_bus: Arc::new(GatewayEventBus::new()),
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a Gateway server with custom configuration
    pub fn with_config(addr: SocketAddr, config: GatewayConfig) -> Self {
        Self {
            addr,
            config,
            handlers: Arc::new(HandlerRegistry::new()),
            event_bus: Arc::new(GatewayEventBus::new()),
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get a reference to the handler registry for registering custom handlers
    pub fn handlers(&self) -> &Arc<HandlerRegistry> {
        &self.handlers
    }

    /// Get a mutable reference to the handler registry
    ///
    /// Note: This consumes the Arc and returns a new one.
    /// Should only be called during setup, before `run()`.
    pub fn handlers_mut(&mut self) -> &mut HandlerRegistry {
        Arc::get_mut(&mut self.handlers)
            .expect("Cannot modify handlers after server is running")
    }

    /// Get a reference to the event bus for publishing events
    pub fn event_bus(&self) -> &Arc<GatewayEventBus> {
        &self.event_bus
    }

    /// Get the current number of active connections
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Run the Gateway server
    ///
    /// This method runs indefinitely, accepting new connections and
    /// processing messages. Each connection is handled in its own task.
    pub async fn run(&self) -> Result<(), GatewayError> {
        let listener = TcpListener::bind(&self.addr).await.map_err(|e| {
            GatewayError::BindFailed {
                addr: self.addr,
                source: e,
            }
        })?;

        info!("Gateway listening on ws://{}", self.addr);

        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    // Check connection limit
                    if self.connections.read().await.len() >= self.config.max_connections {
                        warn!("Connection limit reached, rejecting {}", peer_addr);
                        continue;
                    }

                    let handlers = self.handlers.clone();
                    let event_bus = self.event_bus.clone();
                    let connections = self.connections.clone();
                    let require_auth = self.config.require_auth;

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(
                            stream,
                            peer_addr,
                            handlers,
                            event_bus,
                            connections,
                            require_auth,
                        )
                        .await
                        {
                            error!("Connection error from {}: {}", peer_addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Run the server with graceful shutdown support
    pub async fn run_until_shutdown(
        &self,
        shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), GatewayError> {
        let listener = TcpListener::bind(&self.addr).await.map_err(|e| {
            GatewayError::BindFailed {
                addr: self.addr,
                source: e,
            }
        })?;

        info!("Gateway listening on ws://{}", self.addr);

        tokio::select! {
            result = self.accept_loop(&listener) => result,
            _ = shutdown => {
                info!("Shutdown signal received, stopping gateway");
                Ok(())
            }
        }
    }

    async fn accept_loop(&self, listener: &TcpListener) -> Result<(), GatewayError> {
        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    if self.connections.read().await.len() >= self.config.max_connections {
                        warn!("Connection limit reached, rejecting {}", peer_addr);
                        continue;
                    }

                    let handlers = self.handlers.clone();
                    let event_bus = self.event_bus.clone();
                    let connections = self.connections.clone();
                    let require_auth = self.config.require_auth;

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(
                            stream,
                            peer_addr,
                            handlers,
                            event_bus,
                            connections,
                            require_auth,
                        )
                        .await
                        {
                            error!("Connection error from {}: {}", peer_addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }
}

/// Handle a single WebSocket connection
async fn handle_connection(
    stream: TcpStream,
    peer_addr: SocketAddr,
    handlers: Arc<HandlerRegistry>,
    event_bus: Arc<GatewayEventBus>,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    _require_auth: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_stream = accept_async(stream).await?;
    let (mut write, mut read) = ws_stream.split();
    let conn_id = format!("{}", peer_addr);

    info!("New WebSocket connection: {}", conn_id);

    // Initialize connection state
    {
        let mut conns = connections.write().await;
        conns.insert(conn_id.clone(), ConnectionState::new());
    }

    // Subscribe to event bus for this connection
    let mut event_rx = event_bus.subscribe();

    loop {
        tokio::select! {
            // Handle incoming messages
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!("Received from {}: {}", conn_id, &text[..text.len().min(200)]);
                        let response = process_request(&text, &handlers).await;
                        if let Err(e) = write.send(Message::Text(response.into())).await {
                            error!("Failed to send response to {}: {}", conn_id, e);
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        // Binary messages are not supported in JSON-RPC
                        warn!("Received unexpected binary message from {}: {} bytes", conn_id, data.len());
                    }
                    Some(Ok(Message::Ping(data))) => {
                        debug!("Received ping from {}", conn_id);
                        if let Err(e) = write.send(Message::Pong(data)).await {
                            error!("Failed to send pong: {}", e);
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        debug!("Received pong from {}", conn_id);
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!("Connection closed by {}: {:?}", conn_id, frame);
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => {
                        // Raw frames, usually not seen at this level
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error from {}: {}", conn_id, e);
                        break;
                    }
                    None => {
                        info!("Connection stream ended: {}", conn_id);
                        break;
                    }
                }
            }
            // Forward events to client
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        debug!("Forwarding event to {}", conn_id);
                        if let Err(e) = write.send(Message::Text(event.into())).await {
                            error!("Failed to send event to {}: {}", conn_id, e);
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Connection {} lagged, missed {} events", conn_id, n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        debug!("Event bus closed for {}", conn_id);
                        break;
                    }
                }
            }
        }
    }

    // Cleanup
    {
        let mut conns = connections.write().await;
        conns.remove(&conn_id);
    }

    info!("Connection closed: {}", conn_id);
    Ok(())
}

/// Process a JSON-RPC request string
async fn process_request(text: &str, handlers: &HandlerRegistry) -> String {
    // Parse the request
    let request: JsonRpcRequest = match serde_json::from_str(text) {
        Ok(req) => req,
        Err(e) => {
            return serde_json::to_string(&JsonRpcResponse::error(
                None,
                PARSE_ERROR,
                format!("Parse error: {}", e),
            ))
            .unwrap_or_default();
        }
    };

    // Validate the request
    if let Err(e) = request.validate() {
        return serde_json::to_string(&JsonRpcResponse::error(
            request.id.clone(),
            e.code,
            e.message,
        ))
        .unwrap_or_default();
    }

    // Dispatch to handler
    let response = handlers.handle(&request).await;
    serde_json::to_string(&response).unwrap_or_default()
}

/// Gateway server errors
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("Failed to bind to {addr}: {source}")]
    BindFailed {
        addr: SocketAddr,
        source: std::io::Error,
    },

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Protocol error: {0}")]
    ProtocolError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_process_valid_request() {
        let handlers = HandlerRegistry::new();
        let response = process_request(
            r#"{"jsonrpc":"2.0","method":"health","id":1}"#,
            &handlers,
        )
        .await;

        let parsed: JsonRpcResponse = serde_json::from_str(&response).unwrap();
        assert!(parsed.is_success());
    }

    #[tokio::test]
    async fn test_process_invalid_json() {
        let handlers = HandlerRegistry::new();
        let response = process_request("not json", &handlers).await;

        let parsed: JsonRpcResponse = serde_json::from_str(&response).unwrap();
        assert!(parsed.is_error());
        assert_eq!(parsed.error.unwrap().code, PARSE_ERROR);
    }

    #[tokio::test]
    async fn test_process_method_not_found() {
        let handlers = HandlerRegistry::empty();
        let response = process_request(
            r#"{"jsonrpc":"2.0","method":"unknown","id":1}"#,
            &handlers,
        )
        .await;

        let parsed: JsonRpcResponse = serde_json::from_str(&response).unwrap();
        assert!(parsed.is_error());
    }

    #[test]
    fn test_gateway_config_default() {
        let config = GatewayConfig::default();
        assert_eq!(config.max_connections, 1000);
        assert!(!config.require_auth);
    }
}
