//! WebSocket Gateway Server
//!
//! Handles WebSocket connections and dispatches JSON-RPC 2.0 requests
//! to registered handlers.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use futures_util::{StreamExt, SinkExt};
use tracing::{info, warn, error, debug};
use axum::{
    Router,
    routing::get,
    extract::{State, ConnectInfo, ws::{WebSocket, WebSocketUpgrade, Message as WsMessage}},
    response::IntoResponse,
};
use super::control_plane::create_control_plane_router;
use super::openai_api::routes::{openai_routes, OpenAiApiState};

use super::protocol::{JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, PARSE_ERROR, RATE_LIMITED, INTERNAL_ERROR};
use super::event_bus::{GatewayEventBus, TopicEvent};
use super::handlers::HandlerRegistry;
use super::handlers::events::{
    SubscriptionManager, handle_subscribe, handle_unsubscribe, handle_list as handle_events_list,
};
use super::presence::{PresenceTracker, PresenceEntry};
use super::state_version::StateVersionTracker;
use super::rate_limiter::{RateLimiter, RateLimitKey, RateLimitConfig, scope_for_method, RateLimitError};
use super::lane::{LaneManager, LaneConfig};
use super::event_scope::EventScopeGuard;
use super::config::AuthMode;
use crate::providers::protocols::ProtocolLoader;
use notify::RecommendedWatcher;
use notify_debouncer_full::{Debouncer, FileIdMap};

/// Maximum number of failed authentication attempts before disconnecting
const MAX_AUTH_ATTEMPTS: u8 = 5;

/// State for an individual WebSocket connection
pub struct ConnectionState {
    /// Whether the connection has been authenticated
    pub authenticated: bool,
    /// Whether this is the first message (for handshake enforcement)
    pub first_message: bool,
    /// Number of failed authentication attempts
    pub auth_attempts: u8,
    /// Event topics this connection is subscribed to
    pub subscriptions: Vec<String>,
    /// Connection metadata
    pub metadata: HashMap<String, String>,
    /// Device ID (set after successful connect)
    pub device_id: Option<String>,
    /// Permissions (set after successful connect)
    pub permissions: Vec<String>,
    /// Guest session ID (set for guest connections)
    pub guest_session_id: Option<String>,
}

impl ConnectionState {
    /// Create a new connection state
    fn new() -> Self {
        Self {
            authenticated: false,
            first_message: true,
            auth_attempts: 0,
            subscriptions: vec![],
            metadata: HashMap::new(),
            device_id: None,
            permissions: vec![],
            guest_session_id: None,
        }
    }

    /// Mark connection as authenticated
    pub fn authenticate(&mut self, device_id: String, permissions: Vec<String>) {
        self.authenticated = true;
        self.device_id = Some(device_id);
        self.permissions = permissions;
    }
}

/// Shared state for the unified axum server (WebSocket + ControlPlane)
#[derive(Clone)]
pub struct GatewaySharedState {
    pub handlers: Arc<HandlerRegistry>,
    pub event_bus: Arc<GatewayEventBus>,
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    pub subscription_manager: Arc<SubscriptionManager>,
    pub guest_session_manager: Option<Arc<crate::gateway::security::GuestSessionManager>>,
    pub auth_mode: AuthMode,
    pub max_connections: usize,
    pub presence: Arc<PresenceTracker>,
    pub state_versions: Arc<StateVersionTracker>,
    pub rate_limiter: Arc<RateLimiter>,
    pub lane_manager: Arc<LaneManager>,
    pub event_scope_guard: Arc<EventScopeGuard>,
}

/// Configuration for the Gateway server
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Authentication mode
    pub auth_mode: AuthMode,
    /// Connection timeout in seconds
    pub timeout_secs: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            max_connections: 1000,
            auth_mode: AuthMode::default(),
            timeout_secs: 300,
        }
    }
}

/// Unified Gateway Server
///
/// Serves WebSocket connections at `/ws` and the ControlPlane UI as fallback,
/// dispatching JSON-RPC 2.0 requests to registered handlers.
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::gateway::GatewayServer;
/// use std::net::SocketAddr;
///
/// #[tokio::main]
/// async fn main() {
///     let addr: SocketAddr = "127.0.0.1:18790".parse().unwrap();
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
    /// Protocol file watcher for hot-reload (None if watching disabled/failed).
    /// Held for ownership: dropping the Debouncer stops the watcher.
    #[allow(dead_code)]
    protocol_watcher: Option<Debouncer<RecommendedWatcher, FileIdMap>>,
    /// Presence tracker for connected device awareness
    pub presence: Arc<PresenceTracker>,
    /// Monotonic version tracker for state change detection
    pub state_versions: Arc<StateVersionTracker>,
    /// Per-identity, per-scope sliding window rate limiter
    pub rate_limiter: Arc<RateLimiter>,
    /// Lane-based concurrency control for RPC methods
    pub lane_manager: Arc<LaneManager>,
    /// Permission-based event scope guard
    pub event_scope_guard: Arc<EventScopeGuard>,
    /// Server start time for uptime calculation
    pub start_time: Instant,
    /// Optional A2A server state (set during startup if A2A is enabled)
    a2a_state: Option<Arc<crate::a2a::adapter::server::A2AServerState>>,
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
            presence: Arc::new(PresenceTracker::new()),
            state_versions: Arc::new(StateVersionTracker::new()),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig::default())),
            lane_manager: Arc::new(LaneManager::new(LaneConfig::default())),
            event_scope_guard: Arc::new(EventScopeGuard::default_rules()),
            start_time: Instant::now(),
            a2a_state: None,
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
            presence: Arc::new(PresenceTracker::new()),
            state_versions: Arc::new(StateVersionTracker::new()),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig::default())),
            lane_manager: Arc::new(LaneManager::new(LaneConfig::default())),
            event_scope_guard: Arc::new(EventScopeGuard::default_rules()),
            start_time: Instant::now(),
            a2a_state: None,
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

    /// Set the A2A server state (enables A2A routes in build_router)
    pub fn set_a2a_state(&mut self, state: Arc<crate::a2a::adapter::server::A2AServerState>) {
        self.a2a_state = Some(state);
    }

    /// Get the current number of active connections
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Build a unified axum Router with WebSocket + ControlPlane UI routes.
    /// WebSocket connections are handled at `/ws`, everything else serves the Panel UI.
    pub fn build_router(&self) -> Router {
        let shared = Arc::new(GatewaySharedState {
            handlers: self.handlers.clone(),
            event_bus: self.event_bus.clone(),
            connections: self.connections.clone(),
            subscription_manager: self.subscription_manager.clone(),
            guest_session_manager: self.guest_session_manager.clone(),
            auth_mode: self.config.auth_mode.clone(),
            max_connections: self.config.max_connections,
            presence: self.presence.clone(),
            state_versions: self.state_versions.clone(),
            rate_limiter: self.rate_limiter.clone(),
            lane_manager: self.lane_manager.clone(),
            event_scope_guard: self.event_scope_guard.clone(),
        });

        let control_plane = create_control_plane_router();

        // OpenAI-compatible API routes (/v1/models, /v1/health, /v1/chat/completions)
        let openai_state = Arc::new(OpenAiApiState {
            server_id: format!("aleph-{}", self.addr),
            api_token: None, // Auth handled by HTTP middleware layer
        });
        let openai = openai_routes(openai_state);

        let mut router = Router::new()
            .route("/ws", get(ws_upgrade_handler))
            .fallback_service(control_plane)
            .with_state(shared)
            .merge(openai);

        // Merge A2A routes if the subsystem is enabled
        if let Some(a2a_state) = &self.a2a_state {
            let a2a = crate::a2a::adapter::server::a2a_routes(a2a_state.clone());
            router = router.merge(a2a);
        }

        router
    }

    /// Spawn background tasks for rate limiter pruning and tick heartbeat.
    ///
    /// Call this once before the server starts accepting connections.
    /// The spawned tasks run until the tokio runtime shuts down.
    fn spawn_background_tasks(&self) {
        // Background: prune stale rate-limiter entries every 60s
        let rl = self.rate_limiter.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                rl.prune_stale(Duration::from_secs(300));
            }
        });

        // Background: tick heartbeat every 10s
        let eb = self.event_bus.clone();
        let sv = self.state_versions.clone();
        let pr = self.presence.clone();
        let st = self.start_time;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let _ = eb.publish_json(&TopicEvent::new("system.tick", serde_json::json!({
                    "ts": chrono::Utc::now().timestamp_millis(),
                    "state_version": sv.snapshot(),
                    "connections": pr.count(),
                    "uptime_ms": st.elapsed().as_millis() as u64,
                })));
            }
        });
    }

    /// Gracefully shut down the gateway: notify all clients, then wait for a grace period.
    pub async fn graceful_shutdown(&self, reason: &str) {
        info!("Gateway graceful shutdown: {reason}");
        let event = TopicEvent::new(
            "system.shutdown",
            serde_json::json!({"reason": reason, "grace_period_ms": 5000}),
        );
        let _ = self.event_bus.publish_json(&event);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    /// Run the Gateway server
    ///
    /// This method runs indefinitely, accepting new connections and
    /// processing messages. Each connection is handled in its own task.
    pub async fn run(&self) -> Result<(), GatewayError> {
        self.spawn_background_tasks();
        let router = self.build_router();
        let listener = tokio::net::TcpListener::bind(&self.addr).await.map_err(|e| {
            GatewayError::BindFailed { addr: self.addr, source: e }
        })?;
        info!("Aleph listening on http://{}", self.addr);
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;
        Ok(())
    }

    /// Run the server with graceful shutdown support
    pub async fn run_until_shutdown(
        &self,
        shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), GatewayError> {
        self.spawn_background_tasks();
        let router = self.build_router();
        let listener = tokio::net::TcpListener::bind(&self.addr).await.map_err(|e| {
            GatewayError::BindFailed { addr: self.addr, source: e }
        })?;
        info!("Aleph listening on http://{}", self.addr);
        info!("  WebSocket: ws://{}/ws", self.addr);
        info!("  Panel UI:  http://{}/", self.addr);
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async { let _ = shutdown.await; })
        .await
        .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;
        Ok(())
    }
}

/// Shared context for handling a WebSocket connection.
struct ConnectionContext {
    handlers: Arc<HandlerRegistry>,
    event_bus: Arc<GatewayEventBus>,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    subscription_manager: Arc<SubscriptionManager>,
    guest_session_manager: Option<Arc<crate::gateway::security::GuestSessionManager>>,
    auth_mode: AuthMode,
    presence: Arc<PresenceTracker>,
    state_versions: Arc<StateVersionTracker>,
    rate_limiter: Arc<RateLimiter>,
    lane_manager: Arc<LaneManager>,
    event_scope_guard: Arc<EventScopeGuard>,
}

/// axum handler: upgrade HTTP connection to WebSocket at `/ws`
async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<GatewaySharedState>>,
) -> axum::response::Response {
    // Check connection limit before upgrading
    let current = state.connections.read().await.len();
    if current >= state.max_connections {
        warn!("Connection limit reached, rejecting {}", peer_addr);
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Connection limit reached").into_response();
    }

    ws.on_upgrade(move |socket| async move {
        let ctx = ConnectionContext {
            handlers: state.handlers.clone(),
            event_bus: state.event_bus.clone(),
            connections: state.connections.clone(),
            subscription_manager: state.subscription_manager.clone(),
            guest_session_manager: state.guest_session_manager.clone(),
            auth_mode: state.auth_mode.clone(),
            presence: state.presence.clone(),
            state_versions: state.state_versions.clone(),
            rate_limiter: state.rate_limiter.clone(),
            lane_manager: state.lane_manager.clone(),
            event_scope_guard: state.event_scope_guard.clone(),
        };
        if let Err(e) = handle_connection(socket, peer_addr, ctx).await {
            error!("Connection error from {}: {}", peer_addr, e);
        }
    })
}

/// Handle a single WebSocket connection
async fn handle_connection(
    socket: WebSocket,
    peer_addr: SocketAddr,
    ctx: ConnectionContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut write, mut read) = socket.split();
    let conn_id = format!("{}", peer_addr);

    info!("New WebSocket connection: {}", conn_id);

    // Subscribe to event bus for this connection
    let mut event_rx = ctx.event_bus.subscribe();

    // Initialize connection state
    {
        let mut conns = ctx.connections.write().await;
        conns.insert(conn_id.clone(), ConnectionState::new());
    }

    loop {
        tokio::select! {
            // Handle incoming messages
            msg = read.next() => {
                match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        let preview_end = text.char_indices().take_while(|(i, _)| *i < 200).last().map(|(i, c)| i + c.len_utf8()).unwrap_or(text.len());
                        debug!("Received from {}: {}", conn_id, &text[..preview_end]);

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
                                if ctx.auth_mode.is_auth_required() && !is_authenticated {
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
                                        let _ = write.send(WsMessage::Text(response_str.into())).await;
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

                                                    // Track presence after successful auth
                                                    let presence_entry = PresenceEntry {
                                                        conn_id: conn_id.clone(),
                                                        device_id: state.device_id.clone(),
                                                        device_name: state.metadata.get("client_name").cloned().unwrap_or_else(|| "Unknown".to_string()),
                                                        platform: state.metadata.get("platform").cloned().unwrap_or_else(|| "unknown".to_string()),
                                                        connected_at: chrono::Utc::now(),
                                                        last_heartbeat: chrono::Utc::now(),
                                                    };
                                                    ctx.presence.upsert(conn_id.clone(), presence_entry);
                                                    ctx.state_versions.bump_presence();
                                                    let _ = ctx.event_bus.publish_json(&TopicEvent::new("presence.joined", serde_json::json!({"conn_id": &conn_id})));
                                                }
                                            }
                                        }

                                        // Mark first_message = false even if connect failed
                                        // Track failed auth attempts and disconnect if limit reached
                                        {
                                            let mut conns = ctx.connections.write().await;
                                            if let Some(state) = conns.get_mut(&conn_id) {
                                                state.first_message = false;
                                                if !state.authenticated {
                                                    state.auth_attempts += 1;
                                                    if state.auth_attempts >= MAX_AUTH_ATTEMPTS {
                                                        warn!(
                                                            "Connection {} exceeded max auth attempts ({}), disconnecting",
                                                            conn_id, MAX_AUTH_ATTEMPTS
                                                        );
                                                        let response_str = serde_json::to_string(&JsonRpcResponse::error(
                                                            req.id.clone(),
                                                            AUTH_REQUIRED,
                                                            "Too many failed authentication attempts",
                                                        )).unwrap_or_default();
                                                        drop(conns);
                                                        let _ = write.send(WsMessage::Text(response_str.into())).await;
                                                        break;
                                                    }
                                                }
                                            }
                                        }

                                        response
                                    }
                                } else {
                                    // No auth required OR already authenticated

                                    // --- Rate limit check ---
                                    let peer_addr_str = peer_addr.to_string();
                                    let rl_identity = {
                                        let conns = ctx.connections.read().await;
                                        conns.get(&conn_id)
                                            .and_then(|s| s.device_id.clone())
                                            .unwrap_or_else(|| peer_addr_str.clone())
                                    };
                                    let rl_scope = scope_for_method(&req.method);
                                    let rl_key = RateLimitKey::new(&rl_identity, rl_scope);
                                    if let Err(e) = ctx.rate_limiter.check_and_record(&rl_key) {
                                        let rl_response = match e {
                                            RateLimitError::Exceeded { retry_after_ms, .. } => {
                                                JsonRpcResponse::error_with_data(
                                                    req.id.clone(),
                                                    RATE_LIMITED,
                                                    "Rate limit exceeded",
                                                    serde_json::json!({"retry_after_ms": retry_after_ms}),
                                                )
                                            }
                                            RateLimitError::LockedOut { lockout_remaining_ms, .. } => {
                                                JsonRpcResponse::error_with_data(
                                                    req.id.clone(),
                                                    RATE_LIMITED,
                                                    "Rate limit lockout",
                                                    serde_json::json!({"lockout_remaining_ms": lockout_remaining_ms}),
                                                )
                                            }
                                        };
                                        let rl_resp_str = serde_json::to_string(&rl_response).unwrap_or_default();
                                        if let Err(e) = write.send(WsMessage::Text(rl_resp_str.into())).await {
                                            error!("Failed to send rate limit response to {}: {}", conn_id, e);
                                            break;
                                        }
                                        continue;
                                    }

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
                                    } else {
                                        // --- Lane concurrency control ---
                                        let lane_result = ctx.lane_manager.acquire(&req.method).await;
                                        let response = match lane_result {
                                            Ok(_permit) => {
                                                let resp = process_request(&text, &ctx.handlers).await;
                                                // permit drops here, releasing the lane slot
                                                resp
                                            }
                                            Err(_) => {
                                                serde_json::to_string(&JsonRpcResponse::error(
                                                    req.id.clone(),
                                                    INTERNAL_ERROR,
                                                    "Service congested, try again later",
                                                )).unwrap_or_default()
                                            }
                                        };

                                        // Extract guest_session_id from connect response
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

                                                    // Track presence for no-auth connect
                                                    let conns = ctx.connections.read().await;
                                                    if let Some(state) = conns.get(&conn_id) {
                                                        let presence_entry = PresenceEntry {
                                                            conn_id: conn_id.clone(),
                                                            device_id: state.device_id.clone(),
                                                            device_name: state.metadata.get("client_name").cloned().unwrap_or_else(|| "Unknown".to_string()),
                                                            platform: state.metadata.get("platform").cloned().unwrap_or_else(|| "unknown".to_string()),
                                                            connected_at: chrono::Utc::now(),
                                                            last_heartbeat: chrono::Utc::now(),
                                                        };
                                                        drop(conns);
                                                        ctx.presence.upsert(conn_id.clone(), presence_entry);
                                                        ctx.state_versions.bump_presence();
                                                        let _ = ctx.event_bus.publish_json(&TopicEvent::new("presence.joined", serde_json::json!({"conn_id": &conn_id})));
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

                        if let Err(e) = write.send(WsMessage::Text(response.into())).await {
                            error!("Failed to send response to {}: {}", conn_id, e);
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Binary(data))) => {
                        // Binary messages are not supported in JSON-RPC
                        warn!("Received unexpected binary message from {}: {} bytes", conn_id, data.len());
                    }
                    Some(Ok(WsMessage::Ping(data))) => {
                        debug!("Received ping from {}", conn_id);
                        if let Err(e) = write.send(WsMessage::Pong(data)).await {
                            error!("Failed to send pong: {}", e);
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Pong(_))) => {
                        debug!("Received pong from {}", conn_id);
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        info!("Connection closed by {}: {:?}", conn_id, frame);
                        break;
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

                            // Permission-based scope guard check
                            let scope_allowed = {
                                let conns = ctx.connections.read().await;
                                conns.get(&conn_id)
                                    .map(|s| ctx.event_scope_guard.can_receive(topic, &s.permissions))
                                    .unwrap_or(false)
                            };

                            scope_allowed && ctx.subscription_manager.should_receive(&conn_id, topic).await
                        } else {
                            // Can't parse event, forward by default
                            true
                        };

                        if should_forward {
                            debug!("Forwarding event to {}", conn_id);
                            if let Err(e) = write.send(WsMessage::Text(event_json.into())).await {
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
                                    .unwrap_or_default()
                                    .as_millis() as u64,
                                "request_count": session.request_count,
                            }),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        };
                        let _ = ctx.event_bus.publish_json(&event);
                    }
                }
            }
        }

        conns.remove(&conn_id);
    }

    // Remove presence and emit departure event
    if let Some(_entry) = ctx.presence.remove(&conn_id) {
        ctx.state_versions.bump_presence();
        let _ = ctx.event_bus.publish_json(&TopicEvent::new("presence.left", serde_json::json!({"conn_id": &conn_id})));
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
        assert_eq!(config.auth_mode, AuthMode::Token);
    }
}
