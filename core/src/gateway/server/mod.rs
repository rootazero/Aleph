//! WebSocket Gateway Server
//!
//! Handles WebSocket connections and dispatches JSON-RPC 2.0 requests
//! to registered handlers.

mod handler;

use super::control_plane::create_control_plane_router;
use super::openai_api::{openai_routes, OpenAiApiState};
use crate::sync_primitives::Arc;
use axum::{routing::get, Router};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::config::AuthMode;
use super::event_bus::{GatewayEventBus, TopicEvent};
use super::event_scope::EventScopeGuard;
use super::handlers::events::SubscriptionManager;
use super::handlers::HandlerRegistry;
use super::lane::{LaneConfig, LaneManager};
use super::presence::PresenceTracker;
use super::rate_limiter::{RateLimitConfig, RateLimiter};
use super::state_version::StateVersionTracker;
use crate::providers::protocols::ProtocolLoader;
use crate::security::headers::SecurityHeadersLayer;
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
    pub idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
    pub event_scope_guard: Arc<EventScopeGuard>,
    pub audit_log: Option<crate::security::audit::SecurityAuditLog>,
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
    /// Idempotency guard for preventing duplicate RPC execution
    pub idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
    /// Permission-based event scope guard
    pub event_scope_guard: Arc<EventScopeGuard>,
    /// Server start time for uptime calculation
    pub start_time: Instant,
    /// Optional A2A server state (set during startup if A2A is enabled)
    a2a_state: Option<Arc<crate::a2a::adapter::server::A2AServerState>>,
    /// Execution adapter for OpenAI-compatible agent completions
    pub execution_adapter: Option<Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>>,
    /// Agent registry for OpenAI-compatible agent completions
    pub openai_agent_registry: Option<Arc<crate::gateway::agent_instance::AgentRegistry>>,
    /// Model → HttpProvider map for passthrough completions
    pub openai_provider_map:
        Arc<HashMap<String, Arc<crate::providers::http_provider::HttpProvider>>>,
    /// Provider configs for /v1/models listing
    pub openai_provider_configs: Vec<(String, crate::config::ProviderConfig)>,
    /// Embedding provider for /v1/embeddings
    pub embedding_provider: Option<Arc<dyn crate::memory::EmbeddingProvider>>,
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
            idempotency_guard: Arc::new(crate::gateway::idempotency::IdempotencyGuard::new(
                std::time::Duration::from_secs(300), // 5 minute TTL
            )),
            event_scope_guard: Arc::new(EventScopeGuard::default_rules()),
            start_time: Instant::now(),
            a2a_state: None,
            execution_adapter: None,
            openai_agent_registry: None,
            openai_provider_map: Arc::new(HashMap::new()),
            openai_provider_configs: Vec::new(),
            embedding_provider: None,
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
            idempotency_guard: Arc::new(crate::gateway::idempotency::IdempotencyGuard::new(
                std::time::Duration::from_secs(300), // 5 minute TTL
            )),
            event_scope_guard: Arc::new(EventScopeGuard::default_rules()),
            start_time: Instant::now(),
            a2a_state: None,
            execution_adapter: None,
            openai_agent_registry: None,
            openai_provider_map: Arc::new(HashMap::new()),
            openai_provider_configs: Vec::new(),
            embedding_provider: None,
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
        Arc::get_mut(&mut self.handlers).expect("Cannot modify handlers after server is running")
    }

    /// Get a reference to the event bus for publishing events
    pub fn event_bus(&self) -> &Arc<GatewayEventBus> {
        &self.event_bus
    }

    /// Set the guest session manager
    pub fn set_guest_session_manager(
        &mut self,
        manager: Arc<crate::gateway::security::GuestSessionManager>,
    ) {
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
            idempotency_guard: self.idempotency_guard.clone(),
            event_scope_guard: self.event_scope_guard.clone(),
            audit_log: None,
        });

        let control_plane = create_control_plane_router();

        // OpenAI-compatible API routes (/v1/models, /v1/health, /v1/chat/completions)
        let openai_state = Arc::new(OpenAiApiState {
            server_id: format!("aleph-{}", self.addr),
            api_token: None, // Phase 1: auth intentionally open (self-hosted single-user)
            execution_adapter: self.execution_adapter.clone(),
            provider_map: self.openai_provider_map.clone(),
            agent_registry: self.openai_agent_registry.clone(),
            provider_configs: Arc::new(self.openai_provider_configs.clone()),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            embedding_provider: self.embedding_provider.clone(),
        });
        let openai = openai_routes(openai_state);

        let mut router = Router::new()
            .route("/ws", get(handler::ws_upgrade_handler))
            .fallback_service(control_plane)
            .with_state(shared)
            .merge(openai);

        // Merge A2A routes if the subsystem is enabled
        if let Some(a2a_state) = &self.a2a_state {
            let a2a = crate::a2a::adapter::server::a2a_routes(a2a_state.clone());
            router = router.merge(a2a);
        }

        router.layer(SecurityHeadersLayer::new())
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

        // Background: prune stale idempotency entries every 60s
        let ig = self.idempotency_guard.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let pruned = ig.prune();
                if pruned > 0 {
                    debug!("Pruned {} expired idempotency entries", pruned);
                }
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
                let _ = eb.publish_json(&TopicEvent::new(
                    "system.tick",
                    serde_json::json!({
                        "ts": chrono::Utc::now().timestamp_millis(),
                        "state_version": sv.snapshot(),
                        "connections": pr.count(),
                        "uptime_ms": st.elapsed().as_millis() as u64,
                    }),
                ));
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
        let listener = tokio::net::TcpListener::bind(&self.addr)
            .await
            .map_err(|e| GatewayError::BindFailed {
                addr: self.addr,
                source: e,
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
        let listener = tokio::net::TcpListener::bind(&self.addr)
            .await
            .map_err(|e| GatewayError::BindFailed {
                addr: self.addr,
                source: e,
            })?;
        info!("Aleph listening on http://{}", self.addr);
        info!("  WebSocket: ws://{}/ws", self.addr);
        info!("  Panel UI:  http://{}/", self.addr);
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = shutdown.await;
        })
        .await
        .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;
        Ok(())
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
    use super::super::protocol::{JsonRpcResponse, PARSE_ERROR};
    use super::*;

    #[tokio::test]
    async fn test_process_valid_request() {
        let handlers = HandlerRegistry::new();
        let response =
            handler::process_request(r#"{"jsonrpc":"2.0","method":"health","id":1}"#, &handlers)
                .await;

        let parsed: JsonRpcResponse = serde_json::from_str(&response).unwrap();
        assert!(parsed.is_success());
    }

    #[tokio::test]
    async fn test_process_invalid_json() {
        let handlers = HandlerRegistry::new();
        let response = handler::process_request("not json", &handlers).await;

        let parsed: JsonRpcResponse = serde_json::from_str(&response).unwrap();
        assert!(parsed.is_error());
        assert_eq!(parsed.error.unwrap().code, PARSE_ERROR);
    }

    #[tokio::test]
    async fn test_process_method_not_found() {
        let handlers = HandlerRegistry::empty();
        let response =
            handler::process_request(r#"{"jsonrpc":"2.0","method":"unknown","id":1}"#, &handlers)
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
