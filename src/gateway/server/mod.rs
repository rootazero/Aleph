//! WebSocket Gateway Server
//!
//! Handles WebSocket connections and dispatches JSON-RPC 2.0 requests
//! to registered handlers.

mod artifact_route;
mod byte_range;
mod canvas_asset_route;
mod flood_guard;
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

use super::event_bus::{GatewayEventBus, TopicEvent};
use super::event_scope::EventScopeGuard;
use super::handlers::events::SubscriptionManager;
use super::handlers::HandlerRegistry;
use super::lane::{LaneConfig, LaneManager};
use super::middleware::MiddlewareChain;
use super::presence::PresenceTracker;
use super::rate_limiter::{RateLimitConfig, RateLimiter};
use super::security::SharedTokenManager;
use super::state_version::StateVersionTracker;
use crate::providers::protocols::ProtocolLoader;
use crate::security::headers::SecurityHeadersLayer;
use notify::RecommendedWatcher;
use notify_debouncer_full::{Debouncer, FileIdMap};

/// State for an individual WebSocket connection
pub struct ConnectionState {
    /// Whether this is the first message (for handshake enforcement)
    pub first_message: bool,
    /// Event topics this connection is subscribed to
    pub subscriptions: Vec<String>,
    /// Connection metadata
    pub metadata: HashMap<String, String>,
    /// Event-scope permissions, stamped at the `connect` handshake from the
    /// **resolved role** via
    /// [`event_scope::scope_for_role`](crate::gateway::event_scope::scope_for_role)
    /// and re-stamped in place by `handlers::users::restamp_live_connections`
    /// when that role changes. Operator ⇒ the `"*"` wildcard; member and walled
    /// ⇒ empty.
    ///
    /// Read by two consumers on the per-event delivery path:
    /// [`EventScopeGuard::can_receive`], which is *default-allow* — an empty
    /// scope still receives every unguarded topic (chat, session,
    /// `agent.run.*`) and only loses the admin-guarded prefixes
    /// (`surface.approval`, `config.changed`, `pairing.*`, `guest.*`, `node.`)
    /// — and `event_scope::is_superuser_scope`, which supplies the admin arm of
    /// `event_visibility`'s `BySessionKeyOrAdmin`. The raw `approval.*` topics
    /// moved from the first consumer to the second on 2026-08-08 so a member
    /// can receive the card for their own parked tool call; see
    /// `event_visibility`'s module doc.
    pub permissions: Vec<String>,
    /// Resolved client IP (the trusted-proxy-forwarded client behind a
    /// reverse proxy, else the raw socket peer). The per-IP connection cap
    /// counts established connections by this value.
    pub client_ip: std::net::IpAddr,
    /// Surface identity declared by the client on `connect` (or inferred from
    /// loopback when undeclared). Names *what kind of shell* this connection is
    /// — desktop / browser / cli — for tiering and (Phase 1+) delivery routing.
    /// Distinct from `ChannelClass` (lane priority). `None` before connect or
    /// for legacy clients that declared nothing.
    pub channel_kind: Option<crate::gateway::surface::SurfaceKind>,
    /// Originating connection's authorization role for the config-tier gates
    /// (`"operator"` / `"guest"`). Resolved at the `connect` handshake from the
    /// device tier (loopback ⇒ operator; remote ⇒ persisted per-device tier,
    /// default chat) and read per-request by the WS dispatch gate. Defaulted by
    /// loopback in [`ConnectionState::new`] so the pre-handshake `connect` frame
    /// and any probe path behave safely.
    pub caller_role: String,
    /// The paired Panel device this session authenticated as, latched at the
    /// `connect` handshake. `None` for loopback, the legacy shared-token path,
    /// and still-walled connections — none of which are bound to a device
    /// record. Read by [`invalidate_device_sessions`] so a per-device revoke can
    /// strip authority from exactly the right sockets.
    pub device_id: Option<String>,
    /// Authenticated user behind this connection (`users.user_id`), resolved at
    /// `connect` together with `caller_role`. `None` for walled connections.
    pub caller_user: Option<String>,
}

impl ConnectionState {
    /// Create a new connection state for a connection from `client_ip`
    /// (the resolved real client IP — see [`ConnectionState::client_ip`]),
    /// with `client_is_local` supplied by the caller.
    ///
    /// ⚠️ `client_is_local` is a PARAMETER and must stay one. It used to be
    /// re-derived here as `client_ip.is_loopback()`, which is the predicate the
    /// trusted-proxy fix replaced at every other authority site on 2026-08-29 —
    /// and this one was missed, which is the whole shape of that bug rather
    /// than a second instance of it. Behind a same-host reverse proxy that emits
    /// no `X-Forwarded-For`, `resolve_client` reports the proxy's own loopback
    /// address as `ip` while setting `local: false`; re-deriving the bit from
    /// `ip` here stamps `caller_role = "operator"` on a remote connection before
    /// any handshake has happened. `client_ip` answers "which bucket does this
    /// belong to" (rate limiting, audit). It never answers "may this do that".
    ///
    /// Taking it as an argument is what makes the difference enforceable: a
    /// future call site cannot forget the distinction, because there is nothing
    /// to forget — the compiler asks for the bit.
    pub(crate) fn new(client_ip: std::net::IpAddr, client_is_local: bool) -> Self {
        Self {
            first_message: true,
            subscriptions: vec![],
            metadata: HashMap::new(),
            permissions: vec![],
            client_ip,
            channel_kind: None,
            // Local (the desktop App over loopback, not merely an address that
            // looks like loopback) is the implicit operator; every remote
            // connection starts at guest until the handshake elevates it.
            caller_role: if client_is_local {
                "operator".to_string()
            } else {
                "guest".to_string()
            },
            device_id: None,
            caller_user: None,
        }
    }
}

/// Strip operator authority from every live session bound to `device_id`.
///
/// The synchronous half of `gateway.devices.revoke`: the store write alone only
/// stops the *next* handshake, and the `DeviceRevoked` kick only lands when the
/// connection's event arm is next polled — until then a `tokio::select!` loop may
/// still serve requests the revoked client already pipelined onto its socket.
/// Downgrading `caller_role` here makes those requests hit the existing login
/// wall instead, so revocation is effective the instant the RPC returns rather
/// than "eventually". Mirrors openclaw's `invalidateClientsForDevice` running
/// before `disconnectClientsForDevice`.
///
/// Returns how many connections were downgraded (0 when the device has no open
/// session). Sessions with no `device_id` — loopback, legacy shared token, walled
/// — are never touched.
pub async fn invalidate_device_sessions(
    connections: &Arc<RwLock<HashMap<String, ConnectionState>>>,
    device_id: &str,
) -> usize {
    let mut conns = connections.write().await;
    let mut hit = 0;
    for state in conns.values_mut() {
        if state.device_id.as_deref() == Some(device_id) {
            state.caller_role = "guest".to_string();
            // caller_user is resolved together with caller_role (see its doc
            // comment) — a downgrade to guest must clear it too, or a walled
            // connection would keep reading a stale authenticated user.
            state.caller_user = None;
            state.permissions.clear();
            hit += 1;
        }
    }
    hit
}

/// Shared state for the unified axum server (WebSocket + `ControlPlane`)
#[derive(Clone)]
pub struct GatewaySharedState {
    pub handlers: Arc<HandlerRegistry>,
    pub event_bus: Arc<GatewayEventBus>,
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    pub subscription_manager: Arc<SubscriptionManager>,
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
    /// Trusted-proxy toggle (mirror of `[gateway.trusted_proxy] enabled`).
    pub trusted_proxy_enabled: bool,
    /// Parsed trusted-proxy peer IPs whose `X-Forwarded-*` are honored.
    pub trusted_proxy_ips: Vec<std::net::IpAddr>,
    /// Mirror of `[gateway] allow_insecure_remote`. `false` ⇒ a non-loopback
    /// insecure connection is refused at upgrade (Task 5).
    pub allow_insecure_remote: bool,
    /// True when the gateway terminates TLS in-process (native tiers). Every
    /// connection is then secure regardless of forwarding headers.
    pub tls_enabled: bool,
    /// Readiness flag — flipped to true after `agent_init.rs` completes
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
    /// Vault handle (`SharedTokenManager`). The WS path no longer reads it
    /// (the cookie-bootstrap auto-inject died with the LAN-trust revert),
    /// but the handle is retained alongside the process-global installed by
    /// [`GatewayServer::set_shared_token_manager`] — the vault is live
    /// infrastructure (provider keys, channel secrets, admin IPC bearer).
    pub shared_token_mgr: Option<Arc<SharedTokenManager>>,
    /// Device-token manager for bootstrap-ticket / per-device-token auth.
    /// Populated at boot by [`GatewayServer::set_device_token_manager`].
    pub device_token_mgr: Option<Arc<crate::gateway::security::DeviceTokenManager>>,
    /// Security store handle. The cluster node connect/disconnect paths use
    /// it to stamp the enrolled device's `last_seen_at` for the offline
    /// `environments.list` view. `None` in probe/legacy wiring.
    pub security_store: Option<Arc<crate::gateway::security::SecurityStore>>,
    /// The JSON-RPC middleware chain, built **once** at server construction and
    /// cloned per connection. Building it per-connection (the previous
    /// behaviour) re-ran [`MiddlewareChain::new`], which reinstalls the global
    /// [`RequestStateRegistry`] — so every new connection wiped the
    /// request-lifecycle counters that `/metrics` reads and undercounted
    /// in-flight requests from other connections. A single shared chain keeps
    /// those metrics monotonic and drops a per-connect allocation.
    pub middleware_chain: MiddlewareChain,
    /// Cross-origin policy enforced at the `/ws` upgrade. Rejects browser
    /// pages whose `Origin` is neither same-origin, loopback, `tauri:`, nor
    /// operator-allow-listed — the DNS-rebinding / cross-origin-WebSocket
    /// guard. Native clients (no `Origin` header) are unaffected.
    pub origin_policy: Arc<crate::gateway::origin_policy::OriginPolicy>,
    /// Cluster node registry (shared Arc with `GatewayServer`). Center-side view
    /// of connected `role:node` peers; populated by the connect handler.
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
    pub exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
    /// Session store handle for the owner-scoped WS event filter (P1 data
    /// isolation, spec §5.4 — `event_visibility::EventVisibilityIndex`).
    /// `None` in probe/legacy wiring: the 4th filter term is then skipped
    /// (zero-change guarantee), matching every other `Option<Arc<...>>`
    /// dependency in this struct.
    pub session_store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    /// Team store handle for the same filter's `team.<id>.*` plane, whose
    /// frames resolve to a TEAM's owner rather than a session's. `None` in
    /// probe/legacy wiring and in any deployment with no team database — those
    /// frames are then denied, not waved through (nothing can publish them
    /// either; see `event_visibility::EventVisibilityIndex::event_admits`).
    pub team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    /// Process-shared run→session / session→owner cache backing the
    /// owner-scoped WS event filter. Always constructed (unlike
    /// `session_store`) — the index itself is cheap and harmless to warm
    /// even when no store is wired to consult it.
    pub event_visibility: Arc<crate::gateway::event_visibility::EventVisibilityIndex>,
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
    /// Extra browser origins allowed on the `/ws` upgrade, in addition to the
    /// built-in same-origin / loopback / `tauri:` rules. Populated by the
    /// binary from the TOML `[gateway] allowed_origins`. Empty (default)
    /// ⇒ only same-origin and same-machine clients may open a WebSocket. See
    /// [`crate::gateway::origin_policy::OriginPolicy`].
    pub allowed_origins: Vec<String>,
    /// Trust every Origin on the `/ws` upgrade (reverse-proxy escape
    /// hatch). Mirrors `GatewayServerConfig::allow_any_origin`.
    pub allow_any_origin: bool,
    /// Trusted-proxy toggle. Mirrors `GatewayServerConfig::trusted_proxy.enabled`.
    pub trusted_proxy_enabled: bool,
    /// Trusted-proxy peer IPs (raw strings; parsed in `build_router`).
    /// Mirrors `GatewayServerConfig::trusted_proxy.trusted_ips`.
    pub trusted_proxy_ips: Vec<String>,
    /// Mirrors `GatewayServerConfig::allow_insecure_remote`.
    pub allow_insecure_remote: bool,
    /// Mirrors `GatewayServerConfig::tls.enabled`.
    pub tls_enabled: bool,
    /// Mirrors `GatewayServerConfig::tls.cert_path`.
    pub tls_cert_path: String,
    /// Mirrors `GatewayServerConfig::tls.key_path`.
    pub tls_key_path: String,
    /// Mirrors `GatewayServerConfig::tls.san`.
    pub tls_san: Vec<String>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            max_connections: 1000,
            max_connections_per_ip: 64,
            timeout_secs: 300,
            ping_interval_secs: 30,
            idle_timeout_secs: 90,
            allowed_origins: Vec::new(),
            allow_any_origin: false,
            trusted_proxy_enabled: false,
            trusted_proxy_ips: vec!["127.0.0.1".to_string(), "::1".to_string()],
            allow_insecure_remote: false,
            tls_enabled: false,
            tls_cert_path: String::new(),
            tls_key_path: String::new(),
            tls_san: Vec::new(),
            lane: LaneConfig::default(),
            require_idempotency_key: false,
        }
    }
}

/// Unified Gateway Server
///
/// Serves WebSocket connections at `/ws` and the `ControlPlane` UI as fallback,
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
    /// Model → `HttpProvider` map for passthrough completions
    pub openai_provider_map:
        Arc<HashMap<String, Arc<crate::providers::http_provider::HttpProvider>>>,
    /// Provider configs for /v1/models listing
    pub openai_provider_configs: Vec<(String, crate::config::ProviderConfig)>,
    /// Embedding provider for /v1/embeddings
    pub embedding_provider: Option<Arc<dyn crate::memory::EmbeddingProvider>>,
    /// Phase 5 Orchestrator (flow composition). Populated at boot after
    /// agent registry + session + tool + provider + sandbox are assembled.
    /// Task 10 (Gateway `run_agent_loop` replacement) consumes this.
    pub orchestrator: Option<Arc<crate::orchestrator::Orchestrator>>,
    // Note: the OpenAI-compat bearer token used to live here as an
    // `Option<String>` snapshot taken from `SharedTokenManager` at boot.
    // That snapshot was frozen for the lifetime of the server, so a
    // `gateway.token.rotate` would not revoke the previously issued
    // token for `/v1/*`. The auth path now reads the *current* token
    // through the `api_token` closure injected into `OpenAiApiState`
    // at boot, which captures `SharedTokenManager` directly. Do not
    // reintroduce a snapshot here.
    /// Admin IPC router (Spec C). Mounted under `/v1/admin` when set.
    /// `None` means CLI subcommands routed via `LockOrIpc` will receive
    /// 404 from the server side — the CLI is expected to take the local
    /// lock instead.
    admin_router: Option<Router>,

    /// Join handle for the background reconciler daemon (event-log \u2194
    /// notes filesystem divergence scanner). Stored here so a graceful
    /// shutdown can `.abort()` the task before the StateDatabase it
    /// reads from is dropped. `None` when `[memory.reconciler]` is
    /// disabled or no memory handler was wired into AdminApiState.
    reconciler_handle: Option<tokio::task::JoinHandle<()>>,
    /// Live channel webhook mount table, shared with `ChannelRegistry`.
    ///
    /// `build_router()` always registers the one wildcard route over this
    /// table, so the route table does not depend on configuration — that is
    /// what lets a channel started or created after `serve()` become
    /// reachable without a restart. An empty table 404s every webhook path.
    webhook_mounts: Arc<crate::gateway::webhook_receiver::WebhookMountTable>,
    /// Vault handle. Installed by [`GatewayServer::set_shared_token_manager`],
    /// which also publishes the process-global used by vault consumers
    /// (e.g. the WhatsApp vault store's crypto lookup).
    shared_token_mgr: Option<Arc<SharedTokenManager>>,
    /// Device-token manager for bootstrap-ticket / per-device-token auth.
    /// Populated at boot by [`GatewayServer::set_device_token_manager`].
    device_token_mgr: Option<Arc<crate::gateway::security::DeviceTokenManager>>,
    /// Security store handle for the node `last_seen_at` stamping in the
    /// WS connect/disconnect paths. See `GatewaySharedState::security_store`.
    security_store: Option<Arc<crate::gateway::security::SecurityStore>>,
    /// Whiteboard canvas store, installed by [`GatewayServer::set_canvas_store`]
    /// — the SAME Arc the `canvas.*` handlers hold (a second instance would
    /// split the per-canvas critical sections in two). `Some` mounts the
    /// capability-gated `/canvas-asset/...` byte route in `build_router`;
    /// `None` (canvas root unavailable / probe constructors) leaves the
    /// Panel on the `canvas.asset.get` base64 fallback.
    canvas_store: Option<Arc<crate::canvas::CanvasStore>>,
    /// See [`GatewaySharedState::node_registry`]. `build_router` clones this Arc
    /// into the shared state so both point at the same registry.
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
    /// Shared exec-approval manager (cluster ③). `Some` once boot wires the
    /// canonical instance; `None` in test/probe constructors ⇒ node-approval
    /// routing is inert (the handler refuses `node.approval.request`).
    pub exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
    /// Security audit log for remote-connection auth forensics. Installed by
    /// [`GatewayServer::set_audit_log`] and cloned into `GatewaySharedState`.
    /// `None` in test/probe constructors ⇒ auth events go unrecorded.
    audit_log: Option<crate::security::audit::SecurityAuditLog>,
    /// See [`GatewaySharedState::session_store`]. Installed by
    /// [`GatewayServer::set_session_store`].
    session_store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    /// See [`GatewaySharedState::team_store`]. Installed by
    /// [`GatewayServer::set_team_store`].
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    /// See [`GatewaySharedState::event_visibility`]. Always constructed.
    event_visibility: Arc<crate::gateway::event_visibility::EventVisibilityIndex>,
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
            admin_router: None,
            reconciler_handle: None,
            webhook_mounts: Arc::new(crate::gateway::webhook_receiver::WebhookMountTable::new()),
            shared_token_mgr: None,
            device_token_mgr: None,
            security_store: None,
            canvas_store: None,
            node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
            exec_approval_manager: None,
            audit_log: None,
            session_store: None,
            team_store: None,
            event_visibility: Arc::new(
                crate::gateway::event_visibility::EventVisibilityIndex::new(),
            ),
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
            admin_router: None,
            reconciler_handle: None,
            webhook_mounts: Arc::new(crate::gateway::webhook_receiver::WebhookMountTable::new()),
            shared_token_mgr: None,
            device_token_mgr: None,
            security_store: None,
            canvas_store: None,
            node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
            exec_approval_manager: None,
            audit_log: None,
            session_store: None,
            team_store: None,
            event_visibility: Arc::new(
                crate::gateway::event_visibility::EventVisibilityIndex::new(),
            ),
        }
    }

    /// Get a reference to the subscription manager
    #[must_use]
    pub const fn subscription_manager(&self) -> &Arc<SubscriptionManager> {
        &self.subscription_manager
    }

    /// Get a reference to the handler registry for registering custom handlers
    #[must_use]
    pub const fn handlers(&self) -> &Arc<HandlerRegistry> {
        &self.handlers
    }

    /// Get a mutable reference to the handler registry
    ///
    /// # Panics — boot-phase-only API
    ///
    /// `Arc::get_mut` succeeds only while THIS server holds the sole strong
    /// reference to the registry. It panics (via `unreachable!`) the moment
    /// the registry has been cloned anywhere — e.g. into a middleware chain,
    /// a handler context, or a spawned task. Call ONLY during server setup,
    /// before `run()` and before any clone of [`Self::handlers`] escapes.
    /// A panic here is a startup-ordering bug in the boot sequence, not a
    /// runtime condition to handle: if you see it in the field, some code
    /// cloned the registry before the last `handlers_mut` call finished.
    pub fn handlers_mut(&mut self) -> &mut HandlerRegistry {
        debug_assert!(
            Arc::strong_count(&self.handlers) == 1,
            "handlers_mut called while the registry is shared — this is a boot-ordering bug"
        );
        Arc::get_mut(&mut self.handlers)
            .unwrap_or_else(|| unreachable!("Cannot modify handlers after server is running"))
    }

    /// Get a reference to the event bus for publishing events
    #[must_use]
    pub const fn event_bus(&self) -> &Arc<GatewayEventBus> {
        &self.event_bus
    }

    /// Set the A2A server state (enables A2A routes in `build_router`)
    pub fn set_a2a_state(&mut self, state: Arc<crate::a2a::adapter::server::A2AServerState>) {
        self.a2a_state = Some(state);
    }

    /// Mount the Spec C admin IPC router under `/v1/admin` in `build_router`.
    /// Idempotent — replaces any previously set admin router.
    pub fn set_admin_router(&mut self, router: Router) {
        self.admin_router = Some(router);
    }

    /// Store the `JoinHandle` of the background reconciler daemon so
    /// graceful shutdown can `.abort()` the task before the
    /// `StateDatabase` it reads from is dropped. Idempotent — replacing
    /// the handle aborts the previous one (defensive: in practice the
    /// daemon is started once at boot).
    pub fn set_reconciler_handle(&mut self, handle: tokio::task::JoinHandle<()>) {
        if let Some(prior) = self.reconciler_handle.take() {
            prior.abort();
        }
        self.reconciler_handle = Some(handle);
    }

    /// Abort the background reconciler daemon if one is running. Called
    /// from the server's shutdown path before the database is dropped.
    /// Returns the prior handle (caller may await its completion if it
    /// cares about draining in-flight scans).
    #[must_use]
    pub fn abort_reconciler_daemon(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.reconciler_handle.take().inspect(|h| {
            h.abort();
        })
    }

    /// Serve channel webhook ingestion from `table`.
    ///
    /// Idempotent. Call order does not matter: the table is shared state, not
    /// a snapshot, so mounts added before or after this call are both served.
    pub fn set_webhook_mounts(
        &mut self,
        table: Arc<crate::gateway::webhook_receiver::WebhookMountTable>,
    ) {
        self.webhook_mounts = table;
    }

    /// Install the `SharedTokenManager` (vault handle). Also publishes the
    /// process-global (`SharedTokenManager::set_global`) that vault
    /// consumers resolve outside the request path.
    pub fn set_shared_token_manager(&mut self, manager: Arc<SharedTokenManager>) {
        SharedTokenManager::set_global(manager.clone());
        self.shared_token_mgr = Some(manager);
    }

    /// Install the `DeviceTokenManager` for bootstrap-ticket / per-device-token
    /// authentication in the WebSocket `connect` handshake.
    pub fn set_device_token_manager(
        &mut self,
        manager: Arc<crate::gateway::security::DeviceTokenManager>,
    ) {
        self.device_token_mgr = Some(manager);
    }

    /// Install the `SecurityStore` so the WS node connect/disconnect paths
    /// can stamp enrolled-node `last_seen_at` (offline fleet view honesty).
    pub fn set_security_store(&mut self, store: Arc<crate::gateway::security::SecurityStore>) {
        self.security_store = Some(store);
    }

    /// Install the whiteboard `CanvasStore` (enables the `/canvas-asset/...`
    /// byte route in `build_router`). Pass the same Arc the `canvas.*` RPC
    /// handlers were registered with — one instance owns the per-canvas
    /// locks and the event bus.
    pub fn set_canvas_store(&mut self, store: Arc<crate::canvas::CanvasStore>) {
        self.canvas_store = Some(store);
    }

    /// Install the `SecurityAuditLog` so the WS auth path records a forensic
    /// trail of remote-connection auth failures and flood-guard closes. Fed by
    /// a dedicated drain (see the start command); `None` leaves auth events
    /// unrecorded, matching pre-wiring behavior.
    pub fn set_audit_log(&mut self, log: crate::security::audit::SecurityAuditLog) {
        self.audit_log = Some(log);
    }

    /// A clone of the installed [`SecurityAuditLog`], for boot-time wiring
    /// that hands a *handler* its own sender rather than reading it off the
    /// connection (`trace.list` / `trace.get` record cross-user content reads
    /// — see `handlers::trace_replay`). `None` before
    /// [`GatewayServer::set_audit_log`] runs and in test/probe constructors,
    /// which is why any registration that wants it must be ordered after the
    /// setter; the trace registration says so at its own call site.
    #[must_use]
    pub fn audit_log(&self) -> Option<crate::security::audit::SecurityAuditLog> {
        self.audit_log.clone()
    }

    /// Install the `SessionStore` so the WS event-delivery loop can resolve
    /// session ownership for the owner-scoped event filter (P1 data
    /// isolation, spec §5.4 — `event_visibility::EventVisibilityIndex`).
    /// `None` (unset) skips that 4th filter term entirely, matching pre-P1
    /// delivery behavior — the same zero-change guarantee every other
    /// `Option<Arc<...>>` dependency on this struct provides.
    pub fn set_session_store(
        &mut self,
        store: Arc<dyn crate::gateway::session_store::SessionStore>,
    ) {
        self.session_store = Some(store);
    }

    /// Install the `TeamStore` so the WS event-delivery loop can resolve the
    /// OWNER of a `team.<id>.*` frame — the raw-string event plane that carries
    /// team chat bodies and has no `GatewayEventFrame` variant behind it (see
    /// `event_visibility`'s module doc). Pass the same ownership-scoped handle
    /// every other consumer receives (`builder::agent_init::coord_stores`);
    /// leaving it unset denies those frames rather than broadcasting them.
    pub fn set_team_store(&mut self, store: Arc<dyn crate::teams::TeamStore>) {
        self.team_store = Some(store);
    }

    /// Get the current number of active connections
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Build a unified axum Router with WebSocket + `ControlPlane` UI routes.
    /// WebSocket connections are handled at `/ws`, everything else serves the Panel UI.
    pub fn build_router(&self) -> Router {
        // Build the middleware chain ONCE here (cloned per connection) rather
        // than per-connection, so the global request-state registry is
        // installed a single time and its counters accumulate across
        // connections instead of resetting on every connect.
        let middleware_chain =
            MiddlewareChain::new(self.handlers.clone(), self.rate_limiter.clone());

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
            max_connections: self.config.max_connections,
            max_connections_per_ip: self.config.max_connections_per_ip,
            presence: self.presence.clone(),
            state_versions: self.state_versions.clone(),
            rate_limiter: self.rate_limiter.clone(),
            lane_manager: self.lane_manager.clone(),
            idempotency_guard: self.idempotency_guard.clone(),
            event_scope_guard: self.event_scope_guard.clone(),
            audit_log: self.audit_log.clone(),
            trusted_proxy_enabled: self.config.trusted_proxy_enabled,
            trusted_proxy_ips: handler::parse_trusted_ips(&self.config.trusted_proxy_ips),
            allow_insecure_remote: self.config.allow_insecure_remote,
            tls_enabled: self.config.tls_enabled,
            ready: self.ready.clone(),
            instance_id: self.instance_id.clone(),
            started_at_unix: self.started_at_unix,
            ping_interval_secs: self.config.ping_interval_secs,
            idle_timeout_secs: self.config.idle_timeout_secs,
            require_idempotency_key: self.config.require_idempotency_key,
            shared_token_mgr: self.shared_token_mgr.clone(),
            device_token_mgr: self.device_token_mgr.clone(),
            security_store: self.security_store.clone(),
            middleware_chain,
            origin_policy: Arc::new(if self.config.allow_any_origin {
                crate::gateway::origin_policy::OriginPolicy::allow_any()
            } else {
                crate::gateway::origin_policy::OriginPolicy::new(
                    self.config.allowed_origins.clone(),
                )
            }),
            node_registry: self.node_registry.clone(),
            exec_approval_manager: self.exec_approval_manager.clone(),
            session_store: self.session_store.clone(),
            team_store: self.team_store.clone(),
            event_visibility: self.event_visibility.clone(),
        });

        // Strip query strings from the Panel/control-plane fallback before any
        // static-file or SPA handler sees the request. This prevents bootstrap
        // tickets, legacy tokens, or device tokens from appearing in server logs
        // or error traces even if a future tracing layer is enabled.
        let control_plane =
            create_control_plane_router().layer(super::middleware::RedactQueryLayer::new());

        // OpenAI-compatible API routes (/v1/models, /v1/health, /v1/chat/completions)
        let openai_state = Arc::new(OpenAiApiState {
            server_id: format!("aleph-{}", self.addr),
            // Live read of the current bearer token from SharedTokenManager.
            // Using a closure (rather than a snapshot `Option<String>`)
            // means `SharedTokenManager::rotate` immediately revokes the
            // previously issued token — previously the snapshot was taken
            // at boot and `/v1/*` would accept the rotated-out token
            // indefinitely.
            api_token: {
                let mgr = self.shared_token_manager.clone();
                Arc::new(move || mgr.get_current_token())
                    as Arc<dyn Fn() -> Option<String> + Send + Sync>
            },
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

        // Capability-gated artifact bytes. Registered as a real route so it is
        // matched before `control_plane`, which would otherwise answer
        // `/artifact/...` with the Panel's SPA shell. It carries its own copy of
        // the transport / origin / rate-limit guards — see `artifact_route`,
        // which explains why none of `/ws`'s protections are inherited.
        let artifacts = match crate::artifacts::ArtifactStore::default_root() {
            Ok(root) => Some(artifact_route::artifact_routes(Arc::new(
                artifact_route::ArtifactRouteState::new(
                    Arc::new(crate::artifacts::ArtifactStore::new(root)),
                    shared.origin_policy.clone(),
                    shared.trusted_proxy_enabled,
                    shared.trusted_proxy_ips.clone(),
                    shared.allow_insecure_remote,
                    shared.tls_enabled,
                ),
            ))),
            Err(e) => {
                // Not fatal: the Panel simply has no artifact bytes to show.
                warn!(error = %e, "artifact byte route not mounted");
                None
            }
        };

        // Capability-gated whiteboard asset bytes — the artifact route's twin
        // (registered as a real route so `control_plane`'s SPA fallback never
        // answers `/canvas-asset/...`). Mounted only when boot installed the
        // one shared `CanvasStore`; see `canvas_asset_route` for why none of
        // `/ws`'s protections are inherited.
        let canvas_assets = self.canvas_store.as_ref().map(|store| {
            canvas_asset_route::canvas_asset_routes(Arc::new(
                canvas_asset_route::CanvasAssetRouteState::new(
                    store.clone(),
                    shared.origin_policy.clone(),
                    shared.trusted_proxy_enabled,
                    shared.trusted_proxy_ips.clone(),
                    shared.allow_insecure_remote,
                    shared.tls_enabled,
                ),
            ))
        });

        let mut router = Router::new()
            .route("/ws", get(handler::ws_upgrade_handler))
            .route("/health", get(probe::handle_health))
            .route("/ready", get(probe::handle_ready))
            .route("/metrics", get(metrics_endpoint::handle_metrics))
            .fallback_service(control_plane)
            .with_state(shared)
            .merge(openai);

        if let Some(artifacts) = artifacts {
            router = router.merge(artifacts);
        }

        if let Some(canvas_assets) = canvas_assets {
            router = router.merge(canvas_assets);
        }

        // Merge A2A routes if the subsystem is enabled
        if let Some(a2a_state) = &self.a2a_state {
            let a2a = crate::a2a::adapter::server::a2a_routes(a2a_state.clone());
            router = router.merge(a2a);
        }

        // Spec C: mount admin IPC router under /v1/admin if configured.
        if let Some(admin) = self.admin_router.clone() {
            router = router.nest("/v1/admin", admin);
        }

        // Channel webhook ingestion. One constant route over the shared mount
        // table; auth is per-handler HMAC, the same posture as /metrics and
        // /a2a — see the design spec.
        router = router.merge(crate::gateway::webhook_receiver::WebhookReceiver::router(
            self.webhook_mounts.clone(),
        ));

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

        // Background: prune settled request-lifecycle entries every 60s.
        //
        // `MetricsService::call` inserts one `RequestStateData` per JSON-RPC
        // request and only ever transitions it to a terminal state — nothing
        // removes it. `cleanup` was written, tested and then never called from
        // production, so the map grew one entry per RPC for the whole process
        // lifetime while both of its siblings above were pruned. A single
        // `voice.stream.audio` mic session is ~5 req/s, so this was tens of MB
        // a day of monotonic heap growth in a daemon designed to run for weeks,
        // with no error and no gauge that would show it.
        //
        // The handle is resolved on every tick, not captured here: the
        // registry is installed by `MiddlewareChain::new` inside
        // `build_router`, and this function is not ordered against it. A
        // captured `Option` would freeze whatever was true at spawn time and
        // could silently prune nothing forever.
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let Some(reg) = crate::gateway::middleware::request_state::get_global_registry()
                else {
                    continue; // chain not built yet — nothing has been inserted either
                };
                // Retention comparable to the idempotency window above. This
                // only removes entries that already reached a terminal state,
                // and `remove` deliberately does not decrement terminal
                // counters — so `snapshot().completed` / `.failed` stay
                // cumulative-since-boot, which is what the `/metrics` counters
                // promise. Pruning must not, and does not, walk them back.
                let pruned = reg.cleanup(300_000);
                if pruned > 0 {
                    debug!("Pruned {} settled request-state entries", pruned);
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

    /// Refuse to boot if the gateway would serve plaintext to the network, and
    /// — for the binds that *do* pass — say so once, out loud.
    ///
    /// Two independent verdicts, deliberately evaluated in this order:
    /// [`insecure_exposure_refused`] answers *"are the bytes readable"*,
    /// [`network_exposure_warning`] answers *"who can open the socket at all"*.
    /// A TLS-enabled LAN bind clears the first and still earns the second —
    /// encryption is not reachability, and the operator handing out a credential
    /// is handing out full operator authority.
    fn check_network_exposure(&self) -> Result<(), GatewayError> {
        if let Some(msg) = insecure_exposure_refused(
            self.addr.ip().is_loopback(),
            self.config.tls_enabled,
            self.config.trusted_proxy_enabled,
            self.config.allow_insecure_remote,
        ) {
            return Err(GatewayError::ConnectionError(msg));
        }
        if let Some(msg) = network_exposure_warning(
            &self.addr,
            self.config.tls_enabled,
            self.config.trusted_proxy_enabled,
            self.config.allow_insecure_remote,
        ) {
            warn!("{msg}");
        }
        Ok(())
    }

    /// Run the Gateway server
    ///
    /// This method runs indefinitely, accepting new connections and
    /// processing messages. Each connection is handled in its own task.
    pub async fn run(&self) -> Result<(), GatewayError> {
        self.spawn_background_tasks();
        let router = self.build_router();
        self.check_network_exposure()?;

        // Native TLS tiers terminate in-process via axum-server's own listener;
        // plaintext binds its own listener below and stays on axum::serve. The
        // bind is deliberately *not* hoisted above this branch — axum-server
        // binds `self.addr` itself inside `.serve()`, so pre-binding here too
        // would double-bind the port and fail with "address already in use".
        if self.config.tls_enabled {
            install_ring_provider();
            let tls_cfg = crate::gateway::config::GatewayTlsConfig {
                enabled: self.config.tls_enabled,
                cert_path: self.config.tls_cert_path.clone(),
                key_path: self.config.tls_key_path.clone(),
                san: self.config.tls_san.clone(),
            };
            let tls_dir = crate::utils::paths::get_data_dir()
                .map_err(|e| GatewayError::ConnectionError(format!("data dir: {e}")))?
                .join("tls");
            let (cert_pem, key_pem, fp) = crate::gateway::tls::load_or_generate(&tls_cfg, &tls_dir)
                .await
                .map_err(|e| GatewayError::ConnectionError(format!("TLS material: {e}")))?;
            info!(
                "Aleph listening on https://{} (wss:// on /ws), cert fp {fp}",
                self.addr
            );
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert_pem, key_pem)
                .await
                .map_err(|e| GatewayError::ConnectionError(format!("rustls config: {e}")))?;
            axum_server::bind_rustls(self.addr, tls)
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;
            return Ok(());
        }

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
        self.check_network_exposure()?;

        // Native TLS tiers terminate in-process via axum-server's own listener;
        // plaintext binds its own listener below and stays on axum::serve. The
        // bind is deliberately *not* hoisted above this branch — axum-server
        // binds `self.addr` itself inside `.serve()`, so pre-binding here too
        // would double-bind the port and fail with "address already in use".
        if self.config.tls_enabled {
            install_ring_provider();
            let tls_cfg = crate::gateway::config::GatewayTlsConfig {
                enabled: self.config.tls_enabled,
                cert_path: self.config.tls_cert_path.clone(),
                key_path: self.config.tls_key_path.clone(),
                san: self.config.tls_san.clone(),
            };
            let tls_dir = crate::utils::paths::get_data_dir()
                .map_err(|e| GatewayError::ConnectionError(format!("data dir: {e}")))?
                .join("tls");
            let (cert_pem, key_pem, fp) = crate::gateway::tls::load_or_generate(&tls_cfg, &tls_dir)
                .await
                .map_err(|e| GatewayError::ConnectionError(format!("TLS material: {e}")))?;
            info!("Aleph listening on https://{}", self.addr);
            info!("  WebSocket: wss://{}/ws", self.addr);
            info!("  TLS cert SHA-256 fingerprint: {fp}");
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert_pem, key_pem)
                .await
                .map_err(|e| GatewayError::ConnectionError(format!("rustls config: {e}")))?;
            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();
            tokio::spawn(async move {
                let _ = shutdown.await;
                shutdown_handle.graceful_shutdown(Some(Duration::from_secs(3)));
            });
            axum_server::bind_rustls(self.addr, tls)
                .handle(handle)
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;
            return Ok(());
        }

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

/// Install the process-wide rustls crypto provider once (idempotent). Native
/// TLS tiers need a default `CryptoProvider` installed before the first
/// `RustlsConfig::from_pem` call — both `ring` and `aws-lc-rs` are compiled
/// into the dependency graph, so relying on a single-implementation default
/// is not safe; pin `ring` explicitly. `install_default` returns `Err` if a
/// provider is already installed (fine — idempotent, first writer wins).
fn install_ring_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Boot-time verdict: `Some(diagnostic)` when the server would expose plaintext
/// to the network and must refuse to start. Loopback bind, any native-TLS tier,
/// a trusted-proxy (TLS-terminating) upstream, or an explicit
/// `allow_insecure_remote` all pass.
fn insecure_exposure_refused(
    host_is_loopback: bool,
    tls_enabled: bool,
    trusted_proxy_enabled: bool,
    allow_insecure_remote: bool,
) -> Option<String> {
    if host_is_loopback || tls_enabled || trusted_proxy_enabled || allow_insecure_remote {
        return None;
    }
    Some(
        "gateway would serve PLAINTEXT on a non-loopback interface. Refusing to start. \
         Fix: enable [gateway.tls], OR front it with a TLS reverse proxy and set \
         [gateway.trusted_proxy] enabled = true, OR knowingly set \
         [gateway] allow_insecure_remote = true."
            .to_string(),
    )
}

/// Boot-time advisory: `Some(line)` when the socket is reachable from beyond
/// this machine. Loopback returns `None` — that is the zero-config desktop
/// install and it has nothing to announce.
///
/// This is the line [`SECURITY.md`'s "Network boundary = reachability"] section
/// promises and, until now, did not exist. It is deliberately **not** folded
/// into [`insecure_exposure_refused`]: that function answers *"are the bytes
/// readable"* and lets a TLS'd LAN bind through, while the fact worth saying to
/// an operator is *"anything that can route to this box can now open the
/// socket"*. Encryption does not shrink the audience.
///
/// The transport clause is a fourth arm rather than three: the combination that
/// reaches neither TLS, nor a trusted proxy, nor an explicit opt-in is exactly
/// the one the boot gate above refuses, so it never reaches a running server —
/// returning `None` there keeps the refusal message the only thing printed.
///
/// [`SECURITY.md`'s "Network boundary = reachability"]: ../../../docs/reference/SECURITY.md
fn network_exposure_warning(
    addr: &SocketAddr,
    tls_enabled: bool,
    trusted_proxy_enabled: bool,
    allow_insecure_remote: bool,
) -> Option<String> {
    if addr.ip().is_loopback() {
        return None;
    }
    let transport = if tls_enabled {
        "encrypted by native TLS"
    } else if trusted_proxy_enabled {
        "encrypted by the trusted reverse proxy"
    } else if allow_insecure_remote {
        "PLAINTEXT — allow_insecure_remote = true"
    } else {
        return None;
    };
    Some(format!(
        "gateway is bound to {addr}, which is reachable from the network ({transport}). \
         Remote connections are walled until they present a device token, a bootstrap \
         ticket, or the shared gateway token — but any credential that passes grants \
         FULL operator authority (PTY and shell included). Share it only over a trusted \
         channel and rotate it with `gateway.token.rotate` if it may have leaked. \
         Bind [gateway] host = \"127.0.0.1\" to close this."
    ))
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
    use axum::http::StatusCode;

    /// Every unbounded per-request map the middleware chain writes into needs
    /// a pruner here, or it grows for the whole process lifetime.
    ///
    /// Stated as "the state registry must be pruned" rather than "there are
    /// three spawns", because the failure this guards was not a deleted arm —
    /// it was an arm nobody ever wrote, while `RequestStateRegistry::cleanup`
    /// sat next to its own passing unit tests. `#[must_use]` cannot fire for a
    /// function with no callers, and dead-code lints do not fire for a `pub`
    /// one that its tests call.
    #[test]
    fn every_registry_the_chain_inserts_into_has_a_background_pruner() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/gateway/server/mod.rs"
        ))
        .expect("this file is readable from its own test");
        let production = crate::utils::source_scan::production_prefix(&src);
        let code = crate::utils::source_scan::strip_comment_lines(&production);

        let start = code
            .find("fn spawn_background_tasks(")
            .expect("spawn_background_tasks still exists");
        // Bounded by the next item at the same level; the heartbeat spawn is
        // the last arm, so scanning to the end of the impl is enough.
        let body = &code[start..];

        for (what, needle) in [
            ("rate limiter", "prune_stale("),
            ("idempotency guard", ".prune()"),
            ("request-state registry", ".cleanup("),
        ] {
            assert!(
                body.contains(needle),
                "spawn_background_tasks never prunes the {what}: `{needle}` \
                 does not appear in its body. An unbounded map written once \
                 per request grows for the process lifetime with no error and \
                 no gauge"
            );
        }
        // The registry handle must be resolved INSIDE the tick loop. It is
        // installed by `MiddlewareChain::new` in `build_router`, which is not
        // ordered against this function, so a handle captured at spawn time
        // can be `None` forever — a pruner that prunes nothing looks exactly
        // like the bug it was written to fix.
        let cleanup_at = body.find(".cleanup(").expect("checked above");
        let arm_start = body[..cleanup_at]
            .rfind("tokio::spawn(")
            .expect("the pruner lives in a spawned task");
        assert!(
            body[arm_start..cleanup_at].contains("get_global_registry()"),
            "the request-state pruner must resolve its registry handle inside \
             its own spawned task, on each tick — not capture it before the \
             middleware chain that installs it has been built"
        );
    }

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

    /// Multi-user role gate (spec §4.6), integration-level: a `"member"`
    /// caller must be refused at the `process_request` chokepoint before
    /// registry dispatch, on a real admin-family method
    /// (`config.schema` — one of the `config.` prefix's registered
    /// built-ins). An `"operator"` caller must reach the real handler and
    /// get a normal success response — proving the gate does not
    /// collaterally block the role it is supposed to pass.
    #[tokio::test]
    async fn member_is_refused_admin_methods_at_the_chokepoint() {
        let handlers_arc = Arc::new(HandlerRegistry::new());
        let chain = MiddlewareChain::new(
            handlers_arc.clone(),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"config.schema","params":{}}"#;

        let resp_member = crate::gateway::caller_identity::CALLER_ROLE
            .scope(
                Some("member".to_string()),
                handler::process_request(req, &chain),
            )
            .await;
        let parsed_member: JsonRpcResponse = serde_json::from_str(&resp_member).unwrap();
        assert!(
            parsed_member.is_error(),
            "member must be refused: {resp_member}"
        );
        assert_eq!(
            parsed_member.error.unwrap().code,
            crate::gateway::protocol::AUTH_REQUIRED,
            "refusal must use the same error code as the login wall"
        );

        let resp_operator = crate::gateway::caller_identity::CALLER_ROLE
            .scope(
                Some("operator".to_string()),
                handler::process_request(req, &chain),
            )
            .await;
        let parsed_operator: JsonRpcResponse = serde_json::from_str(&resp_operator).unwrap();
        assert!(
            parsed_operator.is_success(),
            "operator must pass the gate and reach the real handler: {resp_operator}"
        );

        // A caller with no CALLER_ROLE scope at all (internal/cron) is
        // trusted by the same predicate as operator — `current_caller_role()`
        // returns `None` outside a scope, and the gate only refuses `Some("member")`.
        let resp_internal = handler::process_request(req, &chain).await;
        let parsed_internal: JsonRpcResponse = serde_json::from_str(&resp_internal).unwrap();
        assert!(
            parsed_internal.is_success(),
            "internal/cron callers (no CALLER_ROLE scope) must pass the gate: {resp_internal}"
        );
    }

    /// Over-gating guard, sibling of `member_is_refused_admin_methods_at_the_chokepoint`:
    /// a `"member"` caller hitting a real, registered, member-open method
    /// (`health` — not in `method_admin::ADMIN_PREFIXES`) must reach the
    /// real handler and succeed, not be collaterally refused by the gate.
    #[tokio::test]
    async fn member_passes_a_member_open_method_at_the_chokepoint() {
        let handlers_arc = Arc::new(HandlerRegistry::new());
        let chain = MiddlewareChain::new(
            handlers_arc.clone(),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"health","params":{}}"#;

        let resp_member = crate::gateway::caller_identity::CALLER_ROLE
            .scope(
                Some("member".to_string()),
                handler::process_request(req, &chain),
            )
            .await;
        let parsed_member: JsonRpcResponse = serde_json::from_str(&resp_member).unwrap();
        assert!(
            parsed_member.is_success(),
            "member must reach a member-open method: {resp_member}"
        );
    }

    #[test]
    fn test_gateway_config_default() {
        let config = GatewayConfig::default();
        assert_eq!(config.max_connections, 1000);
        assert!(!config.allow_any_origin);
        assert!(config.allowed_origins.is_empty());
    }

    #[tokio::test]
    async fn node_registry_is_empty_on_fresh_server() {
        let server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
        assert!(server.node_registry.list_environments().is_empty());
    }

    #[test]
    fn boot_gate_refuses_only_plaintext_non_loopback() {
        // Default loopback install: allowed.
        assert!(super::insecure_exposure_refused(true, false, false, false).is_none());
        // Non-loopback plaintext, no proxy, not allowed ⇒ refuse.
        assert!(super::insecure_exposure_refused(false, false, false, false).is_some());
        // Non-loopback but native TLS ⇒ allowed.
        assert!(super::insecure_exposure_refused(false, true, false, false).is_none());
        // Non-loopback behind trusted proxy ⇒ allowed.
        assert!(super::insecure_exposure_refused(false, false, true, false).is_none());
        // Non-loopback plaintext but explicitly allowed ⇒ allowed.
        assert!(super::insecure_exposure_refused(false, false, false, true).is_none());
    }

    /// The warning is about REACHABILITY, so the encrypted tiers must still
    /// produce one — the whole point is that clearing the boot gate is not the
    /// same as being unexposed. A test that only checked the plaintext arm
    /// would pass against a version that warns exactly where the server refuses
    /// to start, i.e. nowhere a running server can be observed.
    #[test]
    fn a_non_loopback_bind_warns_on_every_tier_that_actually_boots() {
        let loopback: std::net::SocketAddr = "127.0.0.1:18790".parse().unwrap();
        let lan: std::net::SocketAddr = "0.0.0.0:18790".parse().unwrap();

        // Zero-config desktop install has nothing to announce.
        assert!(super::network_exposure_warning(&loopback, false, false, false).is_none());
        assert!(super::network_exposure_warning(&loopback, true, false, false).is_none());

        for (tls, proxy, allow, clause) in [
            (true, false, false, "native TLS"),
            (false, true, false, "trusted reverse proxy"),
            (false, false, true, "PLAINTEXT"),
        ] {
            let msg = super::network_exposure_warning(&lan, tls, proxy, allow)
                .expect("a non-loopback bind that boots must warn");
            assert!(
                msg.contains(clause),
                "the transport tier must be named in the line: {msg}"
            );
            assert!(
                msg.contains("0.0.0.0:18790"),
                "the operator needs the address that is exposed: {msg}"
            );
        }

        // The combination the boot gate refuses prints the refusal, not this.
        assert!(super::network_exposure_warning(&lan, false, false, false).is_none());
    }

    #[tokio::test]
    async fn webhook_prefix_is_always_routed_and_404s_when_nothing_is_mounted() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        // The route is a constant, present with or without configured
        // channels. That is what lets a channel created at runtime become
        // reachable without a restart.
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = GatewayServer::new(addr);
        let router = server.build_router();

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/none")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 404 from the dispatcher — NOT 405 from the SPA fallback. A 405 here
        // would mean the wildcard route is missing and the request fell through.
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn set_webhook_mounts_makes_a_mounted_path_reachable() {
        use crate::gateway::channel::{ChannelId, ChannelState, ChannelStatus};
        use crate::gateway::webhook_receiver::{WebhookMountTable, WebhookReceiver};
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = ChannelState::new(8);
        state.set_status(ChannelStatus::Connected).await;
        let _rx = state.inbound_subscribe();

        let table = Arc::new(WebhookMountTable::new());
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut server = GatewayServer::new(addr);
        server.set_webhook_mounts(Arc::clone(&table));
        let router = server.build_router();

        // Mounted AFTER build_router: the router holds the table, not a snapshot.
        table
            .mount(crate::gateway::webhook_receiver::WebhookMount {
                handler: Arc::new(AlwaysOkHandler),
                inbound: state.sender(),
                status: state.status_handle(),
                channel_id: ChannelId::new("probe"),
            })
            .await;

        let body = br#"{"text":"hi"}"#.to_vec();
        let sig = WebhookReceiver::compute_signature("probe-secret", &body);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/probe")
                    .header("x-webhook-signature", sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn panel_spa_paths_are_untouched_by_the_webhook_route() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        // Confining webhooks to one prefix is what makes SPA shadowing
        // unexpressible: `path = "/settings"` can no longer become a real
        // POST-only route that turns `GET /settings` into 405.
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = GatewayServer::new(addr);
        let router = server.build_router();

        for path in ["/", "/settings"] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{path} must still reach the Panel fallback"
            );
        }
    }

    #[test]
    fn build_router_registers_no_second_route_under_the_webhook_prefix() {
        // matchit lets `/webhook/foo` coexist with `/webhook/{*rest}` — the more
        // specific static route simply wins, with NO panic. So a future gateway
        // route under this prefix would silently steal a channel's webhook path.
        // axum cannot be asked what is in its route table, so scan the source of
        // the only function that builds it.
        //
        // Boundary: this scans only `server/mod.rs`. A `/webhook/...` route
        // registered in a router merged in from another file (`openai_routes`,
        // `a2a_routes`, `artifact_routes`, `control_plane`, the `/v1/admin`
        // nest) would NOT be caught — none exists today.
        //
        // The needles use the escaped-quote form (`.route(\"` here vs `.route("`
        // in the scanned text) precisely so the test does not match its own
        // source — correct but fragile; do not "clean up" that escaping.
        let src = include_str!("mod.rs");
        for (idx, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            let offends = code.contains(".route(\"/webhook")
                || code.contains(".nest(\"/webhook")
                || code.contains(".nest_service(\"/webhook");
            assert!(
                !offends,
                "server/mod.rs:{} registers a route under {}; channel webhooks \
                 must enter only through WebhookReceiver::router()",
                idx + 1,
                crate::gateway::webhook_receiver::WEBHOOK_ROUTE_PREFIX
            );
        }
    }

    /// Minimal `WebhookHandler` for router-level assertions: fixed secret,
    /// fixed path, produces no inbound messages so no subscriber is needed.
    struct AlwaysOkHandler;

    #[async_trait::async_trait]
    impl crate::gateway::webhook_receiver::WebhookHandler for AlwaysOkHandler {
        fn verify(&self, headers: &axum::http::HeaderMap, body: &[u8]) -> bool {
            let sig = headers
                .get("x-webhook-signature")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            crate::gateway::webhook_receiver::WebhookReceiver::verify_signature(
                "probe-secret",
                body,
                sig,
            )
        }

        async fn handle(
            &self,
            _headers: &axum::http::HeaderMap,
            _body: axum::body::Bytes,
        ) -> crate::gateway::channel::ChannelResult<Vec<crate::gateway::channel::InboundMessage>>
        {
            Ok(vec![])
        }

        fn path(&self) -> &str {
            "/webhook/probe"
        }
    }
}

#[cfg(test)]
mod channel_kind_tests {
    use super::*;
    use crate::gateway::surface::SurfaceKind;

    /// A connection whose ADDRESS is loopback but which is not local must not
    /// start as an operator.
    ///
    /// That pair is not hypothetical and it is not rare: it is what
    /// `trusted_proxy::resolve_client` produces for every client behind a
    /// same-host reverse proxy that does not send `X-Forwarded-For` — `ip`
    /// falls back to the proxy's own address (loopback) while `local` is
    /// false, because the hop is known to be a proxy. Deriving `caller_role`
    /// from the address rather than from the bit handed every internet client
    /// the zero-config operator grant, before any handshake.
    ///
    /// The guard is written on the PAIR rather than on either field alone,
    /// because either field alone reads as fine: a loopback address is normal,
    /// and `local: false` is normal. Only together do they name the bug.
    #[test]
    fn a_loopback_address_that_is_not_local_starts_as_guest() {
        let proxied = ConnectionState::new("127.0.0.1".parse().unwrap(), false);
        assert_eq!(
            proxied.caller_role, "guest",
            "a client reached through a trusted proxy hop must start at the \
             login wall — its address is the proxy's, not its own"
        );

        // The control: without it the assertion above is also satisfied by a
        // constructor that returns "guest" for everyone, which would silently
        // lock the desktop App out of its own machine.
        let desktop = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
        assert_eq!(
            desktop.caller_role, "operator",
            "the local desktop App over loopback is the implicit operator — \
             the zero-config grant this repo's trust model is built on"
        );
    }

    #[test]
    fn new_connection_has_no_channel_kind() {
        let cs = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
        assert_eq!(cs.channel_kind, None);
    }

    #[test]
    fn channel_kind_is_settable() {
        let mut cs = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
        cs.channel_kind = Some(SurfaceKind::Desktop);
        assert_eq!(cs.channel_kind, Some(SurfaceKind::Desktop));
    }
}

#[cfg(test)]
mod device_invalidation_tests {
    use super::*;

    fn authorized(conn: &str, device_id: Option<&str>) -> (String, ConnectionState) {
        let mut cs = ConnectionState::new("10.0.0.9".parse().unwrap(), false);
        cs.caller_role = "operator".to_string();
        cs.caller_user = Some(format!("u-{conn}"));
        cs.permissions = vec!["*".to_string()];
        cs.device_id = device_id.map(String::from);
        (conn.to_string(), cs)
    }

    #[tokio::test]
    async fn downgrades_only_the_revoked_device() {
        let conns: Arc<RwLock<HashMap<String, ConnectionState>>> = Arc::new(RwLock::new(
            [
                authorized("a", Some("device-7")),
                authorized("b", Some("device-7")),
                authorized("c", Some("device-8")),
                // Loopback / legacy-shared-token session: not device-bound.
                authorized("d", None),
            ]
            .into_iter()
            .collect(),
        ));

        assert_eq!(invalidate_device_sessions(&conns, "device-7").await, 2);

        let map = conns.read().await;
        for hit in ["a", "b"] {
            let s = &map[hit];
            assert_eq!(s.caller_role, "guest", "{hit} must fall behind the wall");
            assert_eq!(
                s.caller_user, None,
                "{hit} must lose its authenticated user alongside the role — \
                 caller_user is resolved together with caller_role"
            );
            assert!(s.permissions.is_empty(), "{hit} must lose event scope");
        }
        for spared in ["c", "d"] {
            assert_eq!(
                map[spared].caller_role, "operator",
                "{spared} must be untouched by a per-device revoke"
            );
            assert_eq!(
                map[spared].caller_user,
                Some(format!("u-{spared}")),
                "{spared} must keep its authenticated user"
            );
        }
    }

    #[tokio::test]
    async fn revoking_a_device_with_no_open_session_is_a_no_op() {
        let conns: Arc<RwLock<HashMap<String, ConnectionState>>> = Arc::new(RwLock::new(
            [authorized("a", Some("device-7"))].into_iter().collect(),
        ));
        assert_eq!(invalidate_device_sessions(&conns, "device-99").await, 0);
        assert_eq!(conns.read().await["a"].caller_role, "operator");
    }
}
