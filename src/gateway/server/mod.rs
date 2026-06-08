//! WebSocket Gateway Server
//!
//! Handles WebSocket connections and dispatches JSON-RPC 2.0 requests
//! to registered handlers.

mod handler;
mod metrics_endpoint;
mod per_client_buffer;
mod probe;

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
use super::middleware::MiddlewareChain;
use super::presence::PresenceTracker;
use super::rate_limiter::{RateLimitConfig, RateLimiter};
use super::security::SharedTokenManager;
use super::security::TokenManager;
use super::session::HttpSessionManager;
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
    /// Role this connection authenticated as: `"operator"` for device tokens,
    /// `"guest"` for invitation-scoped guest sessions. `None` before connect or
    /// in auth-disabled mode. Consulted by the method-level authorization gate
    /// (`method_authz`) so non-operator connections cannot reach operator-only
    /// control-plane RPCs. See [`ConnectionState::is_operator`].
    pub role: Option<String>,
    /// Permissions (set after successful connect)
    pub permissions: Vec<String>,
    /// Guest session ID (set for guest connections)
    pub guest_session_id: Option<String>,
    /// HMAC hash of the device token this connection authenticated with.
    ///
    /// Set ONLY when auth succeeded via an existing device token
    /// (`connect` Case 1). `None` for every other auth path — shared-token
    /// loopback bootstrap, guest invitations, approved-device token issuance,
    /// and auth-disabled mode — so those connections are never subject to the
    /// per-dispatch revocation re-check (and the local panel is never
    /// disconnected by it). When `Some`, the dispatch loop re-checks revocation
    /// on each request and closes the connection if the token was revoked.
    pub device_token_hash: Option<String>,
    /// Resolved real client IP (socket peer IP, or the `X-Forwarded-For`
    /// client when the peer is a trusted proxy). The per-IP connection cap
    /// counts established connections by this value so it isolates real
    /// clients even when many share one reverse-proxy socket address.
    pub client_ip: std::net::IpAddr,
    /// Surface identity declared by the client on `connect` (or inferred from
    /// loopback when undeclared). Names *what kind of shell* this connection is
    /// — desktop / browser / cli — for tiering and (Phase 1+) delivery routing.
    /// Distinct from `ChannelClass` (lane priority). `None` before connect or
    /// for legacy clients that declared nothing.
    pub channel_kind: Option<crate::gateway::surface::SurfaceKind>,
}

impl ConnectionState {
    /// Create a new connection state for a connection from `client_ip`
    /// (the resolved real client IP — see [`ConnectionState::client_ip`]).
    pub(crate) fn new(client_ip: std::net::IpAddr) -> Self {
        Self {
            authenticated: false,
            first_message: true,
            auth_attempts: 0,
            subscriptions: vec![],
            metadata: HashMap::new(),
            device_id: None,
            role: None,
            permissions: vec![],
            guest_session_id: None,
            device_token_hash: None,
            client_ip,
            channel_kind: None,
        }
    }

    /// Mark connection as authenticated.
    ///
    /// `role` is `"operator"` for device-token connections and `"guest"` for
    /// invitation-scoped guest sessions; it drives the method-level
    /// authorization gate. `None` leaves the connection non-operator.
    pub fn authenticate(
        &mut self,
        device_id: String,
        permissions: Vec<String>,
        role: Option<String>,
    ) {
        self.authenticated = true;
        self.device_id = Some(device_id);
        self.permissions = permissions;
        self.role = role;
    }

    /// Whether this connection authenticated as an operator (full control-plane
    /// access). Guest / node connections return `false` and are barred from
    /// operator-only RPC methods by the dispatch-time authorization gate.
    pub fn is_operator(&self) -> bool {
        self.role.as_deref() == Some("operator")
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
    /// Per-IP concurrent-connection cap enforced at WS upgrade. `0` disables.
    /// Loopback is exempt. See [`GatewayConfig::max_connections_per_ip`].
    pub max_connections_per_ip: usize,
    pub presence: Arc<PresenceTracker>,
    pub state_versions: Arc<StateVersionTracker>,
    pub rate_limiter: Arc<RateLimiter>,
    pub lane_manager: Arc<LaneManager>,
    pub idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
    pub event_scope_guard: Arc<EventScopeGuard>,
    pub audit_log: Option<crate::security::audit::SecurityAuditLog>,
    /// Readiness flag — flipped to true after agent_init.rs completes
    /// phase-2 wiring. Read by `/ready` HTTP probe.
    pub ready: Arc<crate::sync_primitives::AtomicBool>,
    /// Per-process instance identifier (UUID v4). Stable for the lifetime
    /// of the server; regenerated on every restart. Clients use it to
    /// detect server restart vs same-server-came-back.
    pub instance_id: String,
    /// Unix epoch seconds at server construction. Surfaced by `/health`
    /// and `gateway.identity.get` so clients can compute uptime without
    /// trusting their local clock.
    pub started_at_unix: i64,
    /// Server-initiated WS Ping cadence. See `GatewayConfig::ping_interval_secs`.
    pub ping_interval_secs: u64,
    /// Inbound idle threshold before the connection is closed. See
    /// `GatewayConfig::idle_timeout_secs`.
    pub idle_timeout_secs: u64,
    /// Hard-require an `idempotency_key` on mutating RPCs. See
    /// `GatewayConfig::require_idempotency_key`.
    pub require_idempotency_key: bool,
    /// HTTP session manager used to validate the `aleph_session` cookie
    /// during WebSocket upgrade. When `Some` AND the peer is loopback,
    /// a valid cookie short-circuits the device-pairing flow by
    /// auto-injecting the locally-stored shared token into the first
    /// `connect` request — same trust class as the desktop shell's
    /// `aleph-server bootstrap-url` handoff. `None` in auth-disabled or
    /// legacy wiring; existing flow then runs untouched.
    pub session_mgr: Option<Arc<HttpSessionManager>>,
    /// Shared token manager. Read once per cookie-bootstrapped connection
    /// to obtain the token string injected into the first `connect`.
    /// `None` ⇒ cookie bootstrap is disabled even if `session_mgr` is set.
    pub shared_token_mgr: Option<Arc<SharedTokenManager>>,
    /// Device-token manager, used by the dispatch loop to re-check whether an
    /// already-authenticated connection's device token has been revoked since
    /// `connect`. `None` ⇒ the per-dispatch revocation re-check is disabled
    /// (e.g. auth-disabled mode or legacy wiring); the existing flow is
    /// untouched.
    pub token_manager: Option<Arc<TokenManager>>,
    /// The JSON-RPC middleware chain, built **once** at server construction and
    /// cloned per connection. Building it per-connection (the previous
    /// behaviour) re-ran [`MiddlewareChain::new`], which reinstalls the global
    /// [`RequestStateRegistry`] — so every new connection wiped the
    /// request-lifecycle counters that `/metrics` reads and undercounted
    /// in-flight requests from other connections. A single shared chain keeps
    /// those metrics monotonic and drops a per-connect allocation.
    pub middleware_chain: MiddlewareChain,
    /// Trusted reverse-proxy IPs / CIDRs. When the socket peer matches one of
    /// these, the real client IP is taken from `X-Forwarded-For` for the
    /// per-IP connection cap and rate limiting. Empty (default) ⇒ the socket
    /// peer address is used verbatim, and `X-Forwarded-For` is never trusted.
    pub trusted_proxies: Arc<crate::gateway::trusted_proxy::TrustedProxies>,
    /// Cross-origin policy enforced at the `/ws` upgrade. Rejects browser
    /// pages whose `Origin` is neither same-origin, loopback, `tauri:`, nor
    /// operator-allow-listed — the DNS-rebinding / cross-origin-WebSocket
    /// guard. Native clients (no `Origin` header) are unaffected.
    pub origin_policy: Arc<crate::gateway::origin_policy::OriginPolicy>,
    /// 每条连接的反向 RPC 通道（`conn_id → channel`）。连接建立时由入站
    /// 循环登记，断开时注销。0b 的 NodeRegistry 经此对 node 连接发起
    /// `node.invoke`；本表本身不区分 node/非 node —— 仅是传输层注册表。
    pub reverse_rpc: Arc<RwLock<HashMap<String, crate::cluster::ReverseRpcChannel>>>,
    /// Cluster node registry (shared Arc with GatewayServer). Center-side view
    /// of connected `role:node` peers; populated by the connect handler.
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
}

/// Configuration for the Gateway server
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Maximum concurrent connections from a single (non-loopback) remote IP.
    /// Bounds preauth slot-exhaustion: a remote peer cannot consume all global
    /// connection slots with sockets that never authenticate. `0` disables the
    /// cap; loopback is always exempt.
    pub max_connections_per_ip: usize,
    /// Authentication mode
    pub auth_mode: AuthMode,
    /// Connection timeout in seconds
    pub timeout_secs: u64,
    /// How often the server sends a WebSocket Ping frame to each authenticated
    /// peer. Browsers and `tokio-tungstenite` clients auto-reply with Pong, so
    /// this also proves the socket is still alive even when no application
    /// traffic flows.
    pub ping_interval_secs: u64,
    /// If no inbound frame (including the auto-Pong reply) arrives within this
    /// many seconds, the server closes the connection with WS code 1008. Half-
    /// open TCP sockets are otherwise invisible until the next failed write,
    /// which on macOS/Linux can take 2 hours due to default TCP keepalive.
    pub idle_timeout_secs: u64,
    /// Lane concurrency / channel-class priority config. Populated by
    /// the binary from the TOML `[gateway.lane]` block (or defaults).
    pub lane: LaneConfig,
    /// When true, mutating RPCs (Execute / Mutate / System lanes) must
    /// carry an `idempotency_key` in params or are rejected before lane
    /// dispatch. See `GatewayServerConfig::require_idempotency_key`.
    pub require_idempotency_key: bool,
    /// Trusted reverse-proxy IPs / CIDRs whose `X-Forwarded-For` header may be
    /// trusted to carry the real client IP. Empty (default) ⇒ the socket peer
    /// address is used verbatim. See `GatewayServerConfig::trusted_proxies`.
    pub trusted_proxies: Vec<String>,
    /// Extra browser origins allowed on the `/ws` upgrade, in addition to the
    /// built-in same-origin / loopback / `tauri:` rules. Populated by the
    /// binary from the TOML `[gateway.auth] allowed_origins`. Empty (default)
    /// ⇒ only same-origin and same-machine clients may open a WebSocket. See
    /// [`crate::gateway::origin_policy::OriginPolicy`].
    pub allowed_origins: Vec<String>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            max_connections: 1000,
            max_connections_per_ip: 64,
            auth_mode: AuthMode::default(),
            timeout_secs: 300,
            ping_interval_secs: 30,
            idle_timeout_secs: 90,
            trusted_proxies: Vec::new(),
            allowed_origins: Vec::new(),
            lane: LaneConfig::default(),
            require_idempotency_key: false,
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
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
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
    /// Per-process instance identifier (UUID v4). Stable for the lifetime
    /// of this `GatewayServer`; regenerated on every restart.
    pub instance_id: String,
    /// Unix epoch seconds captured at construction. Sibling of `start_time`
    /// in JSON-serializable form.
    pub started_at_unix: i64,
    /// Readiness flag — flipped to true after boot phase-2 completes.
    /// Read by `/ready` HTTP probe.
    pub ready: Arc<crate::sync_primitives::AtomicBool>,
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
    /// Phase 5 Orchestrator (flow composition). Populated at boot after
    /// agent registry + session + tool + provider + sandbox are assembled.
    /// Task 10 (Gateway run_agent_loop replacement) consumes this.
    pub orchestrator: Option<Arc<crate::orchestrator::Orchestrator>>,
    /// Bearer token snapshot for OpenAI-compatible `/v1/*` routes.
    /// Sourced from `SharedTokenManager` at boot. Some → handler rejects
    /// mismatched bearers with 401; None → dev-open (any bearer accepted).
    /// Token rotation requires a server restart.
    pub openai_api_token: Option<String>,
    /// Admin IPC router (Spec C). Mounted under `/v1/admin` when set.
    /// `None` means CLI subcommands routed via `LockOrIpc` will receive
    /// 404 from the server side — the CLI is expected to take the local
    /// lock instead.
    admin_router: Option<Router>,
    /// HTTP auth routes (`/auth/logout`, `/auth/bootstrap`, `/pair`,
    /// `/rpc`). Mounted at the router
    /// root when set so the loopback bootstrap-consume endpoint and the
    /// browser-pairing flow are reachable from the browser. The legacy
    /// `/login` + `/auth/login` token-paste form was removed in Phase 4.
    auth_routes: Option<Router>,
    /// HTTP session manager. When set together with `shared_token_mgr`,
    /// `ws_upgrade_handler` validates the `aleph_session` cookie on
    /// loopback peers and auto-injects the shared token into the first
    /// `connect` so the Tauri Panel does not have to prompt for pairing.
    session_mgr: Option<Arc<HttpSessionManager>>,
    /// Shared token manager. See `session_mgr` doc for the bootstrap
    /// flow these two cooperate to enable.
    shared_token_mgr: Option<Arc<SharedTokenManager>>,
    /// Device-token manager for the dispatch-time revocation re-check.
    token_manager: Option<Arc<TokenManager>>,
    /// 见 [`GatewaySharedState::reverse_rpc`]。`build_router` 把它 clone 进
    /// 共享状态，因此两侧指向同一张表。
    pub reverse_rpc: Arc<RwLock<HashMap<String, crate::cluster::ReverseRpcChannel>>>,
    /// See [`GatewaySharedState::node_registry`]. `build_router` clones this Arc
    /// into the shared state so both point at the same registry.
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
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
            instance_id: uuid::Uuid::new_v4().to_string(),
            started_at_unix: chrono::Utc::now().timestamp(),
            ready: Arc::new(crate::sync_primitives::AtomicBool::new(false)),
            a2a_state: None,
            execution_adapter: None,
            openai_agent_registry: None,
            openai_provider_map: Arc::new(HashMap::new()),
            openai_provider_configs: Vec::new(),
            embedding_provider: None,
            orchestrator: None,
            openai_api_token: None,
            admin_router: None,
            auth_routes: None,
            session_mgr: None,
            shared_token_mgr: None,
            token_manager: None,
            reverse_rpc: Arc::new(RwLock::new(HashMap::new())),
            node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
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

        let lane_config = config.lane.clone();

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
            lane_manager: Arc::new(LaneManager::new(lane_config)),
            idempotency_guard: Arc::new(crate::gateway::idempotency::IdempotencyGuard::new(
                std::time::Duration::from_secs(300), // 5 minute TTL
            )),
            event_scope_guard: Arc::new(EventScopeGuard::default_rules()),
            start_time: Instant::now(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            started_at_unix: chrono::Utc::now().timestamp(),
            ready: Arc::new(crate::sync_primitives::AtomicBool::new(false)),
            a2a_state: None,
            execution_adapter: None,
            openai_agent_registry: None,
            openai_provider_map: Arc::new(HashMap::new()),
            openai_provider_configs: Vec::new(),
            embedding_provider: None,
            orchestrator: None,
            openai_api_token: None,
            admin_router: None,
            auth_routes: None,
            session_mgr: None,
            shared_token_mgr: None,
            token_manager: None,
            reverse_rpc: Arc::new(RwLock::new(HashMap::new())),
            node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
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

    /// Mount the Spec C admin IPC router under `/v1/admin` in `build_router`.
    /// Idempotent — replaces any previously set admin router.
    pub fn set_admin_router(&mut self, router: Router) {
        self.admin_router = Some(router);
    }

    /// Mount HTTP auth routes (`/auth/logout`, `/auth/bootstrap`,
    /// `/pair`, `/rpc`) at the router
    /// root in `build_router`. Built by
    /// [`crate::gateway::auth_middleware::auth_routes`]. The Phase 4
    /// deletion of `/login` + `/auth/login` means these routes now
    /// strictly override only the bootstrap + pairing surfaces.
    /// Idempotent — replaces any previously set auth routes.
    pub fn set_auth_routes(&mut self, router: Router) {
        self.auth_routes = Some(router);
    }

    /// Install the `HttpSessionManager` whose cookies the `/ws` upgrade
    /// handler may trust on loopback peers. Must be paired with
    /// [`set_shared_token_manager`] for the bootstrap auto-inject to fire.
    pub fn set_session_manager(&mut self, manager: Arc<HttpSessionManager>) {
        self.session_mgr = Some(manager);
    }

    /// Install the `SharedTokenManager`. Required alongside
    /// [`set_session_manager`] so the cookie-validated WS upgrade can
    /// auto-inject the locally-stored shared token into the first
    /// `connect` request.
    pub fn set_shared_token_manager(&mut self, manager: Arc<SharedTokenManager>) {
        self.shared_token_mgr = Some(manager);
    }

    /// Install the `TokenManager` so the dispatch loop can re-check device-token
    /// revocation on each request from an already-authenticated connection.
    /// When unset, the re-check is skipped (existing behaviour).
    pub fn set_token_manager(&mut self, manager: Arc<TokenManager>) {
        self.token_manager = Some(manager);
    }

    /// Get the current number of active connections
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Build a unified axum Router with WebSocket + ControlPlane UI routes.
    /// WebSocket connections are handled at `/ws`, everything else serves the Panel UI.
    pub fn build_router(&self) -> Router {
        // Build the middleware chain ONCE here (cloned per connection) rather
        // than per-connection, so the global request-state registry is
        // installed a single time and its counters accumulate across
        // connections instead of resetting on every connect.
        let middleware_chain =
            MiddlewareChain::new(self.handlers.clone(), self.rate_limiter.clone());
        let trusted_proxies = Arc::new(crate::gateway::trusted_proxy::TrustedProxies::from_config(
            &self.config.trusted_proxies,
        ));

        // Wire the embedded-PTY subsystem to this server's event bus so live
        // terminal output is broadcast on the `pty.output` / `pty.exit` topics
        // through the normal subscription path (single fixed port, no second
        // socket). Idempotent — safe if `build_router` runs more than once.
        crate::gateway::pty::attach_event_bus(self.event_bus.clone());

        let shared = Arc::new(GatewaySharedState {
            handlers: self.handlers.clone(),
            event_bus: self.event_bus.clone(),
            connections: self.connections.clone(),
            subscription_manager: self.subscription_manager.clone(),
            guest_session_manager: self.guest_session_manager.clone(),
            auth_mode: self.config.auth_mode.clone(),
            max_connections: self.config.max_connections,
            max_connections_per_ip: self.config.max_connections_per_ip,
            presence: self.presence.clone(),
            state_versions: self.state_versions.clone(),
            rate_limiter: self.rate_limiter.clone(),
            lane_manager: self.lane_manager.clone(),
            idempotency_guard: self.idempotency_guard.clone(),
            event_scope_guard: self.event_scope_guard.clone(),
            audit_log: None,
            ready: self.ready.clone(),
            instance_id: self.instance_id.clone(),
            started_at_unix: self.started_at_unix,
            ping_interval_secs: self.config.ping_interval_secs,
            idle_timeout_secs: self.config.idle_timeout_secs,
            require_idempotency_key: self.config.require_idempotency_key,
            session_mgr: self.session_mgr.clone(),
            shared_token_mgr: self.shared_token_mgr.clone(),
            token_manager: self.token_manager.clone(),
            middleware_chain,
            trusted_proxies,
            origin_policy: Arc::new(crate::gateway::origin_policy::OriginPolicy::new(
                self.config.allowed_origins.clone(),
            )),
            reverse_rpc: self.reverse_rpc.clone(),
            node_registry: self.node_registry.clone(),
        });

        let control_plane = create_control_plane_router();

        // OpenAI-compatible API routes (/v1/models, /v1/health, /v1/chat/completions)
        let openai_state = Arc::new(OpenAiApiState {
            server_id: format!("aleph-{}", self.addr),
            // Bearer-token snapshot from SharedTokenManager. `None` leaves the
            // endpoint open (dev mode); `Some(token)` rejects mismatched
            // bearers with 401 in completions/mod.rs before reaching the
            // per-agent busy-lock or the LLM.
            api_token: self.openai_api_token.clone(),
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
            .route("/health", get(probe::handle_health))
            .route("/ready", get(probe::handle_ready))
            .route("/metrics", get(metrics_endpoint::handle_metrics))
            .fallback_service(control_plane)
            .with_state(shared)
            .merge(openai);

        // Merge A2A routes if the subsystem is enabled
        if let Some(a2a_state) = &self.a2a_state {
            let a2a = crate::a2a::adapter::server::a2a_routes(a2a_state.clone());
            router = router.merge(a2a);
        }

        // Spec C: mount admin IPC router under /v1/admin if configured.
        if let Some(admin) = self.admin_router.clone() {
            router = router.nest("/v1/admin", admin);
        }

        // Mount auth routes at the root (`/auth/*`, `/pair`, `/rpc`) so
        // the loopback bootstrap-consume endpoint and the browser-pairing
        // flow are reachable. The Phase 4 deletion of `/login` means
        // unauthenticated browsers are redirected to `/pair` instead.
        if let Some(auth) = self.auth_routes.clone() {
            router = router.merge(auth);
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
    use super::super::middleware::MiddlewareChain;
    use super::super::protocol::{JsonRpcResponse, PARSE_ERROR};
    use super::*;

    #[tokio::test]
    async fn test_process_valid_request() {
        let handlers_arc = Arc::new(HandlerRegistry::new());
        let chain = MiddlewareChain::new(
            handlers_arc.clone(),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let response =
            handler::process_request(r#"{"jsonrpc":"2.0","method":"health","id":1}"#, &chain).await;

        let parsed: JsonRpcResponse = serde_json::from_str(&response).unwrap();
        assert!(parsed.is_success());
    }

    #[tokio::test]
    async fn test_process_invalid_json() {
        let handlers_arc = Arc::new(HandlerRegistry::new());
        let chain = MiddlewareChain::new(
            handlers_arc.clone(),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let response = handler::process_request("not json", &chain).await;

        let parsed: JsonRpcResponse = serde_json::from_str(&response).unwrap();
        assert!(parsed.is_error());
        assert_eq!(parsed.error.unwrap().code, PARSE_ERROR);
    }

    #[tokio::test]
    async fn test_process_method_not_found() {
        let handlers_arc = Arc::new(HandlerRegistry::empty());
        let chain = MiddlewareChain::new(
            handlers_arc.clone(),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let response =
            handler::process_request(r#"{"jsonrpc":"2.0","method":"unknown","id":1}"#, &chain)
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

    #[tokio::test]
    async fn reverse_rpc_registry_is_empty_on_fresh_server() {
        let addr = "127.0.0.1:0".parse().unwrap();
        let server = GatewayServer::new(addr);
        assert_eq!(server.reverse_rpc.read().await.len(), 0);
    }

    #[tokio::test]
    async fn node_registry_is_empty_on_fresh_server() {
        let server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
        assert!(server.node_registry.list_environments().is_empty());
    }
}

#[cfg(test)]
mod channel_kind_tests {
    use super::*;
    use crate::gateway::surface::SurfaceKind;

    #[test]
    fn new_connection_has_no_channel_kind() {
        let cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        assert_eq!(cs.channel_kind, None);
    }

    #[test]
    fn channel_kind_is_settable() {
        let mut cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        cs.channel_kind = Some(SurfaceKind::Desktop);
        assert_eq!(cs.channel_kind, Some(SurfaceKind::Desktop));
    }
}
