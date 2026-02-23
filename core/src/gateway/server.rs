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

use super::protocol::{JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, PARSE_ERROR, TOOL_ERROR};
use super::event_bus::GatewayEventBus;
use super::ClientManifest;
use super::handlers::HandlerRegistry;
use super::handlers::events::{
    SubscriptionManager, handle_subscribe, handle_unsubscribe, handle_list as handle_events_list,
};
use super::handlers::debug::{parse_tool_call_params, DebugToolCallResult};
use super::reverse_rpc::ReverseRpcManager;
use crate::providers::protocols::ProtocolLoader;
use notify::RecommendedWatcher;
use notify_debouncer_full::{Debouncer, FileIdMap};

/// State for an individual WebSocket connection
pub struct ConnectionState {
    /// Whether the connection has been authenticated
    pub authenticated: bool,
    /// Whether this is the first message (for handshake enforcement)
    pub first_message: bool,
    /// Event topics this connection is subscribed to
    pub subscriptions: Vec<String>,
    /// Connection metadata
    pub metadata: HashMap<String, String>,
    /// Device ID (set after successful connect)
    pub device_id: Option<String>,
    /// Permissions (set after successful connect)
    pub permissions: Vec<String>,
    /// Client capability manifest (set during connect if provided)
    pub manifest: Option<ClientManifest>,
    /// Reverse RPC manager for Server-Client tool routing
    pub reverse_rpc: Option<Arc<ReverseRpcManager>>,
    /// Channel sender for sending requests to the client
    pub client_sender: Option<tokio::sync::mpsc::Sender<JsonRpcRequest>>,
    /// Guest session ID (set for guest connections)
    pub guest_session_id: Option<String>,
}

impl ConnectionState {
    /// Create a new connection state with Server-Client routing support
    fn with_routing(
        reverse_rpc: Arc<ReverseRpcManager>,
        client_sender: tokio::sync::mpsc::Sender<JsonRpcRequest>,
    ) -> Self {
        Self {
            authenticated: false,
            first_message: true,
            subscriptions: vec![],
            metadata: HashMap::new(),
            device_id: None,
            permissions: vec![],
            manifest: None,
            reverse_rpc: Some(reverse_rpc),
            client_sender: Some(client_sender),
            guest_session_id: None,
        }
    }

    /// Mark connection as authenticated
    pub fn authenticate(&mut self, device_id: String, permissions: Vec<String>) {
        self.authenticated = true;
        self.device_id = Some(device_id);
        self.permissions = permissions;
    }

    /// Set the client manifest after successful connect
    pub fn set_manifest(&mut self, manifest: ClientManifest) {
        self.manifest = Some(manifest);
    }

    /// Check if client supports a specific tool
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        self.manifest
            .as_ref()
            .map(|m| m.supports_tool(tool_name))
            .unwrap_or(false)
    }

    /// Check if client has a specific scope granted
    pub fn has_scope(&self, category: &str, scope: &str) -> bool {
        self.manifest
            .as_ref()
            .map(|m| m.has_scope(category, scope))
            .unwrap_or(true) // No manifest = all allowed
    }

    /// Check if this connection has a client manifest (supports routing)
    pub fn has_manifest(&self) -> bool {
        self.manifest.is_some()
    }

    /// Check if this connection supports Server-Client routing
    ///
    /// Returns true if the connection has all required components:
    /// - Client manifest (capability declaration)
    /// - Reverse RPC manager
    /// - Client sender channel
    pub fn supports_routing(&self) -> bool {
        self.manifest.is_some() && self.reverse_rpc.is_some() && self.client_sender.is_some()
    }

    /// Create a ClientContext for Server-Client routing
    ///
    /// Returns None if the connection doesn't have all required components.
    pub fn client_context(&self) -> Option<super::execution_engine::ClientContext> {
        let manifest = self.manifest.clone()?;
        let reverse_rpc = self.reverse_rpc.clone()?;
        let sender = self.client_sender.clone()?;
        Some(super::execution_engine::ClientContext::new(manifest, reverse_rpc, sender))
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
/// use alephcore::gateway::GatewayServer;
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
    /// Subscription manager for per-connection event filtering
    subscription_manager: Arc<SubscriptionManager>,
    /// Guest session manager for tracking guest connections
    guest_session_manager: Option<Arc<crate::gateway::security::GuestSessionManager>>,
    /// Protocol file watcher for hot-reload (None if watching disabled/failed)
    #[allow(dead_code)]
    protocol_watcher: Option<Debouncer<RecommendedWatcher, FileIdMap>>,
}

impl GatewayServer {
    /// Create a new Gateway server with default configuration
    pub fn new(addr: SocketAddr) -> Self {
        // Start protocol file watcher for hot-reload
        // If it fails (e.g., no ~/.aleph/protocols), log and continue without watching
        let protocol_watcher = match ProtocolLoader::start_watching() {
            Ok(watcher) => watcher,
            Err(e) => {
                warn!("Failed to start protocol watcher: {}", e);
                None
            }
        };

        Self {
            addr,
            config: GatewayConfig::default(),
            handlers: Arc::new(HandlerRegistry::new()),
            event_bus: Arc::new(GatewayEventBus::new()),
            connections: Arc::new(RwLock::new(HashMap::new())),
            subscription_manager: Arc::new(SubscriptionManager::new()),
            guest_session_manager: None,
            protocol_watcher,
        }
    }

    /// Create a Gateway server with custom configuration
    pub fn with_config(addr: SocketAddr, config: GatewayConfig) -> Self {
        // Start protocol file watcher for hot-reload
        let protocol_watcher = match ProtocolLoader::start_watching() {
            Ok(watcher) => watcher,
            Err(e) => {
                warn!("Failed to start protocol watcher: {}", e);
                None
            }
        };

        Self {
            addr,
            config,
            handlers: Arc::new(HandlerRegistry::new()),
            event_bus: Arc::new(GatewayEventBus::new()),
            connections: Arc::new(RwLock::new(HashMap::new())),
            subscription_manager: Arc::new(SubscriptionManager::new()),
            guest_session_manager: None,
            protocol_watcher,
        }
    }

    /// Get a reference to the subscription manager
    pub fn subscription_manager(&self) -> &Arc<SubscriptionManager> {
        &self.subscription_manager
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

    /// Set the guest session manager
    pub fn set_guest_session_manager(&mut self, manager: Arc<crate::gateway::security::GuestSessionManager>) {
        self.guest_session_manager = Some(manager);
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

                    let conn_ctx = ConnectionContext {
                        handlers: self.handlers.clone(),
                        event_bus: self.event_bus.clone(),
                        connections: self.connections.clone(),
                        subscription_manager: self.subscription_manager.clone(),
                        guest_session_manager: self.guest_session_manager.clone(),
                        require_auth: self.config.require_auth,
                    };

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, peer_addr, conn_ctx).await {
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

                    let conn_ctx = ConnectionContext {
                        handlers: self.handlers.clone(),
                        event_bus: self.event_bus.clone(),
                        connections: self.connections.clone(),
                        subscription_manager: self.subscription_manager.clone(),
                        guest_session_manager: self.guest_session_manager.clone(),
                        require_auth: self.config.require_auth,
                    };

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, peer_addr, conn_ctx).await {
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

/// Shared context for handling a WebSocket connection.
struct ConnectionContext {
    handlers: Arc<HandlerRegistry>,
    event_bus: Arc<GatewayEventBus>,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    subscription_manager: Arc<SubscriptionManager>,
    guest_session_manager: Option<Arc<crate::gateway::security::GuestSessionManager>>,
    require_auth: bool,
}

/// Handle a single WebSocket connection
async fn handle_connection(
    stream: TcpStream,
    peer_addr: SocketAddr,
    ctx: ConnectionContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_stream = accept_async(stream).await?;
    let (mut write, mut read) = ws_stream.split();
    let conn_id = format!("{}", peer_addr);

    info!("New WebSocket connection: {}", conn_id);

    // Subscribe to event bus for this connection
    let mut event_rx = ctx.event_bus.subscribe();

    // Create reverse RPC manager for this connection
    let reverse_rpc = Arc::new(ReverseRpcManager::new());

    // Create channel for sending requests to the client (for Server-Client routing)
    // This allows ExecutionEngine to send tool.call requests to the client
    let (client_tx, mut client_rx) = tokio::sync::mpsc::channel::<JsonRpcRequest>(32);

    // Initialize connection state with routing support
    {
        let mut conns = ctx.connections.write().await;
        conns.insert(
            conn_id.clone(),
            ConnectionState::with_routing(reverse_rpc.clone(), client_tx.clone()),
        );
    }

    loop {
        tokio::select! {
            // Handle incoming messages
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!("Received from {}: {}", conn_id, &text[..text.len().min(200)]);

                        // First, check if this is a response to a reverse RPC call
                        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&text) {
                            if reverse_rpc.handle_response(response) {
                                debug!("Handled reverse RPC response from {}", conn_id);
                                continue;
                            }
                        }

                        // Parse request to check method for auth gating
                        let request: Result<JsonRpcRequest, _> = serde_json::from_str(&text);

                        let response = match request {
                            Ok(ref req) => {
                                // Check authentication requirement
                                let (is_first, is_authenticated) = {
                                    let conns = ctx.connections.read().await;
                                    let state = conns.get(&conn_id);
                                    (
                                        state.is_none_or(|s| s.first_message),
                                        state.is_some_and(|s| s.authenticated),
                                    )
                                };

                                // Auth gating logic
                                if ctx.require_auth && !is_authenticated {
                                    // First message must be "connect"
                                    if is_first && req.method != "connect" {
                                        warn!(
                                            "Connection {} rejected: first request must be 'connect' (got '{}')",
                                            conn_id, req.method
                                        );
                                        let response = JsonRpcResponse::error(
                                            req.id.clone(),
                                            AUTH_REQUIRED,
                                            "Authentication required: first request must be 'connect'",
                                        );
                                        let response_str = serde_json::to_string(&response).unwrap_or_default();
                                        let _ = write.send(Message::Text(response_str.into())).await;
                                        // Close connection after auth failure
                                        break;
                                    }

                                    // Non-connect requests require authentication
                                    if !is_first && req.method != "connect" {
                                        warn!(
                                            "Connection {} rejected: not authenticated (method: '{}')",
                                            conn_id, req.method
                                        );
                                        serde_json::to_string(&JsonRpcResponse::error(
                                            req.id.clone(),
                                            AUTH_REQUIRED,
                                            "Authentication required",
                                        ))
                                        .unwrap_or_default()
                                    } else {
                                        // Handle connect request
                                        let response = process_request(&text, &ctx.handlers).await;

                                        // If connect succeeded, mark as authenticated
                                        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&response) {
                                            debug!("Parsed connect response: success={}, method={}", resp.is_success(), req.method);
                                            if resp.is_success() && req.method == "connect" {
                                                debug!("Connect succeeded, extracting device_id and permissions");
                                                // Extract device_id and permissions from result
                                                let device_id = resp.result
                                                    .as_ref()
                                                    .and_then(|r| r.get("device_id"))
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("unknown")
                                                    .to_string();
                                                let permissions = resp.result
                                                    .as_ref()
                                                    .and_then(|r| r.get("permissions"))
                                                    .and_then(|v| v.as_array())
                                                    .map(|arr| {
                                                        arr.iter()
                                                            .filter_map(|v| v.as_str().map(String::from))
                                                            .collect()
                                                    })
                                                    .unwrap_or_default();

                                                // Extract guest_session_id if this is a guest token
                                                let guest_session_id = resp.result
                                                    .as_ref()
                                                    .and_then(|r| r.get("token"))
                                                    .and_then(|v| v.as_str())
                                                    .and_then(|token| {
                                                        debug!("Extracting guest_session_id from token: {}", token);
                                                        // Guest tokens have format: guest:{session_id}:{token}
                                                        if token.starts_with("guest:") {
                                                            let session_id = token.split(':').nth(1).map(String::from);
                                                            debug!("Extracted guest_session_id: {:?}", session_id);
                                                            session_id
                                                        } else {
                                                            debug!("Token does not start with 'guest:'");
                                                            None
                                                        }
                                                    });

                                                let mut conns = ctx.connections.write().await;
                                                if let Some(state) = conns.get_mut(&conn_id) {
                                                    state.authenticate(device_id.clone(), permissions);
                                                    state.guest_session_id = guest_session_id.clone();
                                                    state.first_message = false;
                                                    if let Some(ref session_id) = guest_session_id {
                                                        info!("Connection {} authenticated as guest (session: {})", conn_id, session_id);
                                                    } else {
                                                        info!("Connection {} authenticated (device: {})", conn_id, device_id);
                                                    }
                                                }
                                            }
                                        }

                                        // Mark first_message = false even if connect failed
                                        {
                                            let mut conns = ctx.connections.write().await;
                                            if let Some(state) = conns.get_mut(&conn_id) {
                                                state.first_message = false;
                                            }
                                        }

                                        response
                                    }
                                } else {
                                    // No auth required OR already authenticated
                                    // Handle events.* methods specially (they need conn_id)
                                    if req.method == "events.subscribe" {
                                        let resp = handle_subscribe(req.clone(), &conn_id, ctx.subscription_manager.clone()).await;
                                        serde_json::to_string(&resp).unwrap_or_default()
                                    } else if req.method == "events.unsubscribe" {
                                        let resp = handle_unsubscribe(req.clone(), &conn_id, ctx.subscription_manager.clone()).await;
                                        serde_json::to_string(&resp).unwrap_or_default()
                                    } else if req.method == "events.list" {
                                        let resp = handle_events_list(req.clone(), &conn_id, ctx.subscription_manager.clone()).await;
                                        serde_json::to_string(&resp).unwrap_or_default()
                                    } else if req.method == "debug.tool_call" {
                                        // Handle debug.tool_call - sends reverse RPC to client
                                        let resp = handle_debug_tool_call(
                                            req.clone(),
                                            &mut write,
                                            reverse_rpc.clone(),
                                        ).await;
                                        serde_json::to_string(&resp).unwrap_or_default()
                                    } else {
                                        let response = process_request(&text, &ctx.handlers).await;

                                        // Extract guest_session_id from connect response (when require_auth=false)
                                        if req.method == "connect" {
                                            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&response) {
                                                if resp.is_success() {
                                                    let guest_session_id = resp.result
                                                        .as_ref()
                                                        .and_then(|r| r.get("token"))
                                                        .and_then(|v| v.as_str())
                                                        .and_then(|token| {
                                                            debug!("Extracting guest_session_id from token: {}", token);
                                                            // Guest tokens have format: guest:{session_id}:{token}
                                                            if token.starts_with("guest:") {
                                                                let session_id = token.split(':').nth(1).map(String::from);
                                                                debug!("Extracted guest_session_id: {:?}", session_id);
                                                                session_id
                                                            } else {
                                                                debug!("Token does not start with 'guest:'");
                                                                None
                                                            }
                                                        });

                                                    if let Some(session_id) = guest_session_id {
                                                        let mut conns = ctx.connections.write().await;
                                                        if let Some(state) = conns.get_mut(&conn_id) {
                                                            state.guest_session_id = Some(session_id.clone());
                                                            info!("Connection {} authenticated as guest (session: {})", conn_id, session_id);
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // Log RPC request for guest sessions
                                        if let Some(ref gsm) = ctx.guest_session_manager {
                                            let conns = ctx.connections.read().await;
                                            if let Some(state) = conns.get(&conn_id) {
                                                debug!("Checking for guest_session_id in connection state: {:?}", state.guest_session_id);
                                                if let Some(ref session_id) = state.guest_session_id {
                                                    debug!("Found guest_session_id: {}, looking up session", session_id);
                                                    if let Some(session) = gsm.get_session(session_id) {
                                                        debug!("Found guest session, logging RPC request: {}", req.method);
                                                        // Parse response to determine status
                                                        let status = if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&response) {
                                                            if resp.is_success() {
                                                                crate::gateway::security::ActivityStatus::Success
                                                            } else {
                                                                crate::gateway::security::ActivityStatus::Failed
                                                            }
                                                        } else {
                                                            crate::gateway::security::ActivityStatus::Failed
                                                        };

                                                        gsm.activity_logger().log_rpc_request(
                                                            session_id.clone(),
                                                            session.guest_id.clone(),
                                                            req.method.clone(),
                                                            serde_json::json!({
                                                                "params": req.params,
                                                            }),
                                                            status,
                                                            None,
                                                        );
                                                    }
                                                }
                                            }
                                        }

                                        response
                                    }
                                }
                            }
                            Err(e) => {
                                serde_json::to_string(&JsonRpcResponse::error(
                                    None,
                                    PARSE_ERROR,
                                    format!("Parse error: {}", e),
                                ))
                                .unwrap_or_default()
                            }
                        };

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
            // Forward events to client (with subscription filtering)
            event = event_rx.recv() => {
                match event {
                    Ok(event_json) => {
                        // Try to extract topic from event for filtering
                        let should_forward = if let Ok(event_obj) = serde_json::from_str::<serde_json::Value>(&event_json) {
                            // Check for topic in event (TopicEvent format)
                            let topic = event_obj.get("topic")
                                .and_then(|t| t.as_str())
                                // Or method for JSON-RPC notification format
                                .or_else(|| event_obj.get("method").and_then(|m| m.as_str()))
                                .unwrap_or("");

                            ctx.subscription_manager.should_receive(&conn_id, topic).await
                        } else {
                            // Can't parse event, forward by default
                            true
                        };

                        if should_forward {
                            debug!("Forwarding event to {}", conn_id);
                            if let Err(e) = write.send(Message::Text(event_json.into())).await {
                                error!("Failed to send event to {}: {}", conn_id, e);
                                break;
                            }
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
            // Forward requests to client (for Server-Client reverse RPC)
            // This handles tool.call requests from ExecutionEngine
            request = client_rx.recv() => {
                match request {
                    Some(req) => {
                        debug!("Sending request to client {}: {}", conn_id, req.method);
                        match serde_json::to_string(&req) {
                            Ok(json) => {
                                if let Err(e) = write.send(Message::Text(json.into())).await {
                                    error!("Failed to send request to {}: {}", conn_id, e);
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Failed to serialize request for {}: {}", conn_id, e);
                            }
                        }
                    }
                    None => {
                        // Channel closed, this is normal during shutdown
                        debug!("Client request channel closed for {}", conn_id);
                    }
                }
            }
        }
    }

    // Cleanup
    {
        let mut conns = ctx.connections.write().await;

        // Check if this was a guest session and terminate it
        if let Some(state) = conns.get(&conn_id) {
            if let Some(ref session_id) = state.guest_session_id {
                if let Some(ref manager) = ctx.guest_session_manager {
                    info!("Terminating guest session: {}", session_id);

                    // Get session details before terminating
                    if let Some(session) = manager.get_session(session_id) {
                        // Terminate the session
                        if let Err(e) = manager.terminate_session(session_id) {
                            warn!("Failed to terminate guest session {}: {}", session_id, e);
                        }

                        // Emit disconnection event
                        let event = crate::gateway::event_bus::TopicEvent {
                            topic: "guest.session.disconnected".to_string(),
                            data: serde_json::json!({
                                "session_id": session.session_id,
                                "guest_id": session.guest_id,
                                "guest_name": session.guest_name,
                                "connected_at": session.connected_at,
                                "disconnected_at": std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_millis() as u64,
                                "request_count": session.request_count,
                            }),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64,
                        };
                        let _ = ctx.event_bus.publish_json(&event);
                    }
                }
            }
        }

        conns.remove(&conn_id);
    }

    // Remove subscriptions for this connection
    ctx.subscription_manager.remove_connection(&conn_id).await;

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

/// Handle debug.tool_call - sends a reverse RPC to the client
///
/// This is a test endpoint for validating the Server-Client reverse RPC mechanism.
/// It sends a tool.call request to the connected client and waits for the response.
async fn handle_debug_tool_call<S>(
    request: JsonRpcRequest,
    write: &mut S,
    reverse_rpc: Arc<ReverseRpcManager>,
) -> JsonRpcResponse
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    use std::time::Instant;
    use serde_json::json;

    // Parse parameters
    let params = match parse_tool_call_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    info!(
        tool = %params.tool,
        timeout_ms = params.timeout_ms,
        "debug.tool_call: sending reverse RPC to client"
    );

    let start = Instant::now();

    // Create reverse RPC request
    let (rpc_request, pending) = reverse_rpc.create_request(
        "tool.call",
        json!({
            "tool": params.tool,
            "args": params.args,
        }),
    );

    // Send request to client
    let request_json = match serde_json::to_string(&rpc_request) {
        Ok(j) => j,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                TOOL_ERROR,
                format!("Failed to serialize tool.call request: {}", e),
            );
        }
    };

    debug!("Sending tool.call to client: {}", request_json);

    if let Err(e) = write.send(Message::Text(request_json.into())).await {
        return JsonRpcResponse::error(
            request.id,
            TOOL_ERROR,
            format!("Failed to send tool.call to client: {}", e),
        );
    }

    // Wait for response with timeout
    let timeout = std::time::Duration::from_millis(params.timeout_ms);
    let result = pending.wait_timeout(timeout).await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(value) => {
            info!(
                tool = %params.tool,
                duration_ms = duration_ms,
                "debug.tool_call: received response from client"
            );

            let debug_result = DebugToolCallResult {
                success: true,
                result: Some(value),
                error: None,
                duration_ms,
                executed_on: "client".to_string(),
            };

            JsonRpcResponse::success(request.id, json!(debug_result))
        }
        Err(e) => {
            error!(
                tool = %params.tool,
                error = %e,
                "debug.tool_call: failed"
            );

            let debug_result = DebugToolCallResult {
                success: false,
                result: None,
                error: Some(e.to_string()),
                duration_ms,
                executed_on: "client".to_string(),
            };

            JsonRpcResponse::success(request.id, json!(debug_result))
        }
    }
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
