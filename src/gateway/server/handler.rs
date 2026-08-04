//! WebSocket connection handling
//!
//! Contains the connection lifecycle: upgrade, authentication, message dispatch,
//! event forwarding, and cleanup.

use crate::sync_primitives::Arc;
use axum::{
    extract::{
        ws::{CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    http::{header, HeaderMap},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify, RwLock};
use tokio::time::{interval_at, Instant as TokioInstant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::gateway::event_scope::EventScopeGuard;
use crate::gateway::handlers::events::{
    handle_list as handle_events_list, handle_subscribe, handle_unsubscribe, SubscriptionManager,
};
use crate::gateway::lane::{ChannelClass, LaneManager};
use crate::gateway::middleware::MiddlewareChain;
use crate::gateway::presence::{PresenceEntry, PresenceTracker};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, IDEMPOTENCY_KEY_REQUIRED, INTERNAL_ERROR,
    PARSE_ERROR, RATE_LIMITED,
};
use crate::gateway::rate_limiter::{scope_for_method, RateLimitError, RateLimitKey, RateLimiter};
use crate::gateway::state_version::StateVersionTracker;

use super::per_client_buffer::PerClientBuffer;
use super::{ConnectionState, GatewaySharedState};
use crate::gateway::security::SecurityStore;

/// Parse configured trusted-proxy IP strings into `IpAddr`, silently dropping
/// unparseable entries (fail-safe: a garbage entry just isn't trusted).
pub(super) fn parse_trusted_ips(raw: &[String]) -> Vec<IpAddr> {
    raw.iter()
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .collect()
}

/// Whether to refuse this upgrade for insecure transport. A non-loopback client
/// on an unencrypted leg is refused unless the operator set
/// `allow_insecure_remote`. Loopback is always allowed.
pub(super) fn refuse_insecure_remote(
    client_ip: IpAddr,
    secure: bool,
    allow_insecure_remote: bool,
) -> bool {
    !client_ip.is_loopback() && !secure && !allow_insecure_remote
}

/// Shared context for handling a WebSocket connection.
struct ConnectionContext {
    middleware_chain: MiddlewareChain,
    event_bus: Arc<GatewayEventBus>,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    subscription_manager: Arc<SubscriptionManager>,
    presence: Arc<PresenceTracker>,
    state_versions: Arc<StateVersionTracker>,
    rate_limiter: Arc<RateLimiter>,
    lane_manager: Arc<LaneManager>,
    idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
    event_scope_guard: Arc<EventScopeGuard>,
    /// Channel-class for lane priority. Derived once per connection from
    /// the peer address: loopback peers are classed as
    /// [`ChannelClass::Desktop`] so the local Panel can draw from the
    /// reserved desktop semaphore pool; everyone else is
    /// [`ChannelClass::Bot`].
    ///
    /// Known limitation: today there is no first-class "token issuer"
    /// metadata, so local bot adapters (Telegram/Slack daemons running
    /// on the same host as the gateway) also connect via loopback and
    /// will inherit Desktop priority. This is acknowledged by the
    /// Panel-first goal as an accepted trade-off until token issuance
    /// carries an explicit issuer marker.
    channel_class: ChannelClass,
    /// How often to send a WS-level Ping frame. See `GatewayConfig`.
    ping_interval_secs: u64,
    /// Close the connection if no inbound frame arrives within this many
    /// seconds. See `GatewayConfig`.
    idle_timeout_secs: u64,
    /// When true, every mutating RPC (Execute / Mutate / System lane)
    /// MUST carry an `idempotency_key` or it is rejected before lane
    /// dispatch with [`IDEMPOTENCY_KEY_REQUIRED`].
    require_idempotency_key: bool,
    /// Security store handle. Used by the cluster node connect/disconnect
    /// paths to stamp the enrolled device's `last_seen_at` so the offline
    /// view in `environments.list` stays honest. `None` in probe/legacy
    /// wiring — stamping is then skipped.
    security_store: Option<Arc<SecurityStore>>,
    /// Device-token manager for bootstrap-ticket / per-device-token auth.
    device_token_mgr: Option<Arc<crate::gateway::security::DeviceTokenManager>>,
    /// Resolved client IP (the trusted-proxy-forwarded client behind a
    /// reverse proxy, else the raw socket peer). Used for the per-IP
    /// connection cap and rate-limit identity.
    client_ip: IpAddr,
    /// Cluster node registry (shared Arc). The connect handler registers a
    /// `role:node` connection here and cleanup deregisters it.
    node_registry: Arc<crate::cluster::NodeRegistry>,
    /// Shared exec-approval manager for node-initiated approvals (cluster ③).
    /// `None` ⇒ `node.approval.request` is refused.
    exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
    /// Security audit log for remote-connection auth forensics. Records
    /// `AuthFailure` on a rejected remote `connect` and `RateLimited` when the
    /// flood guard closes an unauthorized connection. `None` ⇒ auth events are
    /// not persisted (probe/degraded wiring).
    audit_log: Option<crate::security::audit::SecurityAuditLog>,
}

/// axum handler: upgrade HTTP connection to WebSocket at `/ws`
pub(super) async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<GatewaySharedState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    // IP-keyed abuse protections (per-IP cap, rate limiting), the security
    // audit log, AND the connect-auth loopback test all read `client_ip`.
    // Behind a trusted proxy the transport peer is the proxy, so resolve the
    // real client from forwarding headers first (spoof-safe: untrusted peers'
    // headers are ignored). `secure` = native TLS OR the proxy's XFF-Proto.
    let resolved = crate::gateway::trusted_proxy::resolve_client(
        peer_addr.ip(),
        &headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let client_ip = resolved.ip;
    let secure = state.tls_enabled || resolved.secure;

    // Insecure-transport guard: a non-loopback client on an unencrypted leg
    // is refused unless the operator opted into `allow_insecure_remote`.
    // Loopback is always allowed. Must run before any auth/origin decision
    // is made over what could be a plaintext, sniffable/tamperable leg.
    if refuse_insecure_remote(client_ip, secure, state.allow_insecure_remote) {
        warn!(
            peer = %peer_addr, client = %client_ip,
            "rejected WebSocket upgrade: insecure transport to a remote client — \
             enable [gateway.tls], or a TLS reverse proxy + [gateway.trusted_proxy], \
             or set allow_insecure_remote=true"
        );
        return (
            axum::http::StatusCode::UPGRADE_REQUIRED,
            "TLS required for remote connections",
        )
            .into_response();
    }

    // Cross-origin / DNS-rebinding guard. A browser always attaches an
    // `Origin` header to a WS upgrade and cannot forge it, so a malicious page
    // that reaches this loopback socket is rejected when its origin is neither
    // same-origin nor allow-listed. Native clients (CLI, bots, bridges) send
    // no `Origin` and pass through untouched. Enforces the documented
    // `[gateway] allowed_origins` contract (`allow_any_origin` bypasses).
    {
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
        if !state.origin_policy.is_allowed(origin, host) {
            warn!(
                peer = %peer_addr,
                origin = origin.unwrap_or("<none>"),
                "rejected WebSocket upgrade: disallowed origin (cross-origin / DNS-rebinding guard)"
            );
            return (axum::http::StatusCode::FORBIDDEN, "origin not allowed").into_response();
        }
    }

    // Check connection limits before upgrading. One read guard covers both the
    // global cap and the per-IP cap so we hold the lock once.
    {
        let conns = state.connections.read().await;
        if conns.len() >= state.max_connections {
            warn!("Connection limit reached, rejecting {}", peer_addr);
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Connection limit reached",
            )
                .into_response();
        }

        // Per-IP concurrent-connection cap: bounds a single client IP from
        // exhausting global connection slots with sockets that never
        // authenticate (preauth flood / slot exhaustion). Loopback (Panel,
        // local CLI, desktop shell) is exempt — it legitimately opens several
        // connections at once. `0` disables the cap. Established connections
        // carry their resolved `client_ip`, so the count isolates real clients
        // even when many share one reverse-proxy socket address.
        let per_ip_cap = state.max_connections_per_ip;
        if per_ip_cap > 0 && !client_ip.is_loopback() {
            let same_ip = conns.values().filter(|c| c.client_ip == client_ip).count();
            if same_ip >= per_ip_cap {
                warn!(
                    "Per-IP connection cap ({}) reached for {}, rejecting",
                    per_ip_cap, client_ip
                );
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "Per-IP connection limit reached",
                )
                    .into_response();
            }
        }
    }

    // Derive the channel class for Lane priority. Loopback connections
    // (Tauri Panel, local CLI…) are treated as Desktop and get first dibs
    // on the reserved desktop semaphore pool; everyone else falls back to
    // the shared pool. See `ConnectionContext::channel_class` for the
    // accepted trade-off.
    let channel_class = if client_ip.is_loopback() {
        ChannelClass::Desktop
    } else {
        ChannelClass::Bot
    };

    ws.on_upgrade(move |socket| async move {
        let ctx = ConnectionContext {
            // Shared chain built once at server construction (cloning shares the
            // global request-state registry instead of resetting it per connect).
            middleware_chain: state.middleware_chain.clone(),
            event_bus: state.event_bus.clone(),
            connections: state.connections.clone(),
            subscription_manager: state.subscription_manager.clone(),
            presence: state.presence.clone(),
            state_versions: state.state_versions.clone(),
            rate_limiter: state.rate_limiter.clone(),
            lane_manager: state.lane_manager.clone(),
            idempotency_guard: state.idempotency_guard.clone(),
            event_scope_guard: state.event_scope_guard.clone(),
            channel_class,
            ping_interval_secs: state.ping_interval_secs,
            idle_timeout_secs: state.idle_timeout_secs,
            require_idempotency_key: state.require_idempotency_key,
            security_store: state.security_store.clone(),
            device_token_mgr: state.device_token_mgr.clone(),
            client_ip,
            node_registry: state.node_registry.clone(),
            exec_approval_manager: state.exec_approval_manager.clone(),
            audit_log: state.audit_log.clone(),
        };
        if let Err(e) = handle_connection(socket, peer_addr, ctx).await {
            error!("Connection error from {}: {}", peer_addr, e);
        }
    })
}

/// Build the JSON-RPC `connection.warning` frame announcing dropped events.
///
/// Shared by the per-client drain path and the global-bus overflow watchdog so
/// both surface identical `events_overflow` diagnostics (with `advice:reconnect`)
/// before the connection is closed with WS code 1008.
fn overflow_warning_frame(dropped: u64, total_overflow: u64) -> String {
    serde_json::json!({
        "method": "event",
        "params": {
            "topic": "connection.warning",
            "data": {
                "reason": "events_overflow",
                "dropped": dropped,
                "total_overflow": total_overflow,
                "advice": "reconnect"
            }
        }
    })
    .to_string()
}

/// Drain the global event bus into a single client's per-client buffer.
///
/// A `tokio::broadcast` receiver that falls behind yields `RecvError::Lagged(n)`
/// but **remains valid** — subsequent `recv()` calls keep working, having skipped
/// `n` messages. We mirror the per-client drain policy here: account the dropped
/// events on the buffer's shared overflow metric and keep forwarding. The
/// connection's idle/ping watchdog observes that metric and closes the socket
/// with code 1008 so the client reconnects and re-syncs from the hello snapshot.
///
/// The forwarder terminates on either of two terminal conditions: the global bus
/// closes (`RecvError::Closed`, i.e. process shutdown) **or** the per-client
/// receiver is dropped (connection closed). The latter makes `try_send` fail —
/// the bounded broadcast errors only when it has zero receivers, and
/// `handle_connection` holds the sole receiver for the connection's lifetime —
/// so a failed send unambiguously means "socket gone", and we stop instead of
/// looping forever against a dead client. (Previously the send error was
/// discarded with `let _ =`, so this task — and its live global-bus receiver —
/// leaked for the whole process lifetime on *every* WS disconnect, cloning every
/// published event to a dead receiver: O(dead_connections) fan-out growth on a
/// long-running daemon. Separately, an earlier `while let Ok(..)` loop treated
/// `Lagged` as fatal, silently killing the task and event-starving a live socket.)
async fn forward_bus_to_client(mut rx: broadcast::Receiver<String>, buffer: PerClientBuffer) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                // A failed send == the per-client receiver was dropped == the
                // connection closed. Reap this task instead of leaking it.
                if buffer.try_send(event).is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                buffer.metrics().add_overflow(n);
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Wire `topic` under which the shared-token rotation event is published (see
/// [`crate::gateway::events::GatewayEventFrame::TokenRotated`]). A drift-guard
/// test keeps this equal to `GatewayEventFrame::TokenRotated.topic_name()`.
const TOKEN_ROTATED_TOPIC: &str = "gateway.token.rotated";

/// Whether the given serialized event frame is a `token_rotated` notification.
///
/// `GatewayEvents::publish_frame` wraps every non-stream event as the TopicEvent
/// wire form `{"topic": "<name>", "data": <frame>}`, so the discriminant is the
/// **top-level `topic`**, not a top-level `type`. (The inner `data` still carries
/// the serde tag `{"type":"token_rotated"}`, but the forward loop only ever sees
/// the wrapped form.) Reading `type` here silently never matched, which left the
/// rotation kick — the documented "revoke all remotes" hammer — inert: open
/// remote Panels kept operator authority until idle timeout. Parses the JSON once
/// and matches `topic == TOKEN_ROTATED_TOPIC`. Pure, host-testable.
fn is_token_rotated_frame(event_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(event_json)
        .ok()
        .and_then(|v| {
            v.get("topic")
                .and_then(|t| t.as_str())
                .map(|s| s == TOKEN_ROTATED_TOPIC)
        })
        .unwrap_or(false)
}

/// Whether this connection must be torn down because the shared token was
/// rotated. True only for a `token_rotated` event on a *remote* (non-loopback)
/// connection — loopback is always operator and never token-gated, so it is
/// unaffected. Pure for host testing.
fn rotated_should_close_remote(event_json: &str, is_loopback: bool) -> bool {
    !is_loopback && is_token_rotated_frame(event_json)
}

/// Wire `topic` under which a single paired-device revocation is published (see
/// [`crate::gateway::events::GatewayEventFrame::DeviceRevoked`]). A drift-guard
/// test keeps this equal to `DeviceRevoked{..}.topic_name()`.
const DEVICE_REVOKED_TOPIC: &str = "gateway.device.revoked";

/// The `device_id` carried by a `device_revoked` event frame, or `None` for any
/// other event.
///
/// Same wire shape as [`is_token_rotated_frame`]: `publish_frame` wraps every
/// non-stream event as `{"topic": …, "data": <frame>}`, so the discriminant is
/// the **top-level `topic`** and the payload lives under `data`. Reading a
/// top-level `type` here is the mistake that once turned the rotation kick into
/// a dud — do not reintroduce it.
fn device_revoked_id(event_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(event_json).ok()?;
    if v.get("topic").and_then(|t| t.as_str()) != Some(DEVICE_REVOKED_TOPIC) {
        return None;
    }
    v.get("data")
        .and_then(|d| d.get("device_id"))
        .and_then(|d| d.as_str())
        .map(String::from)
}

/// Whether this connection must be torn down because *its own* paired device was
/// revoked. True only when the event names exactly the device this session
/// authenticated as — an unbound session (loopback, legacy shared token, or a
/// still-walled connection) has no `device_id` and is never matched, so a
/// per-device revoke can never collaterally kick the operator's own local App.
/// Pure for host testing.
fn device_revoked_should_close(event_json: &str, session_device_id: Option<&str>) -> bool {
    match (device_revoked_id(event_json), session_device_id) {
        (Some(revoked), Some(mine)) => revoked == mine,
        _ => false,
    }
}

/// Resolve the `(caller_role, caller_user)` pair stamped onto
/// `ConnectionState` at a `connect` handshake, given the authorization
/// verdict `resolve_connect_auth` already decided. Pure — host-testable
/// without a live WS socket, unlike the handshake it's extracted from.
///
/// `authorized == false` stays walled (guest, no user) exactly as before
/// per-user resolution existed. `authorized == true` resolves the bound
/// device's user via [`resolve_connection_identity`](crate::gateway::handlers::connect::resolve_connection_identity)
/// when a security store is available — loopback and legacy unbound-device
/// paths still resolve to the implicit owner as operator (zero-change
/// guarantee), but a device bound to a deactivated user is walled here even
/// though its token was valid. With no store wired (probe/test server),
/// authorized falls back to the implicit owner as operator, unchanged from
/// before per-user resolution existed.
fn resolve_stamped_identity(
    authorized: bool,
    is_loopback: bool,
    device_id: Option<&str>,
    store: Option<&crate::gateway::security::store::SecurityStore>,
) -> (Option<String>, &'static str) {
    if !authorized {
        return (None, "guest");
    }
    match store {
        Some(store) => crate::gateway::handlers::connect::resolve_connection_identity(
            is_loopback,
            device_id,
            store,
        ),
        None => (
            Some(crate::gateway::security::store::OWNER_USER_ID.to_string()),
            "operator",
        ),
    }
}

/// Handle a single WebSocket connection
async fn handle_connection(
    socket: WebSocket,
    peer_addr: SocketAddr,
    ctx: ConnectionContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut write, mut read) = socket.split();
    let conn_id = format!("{peer_addr}");

    info!("New WebSocket connection: {}", conn_id);

    let (buffer, mut client_event_rx) = PerClientBuffer::new();
    let buffer_metrics = buffer.metrics().clone();

    tokio::spawn(forward_bus_to_client(ctx.event_bus.subscribe(), buffer));

    // Reverse-RPC outbound channel for this connection. Frames pushed here are
    // written verbatim to the socket by the dedicated select arm below (they
    // bypass the EventBus topic/scope filtering, which would drop RPC frames).
    // Registered under conn_id so reverse-RPC callers can reach this specific
    // connection; deregistered on cleanup.
    let (rpc_out_tx, mut rpc_out_rx) = tokio::sync::mpsc::channel::<String>(64);
    // Clone kept for node-initiated request replies (cluster ③): a spawned
    // approval task sends its JSON-RPC response here; the select arm below
    // writes it to the socket.
    let rpc_out_tx_replies = rpc_out_tx.clone();
    // Slow-consumer teardown: a reverse-RPC call whose outbound queue wedges (the
    // peer stopped draining = half-open / slow consumer) fires this so the select
    // loop below exits and runs the normal cleanup — reaping a zombie the inbound
    // idle-watchdog would miss for a write-only wedge. Maps openclaw
    // `rejectSlowNodeSocket`. Non-node connections never have their channel pulled
    // from the NodeRegistry, so their `call()` never runs and this never fires.
    let rpc_close = Arc::new(Notify::new());
    let rpc_channel = crate::cluster::ReverseRpcChannel::with_close(rpc_out_tx, rpc_close.clone());
    let rpc_pending = rpc_channel.pending();
    // The channel reaches the outside world exactly one way: a node-shaped
    // connect (params carrying `commands`/`tags`) stores this clone inside its
    // `NodeSession`, and `node_invoke` / `node_file` / the approval path pull it
    // back out of the `NodeRegistry`. There is no second index.
    let rpc_channel_for_node = rpc_channel;
    // Disabled once the outbound channel closes so the select arm below stops
    // being polled (a closed mpsc receiver is always-ready and would spin).
    let mut rpc_open = true;

    // Initialize connection state
    {
        let mut conns = ctx.connections.write().await;
        conns.insert(conn_id.clone(), ConnectionState::new(ctx.client_ip));
    }

    // Transport keep-alive: periodic Ping + inbound idle watchdog.
    // The browser/`tokio-tungstenite` peer auto-Pongs, so any live socket
    // updates `last_activity_at` at least once per `ping_interval`. A dead
    // socket (closed peer that the OS hasn't detected yet, common when a
    // laptop sleeps with the lid closed) silently stops replying — after
    // `idle_timeout` we tear the connection down with WS code 1008 so the
    // panel/notification-bridge can reconnect promptly instead of waiting on
    // OS-level TCP keepalive (default ≥2h on macOS/Linux).
    let ping_period = Duration::from_secs(ctx.ping_interval_secs.max(1));
    let idle_timeout = Duration::from_secs(ctx.idle_timeout_secs.max(ctx.ping_interval_secs));
    let mut ping_timer = interval_at(TokioInstant::now() + ping_period, ping_period);
    ping_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_activity_at = Instant::now();
    // Closes the socket after too many login-wall rejections (stale-token
    // retry loops); see `flood_guard` module docs.
    let mut flood_guard = super::flood_guard::UnauthorizedFloodGuard::new(
        super::flood_guard::MAX_UNAUTHORIZED_STRIKES,
    );
    // The paired device this session authenticated as, latched at the `connect`
    // handshake (`ConnectAuthOutcome::{Authorized,BootstrapExchanged}.device_id`).
    // `None` for loopback, legacy shared-token and still-walled connections —
    // they are not bound to a device record. Read only by the per-device
    // revocation kick below; kept as a connection local rather than in the shared
    // `ConnectionState` because exactly one reader exists (R10 "zero consumers ⇒
    // no abstraction").
    let mut session_device_id: Option<String> = None;

    loop {
        tokio::select! {
            // Handle incoming messages
            msg = read.next() => {
                if matches!(msg, Some(Ok(_))) {
                    last_activity_at = Instant::now();
                }
                match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        let preview_end = text.char_indices().take_while(|(i, _)| *i < 200).last().map_or(text.len(), |(i, c)| i + c.len_utf8());
                        debug!("WS recv from {}: {}", conn_id, &text[..preview_end]);

                        // Reverse-RPC response interception: a frame that is a
                        // JSON-RPC *response* (has `id` + `result`/`error`, no
                        // `method`) is the reply to a server-initiated request.
                        // Route it to the pending table and stop — do NOT treat
                        // it as a client request (it would fail JsonRpcRequest
                        // parsing, which requires `method`).
                        if let Ok(maybe_resp) =
                            serde_json::from_str::<JsonRpcResponse>(&text)
                        {
                            let looks_like_response = maybe_resp.id.is_some()
                                && (maybe_resp.result.is_some() || maybe_resp.error.is_some());
                            if looks_like_response {
                                if let Some(id) = maybe_resp.id.clone() {
                                    rpc_pending.resolve(&id, maybe_resp);
                                }
                                continue;
                            }
                        }

                        // Node-initiated reverse request (cluster ③): a
                        // `node.approval.request` from a REGISTERED node
                        // connection is driven asynchronously and answered with a
                        // JSON-RPC response on this connection's outbound. Spawned
                        // so the select loop is not blocked for the (up to 120s)
                        // operator decision. Node identity is taken from the
                        // authenticated connection (anti-spoof), never params.
                        if let Ok(node_req) = serde_json::from_str::<JsonRpcRequest>(&text) {
                            if node_req.method == "node.approval.request" {
                                // LAN-trust: every connection is an implicit
                                // operator, so the node-approval path is always
                                // reachable. Node identity is taken from the
                                // connection (anti-spoof), never params.
                                match (
                                    ctx.node_registry.node_identity_by_conn(&conn_id),
                                    ctx.exec_approval_manager.clone(),
                                ) {
                                    (Some((node_id, node_name)), Some(manager)) => {
                                        let event_bus = ctx.event_bus.clone();
                                        let out = rpc_out_tx_replies.clone();
                                        let req_id = node_req.id.clone();
                                        let params = node_req
                                            .params
                                            .clone()
                                            .unwrap_or(serde_json::Value::Null);
                                        let tool = params
                                            .get("tool")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        let reason = params
                                            .get("reason")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        // Redacted action summary from the node.
                                        // Absent (older node) ⇒ falls back to the
                                        // tool name in `run_node_approval`.
                                        let action = params
                                            .get("action")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        tokio::spawn(async move {
                                            let (outcome, deny_reason) =
                                                crate::approval::run_node_approval(
                                                    &manager,
                                                    &event_bus,
                                                    &node_id,
                                                    &node_name,
                                                    &tool,
                                                    &action,
                                                    &reason,
                                                )
                                                .await;
                                            // Optional field: older nodes ignore it,
                                            // newer ones relay the operator's own
                                            // words to their model.
                                            let mut body =
                                                serde_json::json!({ "outcome": outcome });
                                            if let Some(r) = deny_reason {
                                                body["deny_reason"] =
                                                    serde_json::Value::String(r);
                                            }
                                            let resp = JsonRpcResponse::success(
                                                req_id, body,
                                            );
                                            if let Ok(s) = serde_json::to_string(&resp) {
                                                let _ = out.send(s).await;
                                            }
                                        });
                                    }
                                    _ => {
                                        // Not a registered node conn, or no
                                        // manager wired: refuse.
                                        let resp = JsonRpcResponse::error(
                                            node_req.id.clone(),
                                            -32000,
                                            "node.approval.request not permitted".to_string(),
                                        );
                                        if let Ok(s) = serde_json::to_string(&resp) {
                                            let _ = rpc_out_tx_replies.send(s).await;
                                        }
                                    }
                                }
                                continue;
                            }
                        }

                        // Parse request to check method for auth gating
                        let request: Result<JsonRpcRequest, _> = serde_json::from_str(&text);

                        let response = match request {
                            Ok(ref req) => {
                                // Session-init invariant: the first frame on a
                                // connection must be `connect`. LAN-trust drops
                                // all token machinery, but the handshake still
                                // bootstraps per-connection session state (presence,
                                // surface kind, operator permissions).
                                let is_first = {
                                    let conns = ctx.connections.read().await;
                                    conns.get(&conn_id).is_none_or(|s| s.first_message)
                                };
                                if is_first && req.method != "connect" {
                                    warn!(
                                        "Connection {} rejected: first request must be 'connect' (got '{}')",
                                        conn_id, req.method
                                    );
                                    let response = JsonRpcResponse::error(
                                        req.id.clone(),
                                        AUTH_REQUIRED,
                                        "First request must be 'connect'",
                                    );
                                    let response_str = serde_json::to_string(&response).unwrap_or_default();
                                    let _ = write.send(WsMessage::Text(response_str.into())).await;
                                    break;
                                }

                                {
                                    // Dispatch path. LAN-trust treats every
                                    // connection as an implicit operator.

                                    // --- Rate limit check ---
                                    // Loopback exemption is based on network origin
                                    // (the resolved client IP, so a reverse proxy
                                    // running on loopback never exempts the remote
                                    // clients behind it), not identity (device_id). For
                                    // authenticated connections the rl_identity is the
                                    // device_id which never looks like a loopback IP.
                                    if !ctx.client_ip.is_loopback() {
                                    let rl_identity = ctx.client_ip.to_string();
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
                                    } // end loopback exemption

                                    // Originating-connection role for the login
                                    // wall. Resolved at the `connect` handshake
                                    // (loopback ⇒ operator; remote ⇒ operator iff
                                    // a valid Gateway token was presented, else
                                    // guest) and stamped onto ConnectionState;
                                    // every later request reads it here. Absent
                                    // state (pre-handshake / probe) defaults by
                                    // network position — loopback operator,
                                    // remote guest (fail closed).
                                    let caller_role: Option<String> = {
                                        let conns = ctx.connections.read().await;
                                        conns.get(&conn_id).map(|s| s.caller_role.clone())
                                    }
                                    .or_else(|| {
                                        Some(
                                            if ctx.client_ip.is_loopback() {
                                                "operator"
                                            } else {
                                                "guest"
                                            }
                                            .to_string(),
                                        )
                                    });

                                    // Originating connection's authenticated user
                                    // (`users.user_id`), latched at `connect`
                                    // alongside `caller_role`. Pre-handshake /
                                    // probe paths default by network position —
                                    // loopback is the implicit owner, remote has
                                    // no user until authorized.
                                    let caller_user: Option<String> = {
                                        let conns = ctx.connections.read().await;
                                        conns.get(&conn_id).and_then(|s| s.caller_user.clone())
                                    }
                                    .or_else(|| {
                                        ctx.client_ip
                                            .is_loopback()
                                            .then(|| crate::gateway::security::store::OWNER_USER_ID.to_string())
                                    });

                                    // Login wall (Gateway-token model): an
                                    // unauthorized connection — a remote Panel
                                    // that has not presented a valid Gateway
                                    // token — may only (re)issue `connect` to
                                    // authorize. Every other method is refused
                                    // until a valid token is presented. Loopback
                                    // and token-authorized connections are
                                    // operator and pass freely; once authorized,
                                    // authority equals local (single tier).
                                    if caller_role.as_deref() != Some("operator")
                                        && req.method != "connect"
                                    {
                                        let resp = JsonRpcResponse::error(
                                            req.id.clone(),
                                            AUTH_REQUIRED,
                                            "Not authorized: present a valid Gateway token via \
                                             `connect` to access this core."
                                                .to_string(),
                                        );
                                        let resp_str =
                                            serde_json::to_string(&resp).unwrap_or_default();
                                        if let Err(e) =
                                            write.send(WsMessage::Text(resp_str.into())).await
                                        {
                                            error!(
                                                "Failed to send auth-required response to {}: {}",
                                                conn_id, e
                                            );
                                            break;
                                        }
                                        if flood_guard.record_rejection() {
                                            warn!(
                                                "Connection {} closed: {} requests without a \
                                                 valid Gateway token (flood guard)",
                                                conn_id,
                                                flood_guard.strikes()
                                            );
                                            // Forensic trail: one row per abusive
                                            // connection (bounded by the flood-guard
                                            // close, not per rejected frame).
                                            if let Some(log) = ctx.audit_log.as_ref() {
                                                log.log(crate::security::audit::AuditEntry::rate_limited(
                                                    ctx.client_ip.to_string(),
                                                    format!(
                                                        "connection closed: {} unauthorized requests (flood guard)",
                                                        flood_guard.strikes()
                                                    ),
                                                ));
                                            }
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
                                        // --- Idempotency + Lane concurrency control ---
                                        debug!("RPC dispatch: method={}", req.method);

                                        // Extract idempotency_key from params (optional)
                                        let idempotency_key = req.params
                                            .as_ref()
                                            .and_then(|p| p.get("idempotency_key"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from);

                                        let lane = crate::gateway::lane::Lane::for_method(&req.method);

                                        // Hard-require idempotency_key when the operator opted in
                                        // (require_idempotency_key=true). Read-only Query-lane RPCs
                                        // are exempt — they can never double-execute mutations.
                                        if ctx.require_idempotency_key
                                            && lane.needs_idempotency()
                                            && idempotency_key.is_none()
                                        {
                                            warn!(
                                                method = %req.method,
                                                lane = %lane,
                                                "Rejecting mutating RPC without idempotency_key (require_idempotency_key=true)"
                                            );
                                            let resp = JsonRpcResponse::error_with_data(
                                                req.id.clone(),
                                                IDEMPOTENCY_KEY_REQUIRED,
                                                "idempotency_key required for mutating RPCs",
                                                serde_json::json!({
                                                    "method": req.method,
                                                    "lane": lane.to_string(),
                                                    "hint": "include a stable per-attempt idempotency_key (UUID v4) in params",
                                                }),
                                            );
                                            let resp_str = serde_json::to_string(&resp).unwrap_or_default();
                                            if let Err(e) = write.send(WsMessage::Text(resp_str.into())).await {
                                                error!("Failed to send idempotency-required response to {}: {}", conn_id, e);
                                                break;
                                            }
                                            continue;
                                        }

                                        // Helper closure: standard lane dispatch (no idempotency)
                                        let do_lane_dispatch = |text: String, lm: Arc<LaneManager>, mc: MiddlewareChain, method: String, req_id: Option<serde_json::Value>, class: ChannelClass, caller_role: Option<String>, caller_user: Option<String>, caller_is_loopback: bool| async move {
                                            let lane_result = lm.acquire(&method, class).await;
                                            match lane_result {
                                                Ok(_permit) => crate::gateway::caller_identity::CALLER_USER
                                                    .scope(caller_user, crate::gateway::caller_identity::CALLER_ROLE
                                                        .scope(caller_role, crate::gateway::caller_identity::CALLER_IS_LOOPBACK
                                                            .scope(caller_is_loopback, process_request(&text, &mc))))
                                                    .await,
                                                Err(_) => serde_json::to_string(&JsonRpcResponse::error(
                                                    req_id,
                                                    INTERNAL_ERROR,
                                                    "Service congested, try again later",
                                                )).unwrap_or_default()
                                            }
                                        };

                                        // Check idempotency guard (only for non-Query lanes with a key)
                                        let mut response = if let Some(ref key) = idempotency_key {
                                            if lane.needs_idempotency() {
                                                use crate::gateway::idempotency::AcquireResult;
                                                match ctx.idempotency_guard.try_acquire(key) {
                                                    AcquireResult::Cached(cached) => {
                                                        debug!("Idempotency hit: key={}", key);
                                                        let resp = JsonRpcResponse::success(req.id.clone(), cached);
                                                        serde_json::to_string(&resp).unwrap_or_default()
                                                    }
                                                    AcquireResult::Waiting(mut rx) => {
                                                        debug!("Idempotency: awaiting in-flight key={}", key);
                                                        let result = tokio::time::timeout(
                                                            std::time::Duration::from_secs(30),
                                                            async {
                                                                let _ = rx.changed().await;
                                                                rx.borrow().clone()
                                                            }
                                                        ).await;
                                                        match result {
                                                            Ok(Some(val)) => {
                                                                let resp = JsonRpcResponse::success(req.id.clone(), val);
                                                                serde_json::to_string(&resp).unwrap_or_default()
                                                            }
                                                            _ => {
                                                                serde_json::to_string(&JsonRpcResponse::error(
                                                                    req.id.clone(),
                                                                    INTERNAL_ERROR,
                                                                    "Request timed out waiting for in-flight duplicate",
                                                                )).unwrap_or_default()
                                                            }
                                                        }
                                                    }
                                                    AcquireResult::Proceed(slot) => {
                                                        // First request — slot auto-discards on panic (RAII)
                                                        let lane_result = ctx.lane_manager.acquire(&req.method, ctx.channel_class).await;
                                                        match lane_result {
                                                            Ok(_permit) => {
                                                                let resp = crate::gateway::caller_identity::CALLER_USER
                                                                    .scope(caller_user.clone(), crate::gateway::caller_identity::CALLER_ROLE
                                                                        .scope(caller_role.clone(), crate::gateway::caller_identity::CALLER_IS_LOOPBACK
                                                                            .scope(ctx.client_ip.is_loopback(), process_request(&text, &ctx.middleware_chain))))
                                                                    .await;
                                                                if let Ok(parsed) = serde_json::from_str::<JsonRpcResponse>(&resp) {
                                                                    if parsed.is_success() {
                                                                        if let Some(result) = parsed.result {
                                                                            slot.complete(result);
                                                                        } else {
                                                                            slot.discard();
                                                                        }
                                                                    } else {
                                                                        slot.discard(); // Error — let next request retry
                                                                    }
                                                                } else {
                                                                    slot.discard();
                                                                }
                                                                resp
                                                            }
                                                            Err(_) => {
                                                                slot.discard();
                                                                serde_json::to_string(&JsonRpcResponse::error(
                                                                    req.id.clone(),
                                                                    INTERNAL_ERROR,
                                                                    "Service congested, try again later",
                                                                )).unwrap_or_default()
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Query lane — skip idempotency
                                                do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone(), caller_user.clone(), ctx.client_ip.is_loopback()).await
                                            }
                                        } else {
                                            // No idempotency key — standard lane dispatch
                                            do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone(), caller_user.clone(), ctx.client_ip.is_loopback()).await
                                        };
                                        // --- End idempotency + lane block ---

                                        // Establish session state from a successful connect
                                        // handshake. LAN-trust: no auth, but the handshake
                                        // still records surface kind, clears first_message,
                                        // and tracks presence.
                                        if req.method == "connect" {
                                            if let Ok(mut resp) = serde_json::from_str::<JsonRpcResponse>(&response) {
                                                if resp.is_success() {
                                                    // Gateway-token authorization. Loopback ⇒
                                                    // operator (zero-config, no token). Remote ⇒
                                                    // validate device token, bootstrap ticket, or the
                                                    // legacy shared Gateway token presented in
                                                    // `connect` params. The decision lives in
                                                    // `connect::resolve_connect_auth`.
                                                    let params = req.params.as_ref();
                                                    let presented_token = params
                                                        .and_then(|p| p.get("token"))
                                                        .and_then(|v| v.as_str());
                                                    let device_token = params
                                                        .and_then(|p| p.get("device_token"))
                                                        .and_then(|v| v.as_str());
                                                    let bootstrap_ticket = params
                                                        .and_then(|p| p.get("bootstrap_ticket"))
                                                        .and_then(|v| v.as_str());
                                                    let device_id = params
                                                        .and_then(|p| p.get("device_id"))
                                                        .and_then(|v| v.as_str());
                                                    let device_name = params
                                                        .and_then(|p| p.get("device_name"))
                                                        .and_then(|v| v.as_str());

                                                    let auth_outcome = if let Some(mgr) = ctx.device_token_mgr.as_ref() {
                                                        crate::gateway::handlers::connect::resolve_connect_auth(
                                                            ctx.client_ip.is_loopback(),
                                                            presented_token,
                                                            device_token,
                                                            bootstrap_ticket,
                                                            device_id,
                                                            device_name,
                                                            |t| {
                                                                crate::gateway::security::SharedTokenManager::global()
                                                                    .map(|m| m.validate(t).unwrap_or(false))
                                                                    .unwrap_or(false)
                                                            },
                                                            mgr,
                                                        )
                                                    } else {
                                                        // Device-token manager not wired: fall back to
                                                        // the legacy shared-token-only behavior.
                                                        let authorized = crate::gateway::handlers::connect::connect_authorized(
                                                            ctx.client_ip.is_loopback(),
                                                            presented_token,
                                                            |t| {
                                                                crate::gateway::security::SharedTokenManager::global()
                                                                    .map(|m| m.validate(t).unwrap_or(false))
                                                                    .unwrap_or(false)
                                                            },
                                                        );
                                                        if authorized {
                                                            crate::gateway::handlers::connect::ConnectAuthOutcome::Authorized { device_id: None }
                                                        } else {
                                                            crate::gateway::handlers::connect::ConnectAuthOutcome::Unauthorized
                                                        }
                                                    };

                                                    let (authorized, panel_role, issued_device_token, authed_device_id) = match &auth_outcome {
                                                        crate::gateway::handlers::connect::ConnectAuthOutcome::Authorized { device_id } => (true, "operator", None, device_id.clone()),
                                                        crate::gateway::handlers::connect::ConnectAuthOutcome::BootstrapExchanged { device_token, device_id } => (true, "operator", Some(device_token.clone()), Some(device_id.clone())),
                                                        crate::gateway::handlers::connect::ConnectAuthOutcome::Unauthorized => (false, "guest", None, None),
                                                    };
                                                    // Bind this session to the paired device it authenticated
                                                    // as, so `gateway.devices.revoke` can close exactly this
                                                    // socket (and no other) the moment the revocation lands.
                                                    session_device_id.clone_from(&authed_device_id);
                                                    // A device-token reconnect (or fresh pairing) refreshes the
                                                    // paired device's `last_seen_at`, so the Paired-devices roster
                                                    // reflects real activity, not the pairing date. Token
                                                    // validation alone only touches the token row's `last_used_at`.
                                                    if let (Some(store), Some(did)) =
                                                        (ctx.security_store.as_ref(), authed_device_id.as_deref())
                                                    {
                                                        if let Err(e) = store.touch_device(did) {
                                                            tracing::debug!("touch_device on connect failed: {e}");
                                                        }
                                                    }
                                                    // Forensic trail: a remote connection that
                                                    // failed the Gateway-token login wall. Bounded
                                                    // to <=10/60s/IP by the `Auth`-scope limiter,
                                                    // so a brute-force campaign self-throttles
                                                    // after ~10 recorded attempts. Loopback is the
                                                    // zero-config operator and never audited.
                                                    if crate::gateway::handlers::connect::should_audit_connect_failure(
                                                        authorized,
                                                        ctx.client_ip.is_loopback(),
                                                    ) {
                                                        if let Some(log) = ctx.audit_log.as_ref() {
                                                            log.log(crate::security::audit::AuditEntry::auth_failure(
                                                                ctx.client_ip.to_string(),
                                                                "remote connect rejected: no valid Gateway credential",
                                                            ));
                                                        }
                                                    }
                                                    {
                                                        let mut conns = ctx.connections.write().await;
                                                        if let Some(state) = conns.get_mut(&conn_id) {
                                                            // Record the surface identity: client-declared
                                                            // kind, else inferred from loopback (same-machine
                                                            // attach ⇒ desktop-class).
                                                            let declared = req
                                                                .params
                                                                .as_ref()
                                                                .and_then(|p| p.get("channel_kind"))
                                                                .and_then(|v| v.as_str());
                                                            let kind = match crate::gateway::surface::SurfaceKind::from_opt_str(declared) {
                                                                crate::gateway::surface::SurfaceKind::Unknown if ctx.client_ip.is_loopback() => {
                                                                    crate::gateway::surface::SurfaceKind::Desktop
                                                                }
                                                                other => other,
                                                            };
                                                            state.channel_kind = Some(kind);
                                                            state.first_message = false;
                                                            // Authorized ⇒ wildcard scope so
                                                            // EventScopeGuard delivers guarded topics
                                                            // (approval banners, config.changed).
                                                            // Unauthorized ⇒ no scopes (walled).
                                                            state.permissions = if authorized {
                                                                vec!["*".to_string()]
                                                            } else {
                                                                Vec::new()
                                                            };
                                                            // Role + user for the login-wall gate and the
                                                            // config-tier tool gate. See
                                                            // `resolve_stamped_identity` (pure, unit-tested)
                                                            // for the decision rules.
                                                            let (resolved_user, resolved_role) = resolve_stamped_identity(
                                                                authorized,
                                                                ctx.client_ip.is_loopback(),
                                                                authed_device_id.as_deref(),
                                                                ctx.security_store.as_deref(),
                                                            );
                                                            state.caller_role = resolved_role.to_string();
                                                            state.caller_user = resolved_user;
                                                            // Device binding for the per-device revoke.
                                                            // Same value as the connection-local latch
                                                            // above, written under this one lock: the
                                                            // local serves the per-event hot path (no
                                                            // lock per event), this copy serves
                                                            // `invalidate_device_sessions`, which has
                                                            // only the shared map to look in.
                                                            state.device_id.clone_from(&authed_device_id);
                                                        }
                                                    }
                                                    // Cluster: a node both *enrolls* and *registers*
                                                    // inside this one `connect`. Enrollment cannot be
                                                    // its own RPC — `connect` is the only frame that
                                                    // clears the first-message rule AND precedes the
                                                    // login wall, so a remote (LAN) node has no other
                                                    // way to obtain an id. `admit_node` mints on first
                                                    // boot, adopts the operator's pre-enrolled row by
                                                    // name, and REFUSES a node whose device record was
                                                    // revoked by `cluster.deregister` (otherwise the
                                                    // node just resurrects itself on its next backoff).
                                                    let node_result = node_connect_claim(
                                                        req.params.as_ref(),
                                                    )
                                                    .map(|claim| {
                                                        let admission = ctx
                                                            .security_store
                                                            .as_ref()
                                                            .map_or_else(
                                                                || {
                                                                    // No store wired (probe/test server):
                                                                    // LAN-trust degrade — keep the node usable.
                                                                    crate::cluster::NodeAdmission::Admitted {
                                                                        node_id: claim
                                                                            .presented_id
                                                                            .clone()
                                                                            .unwrap_or_else(|| conn_id.clone()),
                                                                        minted: false,
                                                                    }
                                                                },
                                                                |store| {
                                                                    crate::cluster::admit_node(
                                                                        store,
                                                                        claim.presented_id.as_deref(),
                                                                        &claim.device_name,
                                                                    )
                                                                },
                                                            );
                                                        (claim, admission)
                                                    });

                                                    let node_payload = match &node_result {
                                                        Some((
                                                            claim,
                                                            crate::cluster::NodeAdmission::Admitted {
                                                                node_id,
                                                                minted,
                                                            },
                                                        )) => {
                                                            if crate::cluster::maybe_register_node(
                                                                &ctx.node_registry,
                                                                Some("node"),
                                                                node_id,
                                                                &conn_id,
                                                                req.params.as_ref(),
                                                                &rpc_channel_for_node,
                                                            ) {
                                                                let _ = ctx.event_bus.publish_json(&TopicEvent::new(
                                                                    "node.connected",
                                                                    serde_json::json!({"node_id": node_id, "name": &claim.device_name, "conn_id": &conn_id}),
                                                                ));
                                                                // Stamp last_seen so the offline half of
                                                                // environments.list stays honest.
                                                                if let Some(store) = ctx.security_store.as_ref() {
                                                                    if let Err(e) = store.touch_device(node_id) {
                                                                        debug!("failed to stamp node last_seen on connect for {}: {}", node_id, e);
                                                                    }
                                                                }
                                                            }
                                                            Some(serde_json::json!({
                                                                "node_id": node_id,
                                                                "status": "registered",
                                                                // The node persists its id only when we
                                                                // say it is new; a reconnect is a no-op.
                                                                "persist": minted,
                                                            }))
                                                        }
                                                        Some((
                                                            _,
                                                            crate::cluster::NodeAdmission::Deregistered {
                                                                node_id,
                                                            },
                                                        )) => {
                                                            warn!(
                                                                "Refusing node {}: device record was revoked by cluster.deregister",
                                                                node_id
                                                            );
                                                            Some(serde_json::json!({
                                                                "node_id": node_id,
                                                                "status": "deregistered",
                                                            }))
                                                        }
                                                        Some((
                                                            _,
                                                            crate::cluster::NodeAdmission::IdentityConflict {
                                                                node_id,
                                                            },
                                                        )) => {
                                                            warn!(
                                                                "Refusing node {}: that id already belongs to a paired Panel device",
                                                                node_id
                                                            );
                                                            // Same terminal wire status as a deregistration:
                                                            // the node client treats it as "stop retrying,
                                                            // an operator must intervene", which is exactly
                                                            // right for an id collision. Keeping one status
                                                            // avoids rippling a new verdict into the node
                                                            // client for a case only an operator can fix.
                                                            Some(serde_json::json!({
                                                                "node_id": node_id,
                                                                "status": "deregistered",
                                                            }))
                                                        }
                                                        None => None,
                                                    };

                                                    // Echo the verdict so the Panel renders the login
                                                    // wall / token box when unauthorized, and unlocks
                                                    // the full app (same as local) when authorized.
                                                    if let Some(obj) = resp
                                                        .result
                                                        .as_mut()
                                                        .and_then(serde_json::Value::as_object_mut)
                                                    {
                                                        obj.insert(
                                                            "role".to_string(),
                                                            serde_json::Value::String(
                                                                panel_role.to_string(),
                                                            ),
                                                        );
                                                        obj.insert(
                                                            "authorized".to_string(),
                                                            serde_json::Value::Bool(authorized),
                                                        );
                                                        obj.insert(
                                                            "needs_token".to_string(),
                                                            serde_json::Value::Bool(!authorized),
                                                        );
                                                        if let Some(dt) = issued_device_token {
                                                            obj.insert(
                                                                "device_token".to_string(),
                                                                serde_json::Value::String(dt),
                                                            );
                                                        }
                                                        if let Some(node) = node_payload {
                                                            obj.insert("node".to_string(), node);
                                                        }
                                                    }
                                                    response =
                                                        serde_json::to_string(&resp).unwrap_or(response);

                                                    // Track presence for this connect. A device-token or
                                                    // bootstrap session carries the paired device_id for honest
                                                    // roster attribution; loopback and legacy shared-token stay
                                                    // None (not bound to a specific paired device).
                                                    {
                                                        let conns = ctx.connections.read().await;
                                                        if let Some(state) = conns.get(&conn_id) {
                                                            let presence_entry = PresenceEntry {
                                                                conn_id: conn_id.clone(),
                                                                device_id: authed_device_id.clone(),
                                                                device_name: state.metadata.get("client_name").cloned().unwrap_or_else(|| "Unknown".to_string()),
                                                                platform: state.metadata.get("platform").cloned().unwrap_or_else(|| "unknown".to_string()),
                                                                role: crate::gateway::presence::ConnectionRole::User,
                                                                connected_at: chrono::Utc::now(),
                                                                last_heartbeat: chrono::Utc::now(),
                                                            };
                                                            drop(conns);
                                                            ctx.presence.upsert(conn_id.clone(), presence_entry);
                                                            ctx.state_versions.bump_presence();
                                                            let _ = ctx.event_bus.publish_json(&TopicEvent::new("presence.joined", serde_json::json!({"conn_id": &conn_id})).with_state_version(ctx.state_versions.snapshot()));
                                                        }
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
                                    format!("Parse error: {e}"),
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
            event = client_event_rx.recv() => {
                match event {
                    Ok(event_json) => {
                        // Token rotation kick: close remote (token-authorized)
                        // sessions so they re-authenticate; never forward this
                        // frame to clients verbatim, and never close loopback.
                        if rotated_should_close_remote(&event_json, ctx.client_ip.is_loopback()) {
                            info!("token rotated — closing remote session {}", conn_id);
                            let _ = write
                                .send(WsMessage::Close(Some(CloseFrame {
                                    code: 4001,
                                    reason: "token_rotated".into(),
                                })))
                                .await;
                            break;
                        }
                        // Loopback receives token_rotated: swallow silently, do not forward.
                        if is_token_rotated_frame(&event_json) {
                            continue;
                        }
                        // Per-device revocation kick: close only the sessions bound
                        // to the revoked device. `gateway.devices.revoke` already
                        // downgraded this connection to the login wall synchronously
                        // (`invalidate_device_sessions`), so anything pipelined ahead
                        // of this frame is refused rather than served; this closes the
                        // socket so the client stops holding a dead session open.
                        if device_revoked_should_close(&event_json, session_device_id.as_deref()) {
                            info!("device revoked — closing session {}", conn_id);
                            let _ = write
                                .send(WsMessage::Close(Some(CloseFrame {
                                    code: 4001,
                                    reason: "device_revoked".into(),
                                })))
                                .await;
                            break;
                        }
                        // Everyone else: the revocation names another device (or none
                        // of ours). Never forwarded — it carries a device id and no
                        // client renders it.
                        if device_revoked_id(&event_json).is_some() {
                            continue;
                        }
                        // Try to extract topic from event for filtering
                        let should_forward = if let Ok(event_obj) = serde_json::from_str::<serde_json::Value>(&event_json) {
                            // Check for topic in event (TopicEvent format)
                            let topic = event_obj.get("topic")
                                .and_then(|t| t.as_str())
                                // Or method for JSON-RPC notification format
                                .or_else(|| event_obj.get("method").and_then(|m| m.as_str()))
                                .unwrap_or("");

                            // Permission-based scope guard check + surface audience.
                            // Both read the same ConnectionState under one lock.
                            let (scope_allowed, channel_kind) = {
                                let conns = ctx.connections.read().await;
                                match conns.get(&conn_id) {
                                    Some(s) => (
                                        ctx.event_scope_guard.can_receive(topic, &s.permissions),
                                        s.channel_kind,
                                    ),
                                    None => (false, None),
                                }
                            };

                            // Extract payload data for field-level filter predicates.
                            // TopicEvent shape stores it at .data; JSON-RPC notifications
                            // store it at .params (then nested .data for our wrapper).
                            let event_data = event_obj
                                .get("data")
                                .or_else(|| event_obj.get("params").and_then(|p| p.get("data")));

                            scope_allowed
                                && crate::gateway::surface::delivery::audience_allows(
                                    event_data,
                                    channel_kind,
                                )
                                && ctx.subscription_manager.should_receive(&conn_id, topic, event_data).await
                        } else {
                            // Can't parse event, forward by default
                            true
                        };

                        if should_forward {
                            debug!("Forwarding event to {}", conn_id);
                            // Wrap TopicEvent into JSON-RPC notification format
                            // so the panel can dispatch it via method == "event"
                            let wire_json = if let Ok(event_obj) = serde_json::from_str::<serde_json::Value>(&event_json) {
                                if event_obj.get("topic").is_some() && event_obj.get("method").is_none() {
                                    // TopicEvent format -> wrap as JSON-RPC notification
                                    serde_json::json!({
                                        "method": "event",
                                        "params": event_obj,
                                    }).to_string()
                                } else {
                                    event_json
                                }
                            } else {
                                event_json
                            };
                            if let Err(e) = write.send(WsMessage::Text(wire_json.into())).await {
                                error!("Failed to send event to {}: {}", conn_id, e);
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        buffer_metrics.add_overflow(n);
                        warn!(
                            "Event forwarder lagged for {}, dropped {} events, total overflow={}",
                            conn_id,
                            n,
                            buffer_metrics.overflow()
                        );
                        // Tell the client why before tearing the socket down.
                        // The panel/notification-bridge can surface the warning
                        // and reconnect, instead of seeing a random drop.
                        // Best-effort: the connection is already in trouble.
                        let diag = overflow_warning_frame(n, buffer_metrics.overflow());
                        let _ = write.send(WsMessage::Text(diag.into())).await;
                        let _ = write
                            .send(WsMessage::Close(Some(CloseFrame {
                                code: 1008,
                                reason: "slow consumer".into(),
                            })))
                            .await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Event forwarder closed for {}", conn_id);
                        break;
                    }
                }
            }
            // Reverse-RPC outbound: write server-initiated frames verbatim.
            // Full JSON-RPC request strings produced by ReverseRpcChannel::call();
            // no filtering, no wrapping.
            frame = rpc_out_rx.recv(), if rpc_open => {
                match frame {
                    Some(text) => {
                        if let Err(e) = write.send(WsMessage::Text(text.into())).await {
                            error!("Failed to send reverse-rpc frame to {}: {}", conn_id, e);
                            break;
                        }
                    }
                    None => {
                        // All senders dropped; disable this arm so select stops
                        // polling an always-ready closed receiver (no spin).
                        rpc_open = false;
                    }
                }
            }
            // Slow-consumer teardown: a reverse-RPC call detected the outbound
            // queue wedged (peer stopped draining = half-open / slow consumer) and
            // fired this. Break to the shared cleanup below, which deregisters the
            // node, emits `node.disconnected`, cancels in-flight calls and drops
            // the socket — so the node reconnects instead of holding a registry
            // slot the inbound idle-watchdog can't reclaim for a write-only wedge.
            _ = rpc_close.notified() => {
                warn!(
                    "Reverse-RPC outbound wedged for {}; closing connection (slow consumer)",
                    conn_id
                );
                break;
            }
            // Server-initiated WS Ping + inbound idle watchdog
            _ = ping_timer.tick() => {
                // Global-hop overflow watchdog. The bus->buffer forwarder drops
                // events on transient lag and records them on the shared metric
                // (without access to this socket). If any overflow has accrued,
                // apply the same slow-consumer policy as the per-client drain arm:
                // warn the client to reconnect, then close 1008 so it re-syncs from
                // the hello snapshot. Bounded by the ping interval; the client just
                // misses some events until then, which the resync recovers.
                let overflow_now = buffer_metrics.overflow();
                if overflow_now > 0 {
                    let dropped = overflow_now;
                    warn!(
                        "Event bus overflow for {} ({} dropped, total {}); closing for reconnect",
                        conn_id, dropped, overflow_now
                    );
                    let diag = overflow_warning_frame(dropped, overflow_now);
                    let _ = write.send(WsMessage::Text(diag.into())).await;
                    let _ = write
                        .send(WsMessage::Close(Some(CloseFrame {
                            code: 1008,
                            reason: "slow consumer".into(),
                        })))
                        .await;
                    break;
                }

                let idle_for = last_activity_at.elapsed();
                if idle_for > idle_timeout {
                    warn!(
                        "Idle timeout for {} (no inbound for {}s, threshold {}s); closing",
                        conn_id,
                        idle_for.as_secs(),
                        idle_timeout.as_secs(),
                    );
                    let _ = write
                        .send(WsMessage::Close(Some(CloseFrame {
                            code: 1008,
                            reason: "idle timeout".into(),
                        })))
                        .await;
                    break;
                }
                if let Err(e) = write.send(WsMessage::Ping(Default::default())).await {
                    debug!("Ping send failed for {} ({}); closing", conn_id, e);
                    break;
                }
            }
        }
    }

    // Cleanup
    {
        let mut conns = ctx.connections.write().await;
        conns.remove(&conn_id);
    }

    // Fail-fast every in-flight reverse-RPC call bound to this socket. Without
    // this, callers (node_invoke / node_file / node approval) keep their own
    // ReverseRpcChannel clone alive, so the oneshot senders never drop and the
    // call blocks until its per-call timeout (≤130s) even though the node is
    // already gone. Mirrors openclaw `node-registry.unregister()` rejecting
    // pending invokes on disconnect.
    let cancelled = rpc_pending.cancel_all();
    if cancelled > 0 {
        debug!(
            "Connection {} closed: cancelled {} in-flight reverse-RPC call(s)",
            conn_id, cancelled
        );
    }
    // Cluster Phase 0b: drop this connection's node session if it was a node, and
    // emit a `node.disconnected` lifecycle event so operators/Panel get a live
    // fleet feed instead of polling `environments.list`. Capture identity BEFORE
    // deregister (which removes it); the `if deregister` guard skips emission for
    // a stale old connection whose node_id was already reclaimed by a reconnect.
    let node_ident = ctx.node_registry.node_identity_by_conn(&conn_id);
    if ctx.node_registry.deregister(&conn_id) {
        if let Some((node_id, name)) = node_ident {
            // Refresh last_seen_at at the moment the node drops, so the offline
            // entry in environments.list reads "last seen ≈ disconnect time"
            // rather than "≈ connect time" for long-lived sessions.
            if let Some(store) = ctx.security_store.as_ref() {
                if let Err(e) = store.touch_device(&node_id) {
                    debug!(
                        "failed to stamp node last_seen on disconnect for {}: {}",
                        node_id, e
                    );
                }
            }
            let _ = ctx.event_bus.publish_json(&TopicEvent::new(
                "node.disconnected",
                serde_json::json!({"node_id": node_id, "name": name, "conn_id": &conn_id}),
            ));
        }
    }

    // Remove presence and emit departure event
    if let Some(_entry) = ctx.presence.remove(&conn_id) {
        ctx.state_versions.bump_presence();
        let _ = ctx.event_bus.publish_json(
            &TopicEvent::new("presence.left", serde_json::json!({"conn_id": &conn_id}))
                .with_state_version(ctx.state_versions.snapshot()),
        );
    }

    // Remove subscriptions for this connection
    ctx.subscription_manager.remove_connection(&conn_id).await;

    info!("Connection closed: {}", conn_id);
    Ok(())
}

/// Process a JSON-RPC request string
pub(super) async fn process_request(text: &str, middleware_chain: &MiddlewareChain) -> String {
    // Parse the request
    let request: JsonRpcRequest = match serde_json::from_str(text) {
        Ok(req) => req,
        Err(e) => {
            return serde_json::to_string(&JsonRpcResponse::error(
                None,
                PARSE_ERROR,
                format!("Parse error: {e}"),
            ))
            .unwrap_or_default();
        }
    };

    // Distributed-trace context: honour an inbound W3C `traceparent` (carried
    // in params) or mint a fresh root trace, so every log/span emitted while
    // handling this request is correlatable. See `trace_context` for why this
    // is a lightweight propagation layer rather than a full OTel integration.
    let trace =
        crate::gateway::trace_context::TraceContext::from_request_params(request.params.as_ref());
    let span = tracing::info_span!(
        "rpc",
        trace_id = %trace.trace_id,
        span_id = %trace.span_id,
        method = %request.method,
    );

    // Dispatch to middleware chain inside the trace span.
    let response = {
        use tracing::Instrument;
        middleware_chain.serve(request).instrument(span).await
    };

    // Echo the trace context back so the caller / a downstream hop can continue
    // the trace (naming our span as the parent). `traceparent` is a non-standard
    // sibling of the JSON-RPC envelope fields; serde clients ignore unknown
    // fields, so this stays backward-compatible.
    let mut value = serde_json::to_value(&response).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "traceparent".to_string(),
            serde_json::Value::String(trace.to_header()),
        );
    }
    serde_json::to_string(&value).unwrap_or_default()
}

/// What a cluster node claims about itself in its `connect` frame.
pub(crate) struct NodeConnectClaim {
    /// The node's persisted `node_id`, or `None` on its very first boot (it has
    /// nothing to present yet — `cluster::admit_node` hands one back).
    pub presented_id: Option<String>,
    pub device_name: String,
}

/// LAN-trust cluster-node detection at `connect` time.
///
/// Token roles are gone, so a cluster node announces itself by request shape:
/// the node client (`aleph-server node`) always sends `commands` + `tags` in its
/// connect params, which no other client does. Returns `None` for every non-node
/// connect. Unlike the old `node_connect_identity`, this does NOT invent an id
/// from `device_name`/`conn_id` — identity is resolved against the device store
/// by [`crate::cluster::admit_node`], so a node's id is stable across reconnects
/// and a revoked node can be told apart from a brand-new one.
fn node_connect_claim(params: Option<&serde_json::Value>) -> Option<NodeConnectClaim> {
    let p = params?;
    if p.get("commands").is_none() && p.get("tags").is_none() {
        return None;
    }
    Some(NodeConnectClaim {
        presented_id: p
            .get("device_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        device_name: p
            .get("device_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
    })
}

#[cfg(test)]
mod token_rotation_tests {
    use super::{is_token_rotated_frame, rotated_should_close_remote, TOKEN_ROTATED_TOPIC};
    use crate::gateway::events::GatewayEventFrame;

    /// The exact wire string `GatewayEvents::publish_frame` emits for the
    /// rotation event: the TopicEvent wrapper `{"topic": ..., "data": <frame>}`.
    /// Built from the real `topic_name()` + serde serialization so the test
    /// catches drift in either the topic name or the frame wrapping — the
    /// original tests fed the bare inner frame and so masked the wire-format bug.
    fn rotated_wire_frame() -> String {
        let frame = GatewayEventFrame::TokenRotated;
        serde_json::json!({
            "topic": frame.topic_name(),
            "data": serde_json::to_value(&frame).unwrap(),
        })
        .to_string()
    }

    #[test]
    fn topic_constant_matches_frame_topic_name() {
        // Drift guard: the interceptor's literal must equal the frame's topic,
        // or the kick silently breaks again the next time the topic is renamed.
        assert_eq!(
            GatewayEventFrame::TokenRotated.topic_name(),
            TOKEN_ROTATED_TOPIC
        );
    }

    #[test]
    fn detects_real_publish_frame_wire_form() {
        // Regression for the wire-format bug: the wrapped TopicEvent form the
        // forward loop actually receives must be recognized.
        assert!(is_token_rotated_frame(&rotated_wire_frame()));
    }

    #[test]
    fn remote_session_closes_on_token_rotated() {
        assert!(rotated_should_close_remote(&rotated_wire_frame(), false));
    }

    #[test]
    fn loopback_session_ignores_token_rotated() {
        assert!(!rotated_should_close_remote(&rotated_wire_frame(), true));
    }

    #[test]
    fn other_events_never_trigger_close() {
        assert!(!rotated_should_close_remote(
            r#"{"topic":"acp.sessions.changed"}"#,
            false
        ));
        assert!(!rotated_should_close_remote(
            r#"{"topic":"alerts.system"}"#,
            false
        ));
        // The bare inner serde-tagged frame is NOT the wire form and must not
        // match — only the wrapped TopicEvent form reaches the interceptor.
        assert!(!rotated_should_close_remote(
            r#"{"type":"token_rotated"}"#,
            false
        ));
    }
}

#[cfg(test)]
mod device_revocation_tests {
    use super::{device_revoked_id, device_revoked_should_close, DEVICE_REVOKED_TOPIC};
    use crate::gateway::events::GatewayEventFrame;

    /// The exact wire string `publish_frame` emits — the wrapped TopicEvent form
    /// `{"topic": …, "data": <frame>}`, built from the real `topic_name()` and
    /// serde output. Same discipline as the rotation tests: feeding the bare
    /// inner frame is what once let a dud predicate stay green.
    fn revoked_wire_frame(device_id: &str) -> String {
        let frame = GatewayEventFrame::DeviceRevoked {
            device_id: device_id.to_string(),
        };
        serde_json::json!({
            "topic": frame.topic_name(),
            "data": serde_json::to_value(&frame).unwrap(),
        })
        .to_string()
    }

    #[test]
    fn topic_constant_matches_frame_topic_name() {
        assert_eq!(
            GatewayEventFrame::DeviceRevoked {
                device_id: "x".into()
            }
            .topic_name(),
            DEVICE_REVOKED_TOPIC
        );
    }

    #[test]
    fn reads_device_id_from_the_real_publish_frame_wire_form() {
        assert_eq!(
            device_revoked_id(&revoked_wire_frame("device-7")).as_deref(),
            Some("device-7")
        );
        // Bare inner frame is not the wire form.
        assert!(device_revoked_id(r#"{"type":"device_revoked","device_id":"device-7"}"#).is_none());
    }

    #[test]
    fn closes_only_the_named_device() {
        let frame = revoked_wire_frame("device-7");
        assert!(device_revoked_should_close(&frame, Some("device-7")));
        // A different paired device keeps its session.
        assert!(!device_revoked_should_close(&frame, Some("device-8")));
    }

    #[test]
    fn never_closes_an_unbound_session() {
        // Loopback, legacy shared-token, and still-walled connections carry no
        // device_id. A per-device revoke must never collaterally kick the
        // operator's own local App — that is `gateway.token.rotate`'s job.
        assert!(!device_revoked_should_close(
            &revoked_wire_frame("device-7"),
            None
        ));
    }

    #[test]
    fn other_events_never_trigger_close() {
        assert!(!device_revoked_should_close(
            r#"{"topic":"gateway.token.rotated","data":{}}"#,
            Some("device-7")
        ));
        assert!(!device_revoked_should_close(
            r#"{"topic":"acp.sessions.changed"}"#,
            Some("device-7")
        ));
        assert!(!device_revoked_should_close("not json", Some("device-7")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Connect-time identity stamping (resolve_stamped_identity) ─────────
    // These pin the branch logic extracted verbatim from the connect
    // handshake's ConnectionState-stamping site (server::handler still calls
    // this exact function). The surrounding WS glue — lock acquisition, the
    // `state.caller_role = ..` / `state.caller_user = ..` assignment, and
    // `handle_connection`'s dispatch loop itself — has no injectable seam
    // (it operates on a live `axum::extract::ws::WebSocket`) and is not
    // covered here; see the Task 2 fix report for what that would require.

    fn store_with_device_user(
        device_id: &str,
        user_id: &str,
        role: crate::gateway::security::store::UserRole,
    ) -> crate::gateway::security::store::SecurityStore {
        use crate::gateway::security::store::{DeviceUpsertData, SecurityStore};
        let store = SecurityStore::in_memory().unwrap();
        store.create_user(user_id, "Test User", role).unwrap();
        store
            .upsert_device(&DeviceUpsertData {
                device_id,
                device_name: "Test Device",
                device_type: Some("panel"),
                public_key: &[1u8; 32],
                fingerprint: device_id,
                role: "operator",
                scopes: &[],
            })
            .unwrap();
        store.set_device_user(device_id, user_id).unwrap();
        store
    }

    #[test]
    fn unauthorized_stays_walled_even_with_a_store_present() {
        // A store being available never overrides an unauthorized verdict.
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (user, role) = resolve_stamped_identity(false, false, Some("dev-r"), Some(&store));
        assert_eq!(user, None);
        assert_eq!(role, "guest");
    }

    #[test]
    fn no_store_wired_falls_back_to_owner_when_authorized() {
        // probe/test server with no security store: LAN-trust degrade.
        let (user, role) = resolve_stamped_identity(true, false, None, None);
        assert_eq!(
            user.as_deref(),
            Some(crate::gateway::security::store::OWNER_USER_ID)
        );
        assert_eq!(role, "operator");
    }

    #[test]
    fn admin_user_device_stamps_operator_and_user_id() {
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(user.as_deref(), Some("u-root"));
        assert_eq!(role, "operator");
    }

    #[test]
    fn member_user_device_stamps_member_and_user_id() {
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        assert_eq!(user.as_deref(), Some("u-alice"));
        assert_eq!(role, "member");
    }

    #[test]
    fn deactivated_user_device_is_walled_at_connect_time() {
        // The key behavior this task pins: a device bound to a deactivated
        // user is walled at connect time even though its token was valid
        // (authorized == true).
        use crate::gateway::security::store::UserStatus;
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        store
            .update_user("u-root", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(user, None);
        assert_eq!(role, "guest");
    }

    #[test]
    fn loopback_stamps_owner_operator_regardless_of_device() {
        let (user, role) = resolve_stamped_identity(true, true, None, None);
        assert_eq!(
            user.as_deref(),
            Some(crate::gateway::security::store::OWNER_USER_ID)
        );
        assert_eq!(role, "operator");
    }

    // ── LAN-trust node-shape detection + cluster registration ────────────

    #[test]
    fn node_connect_claim_detects_commands_or_tags_shape() {
        // The node client always sends both `commands` and `tags`.
        let full = serde_json::json!({
            "device_name": "build-box",
            "commands": [],
            "tags": ["linux"]
        });
        let claim = node_connect_claim(Some(&full)).expect("node shape");
        assert_eq!(claim.device_name, "build-box");
        assert!(
            claim.presented_id.is_none(),
            "first boot presents no id — admit_node mints one"
        );
        // Either key alone is enough.
        let tags_only = serde_json::json!({"device_name": "t", "tags": []});
        assert!(node_connect_claim(Some(&tags_only)).is_some());
        let commands_only = serde_json::json!({"device_name": "c2", "commands": []});
        assert!(node_connect_claim(Some(&commands_only)).is_some());
        // A reconnecting node presents its persisted id.
        let with_id = serde_json::json!({
            "device_id": "node-7", "device_name": "x", "commands": [], "tags": []
        });
        assert_eq!(
            node_connect_claim(Some(&with_id)).unwrap().presented_id,
            Some("node-7".to_string())
        );
        // An empty device_id is treated as absent (first boot), not as an id.
        let empty_id = serde_json::json!({
            "device_id": "", "device_name": "x", "commands": [], "tags": []
        });
        assert!(node_connect_claim(Some(&empty_id))
            .unwrap()
            .presented_id
            .is_none());
    }

    #[test]
    fn ordinary_connect_params_are_not_node_shaped() {
        // Panel / CLI connects (no commands/tags) must not register as nodes.
        let panel = serde_json::json!({
            "device_name": "Web Panel",
            "channel_kind": "browser",
            "token": "legacy:sig"
        });
        assert!(node_connect_claim(Some(&panel)).is_none());
        let bare = serde_json::json!({});
        assert!(node_connect_claim(Some(&bare)).is_none());
        assert!(node_connect_claim(None).is_none());
    }

    #[test]
    fn node_shape_connect_registers_and_disconnect_deregisters() {
        let registry = crate::cluster::NodeRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let channel = crate::cluster::ReverseRpcChannel::new(tx);
        let params = serde_json::json!({
            "device_id": "node-7",
            "device_name": "build-box",
            "commands": [{"name": "bash", "schema": {}}],
            "tags": ["linux"]
        });
        // Same decision + registration sequence the dispatch loop runs on a
        // successful connect (admission resolved to the presented id).
        let claim = node_connect_claim(Some(&params)).expect("node shape");
        let node_id = claim.presented_id.expect("presented id");
        assert!(crate::cluster::maybe_register_node(
            &registry,
            Some("node"),
            &node_id,
            "conn-1",
            Some(&params),
            &channel,
        ));
        // Online + resolvable, as environments.list / node_invoke require.
        assert_eq!(
            registry.node_identity_by_conn("conn-1"),
            Some(("node-7".to_string(), "build-box".to_string()))
        );
        let envs = registry.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "node-7");
        assert_eq!(envs[0].commands[0].name, "bash");
        // The existing disconnect path deregisters by conn_id.
        assert!(registry.deregister("conn-1"));
        assert!(registry.node_identity_by_conn("conn-1").is_none());
        assert!(registry.list_environments().is_empty());
    }

    #[test]
    fn overflow_warning_frame_has_reconnect_advice() {
        let frame = overflow_warning_frame(7, 42);
        let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["method"].as_str(), Some("event"));
        assert_eq!(v["params"]["topic"].as_str(), Some("connection.warning"));
        assert_eq!(
            v["params"]["data"]["reason"].as_str(),
            Some("events_overflow")
        );
        assert_eq!(v["params"]["data"]["dropped"].as_u64(), Some(7));
        assert_eq!(v["params"]["data"]["total_overflow"].as_u64(), Some(42));
        assert_eq!(v["params"]["data"]["advice"].as_str(), Some("reconnect"));
    }

    #[tokio::test]
    async fn forwarder_survives_lag_and_forwards_post_lag_events() {
        // Regression: a transient broadcast `Lagged` must NOT kill the forwarder.
        // Overflow a small bus, then enqueue a post-lag event and close the bus.
        let (bus_tx, bus_rx) = broadcast::channel::<String>(4);
        let (buffer, mut client_rx) = PerClientBuffer::with_capacity(256);
        let metrics = buffer.metrics().clone();

        for i in 0..10 {
            let _ = bus_tx.send(format!("e{i}"));
        }
        let _ = bus_tx.send("after".to_string());
        drop(bus_tx); // forwarder returns on `Closed` once retained events drain

        // Must terminate (not hang): proves `Lagged` is handled, not fatal.
        forward_bus_to_client(bus_rx, buffer).await;

        // The global-hop drop was accounted on the shared overflow metric.
        assert!(metrics.overflow() >= 1, "lag must be counted as overflow");

        // The most recent event (sent AFTER the lag-inducing burst) survived and
        // was forwarded — the OLD `while let Ok` loop would have broken before it.
        let mut last = None;
        while let Ok(s) = client_rx.try_recv() {
            last = Some(s);
        }
        assert_eq!(last.as_deref(), Some("after"));
    }

    #[tokio::test]
    async fn forwarder_forwards_all_events_without_lag() {
        // Healthy path: every event reaches the client buffer in order.
        let (bus_tx, bus_rx) = broadcast::channel::<String>(64);
        let (buffer, mut client_rx) = PerClientBuffer::with_capacity(256);
        let metrics = buffer.metrics().clone();

        for i in 0..5 {
            let _ = bus_tx.send(format!("m{i}"));
        }
        drop(bus_tx);
        forward_bus_to_client(bus_rx, buffer).await;

        assert_eq!(metrics.overflow(), 0, "no overflow on the healthy path");
        for i in 0..5 {
            assert_eq!(
                client_rx.try_recv().ok().as_deref(),
                Some(format!("m{i}").as_str())
            );
        }
    }

    #[tokio::test]
    async fn forwarder_terminates_when_client_receiver_dropped() {
        // Regression (task-leak): when the connection closes, its sole per-client
        // receiver drops, so the forwarder must reap itself instead of leaking for
        // the process lifetime. The global bus is kept OPEN for the whole await, so
        // termination can ONLY come from the send-failure path — never
        // `RecvError::Closed`. A hang here means the leak (one stranded task holding
        // a live global-bus receiver per WS disconnect) is back.
        let (bus_tx, bus_rx) = broadcast::channel::<String>(16);
        let (buffer, client_rx) = PerClientBuffer::with_capacity(256);

        drop(client_rx); // connection closed: sole per-client receiver gone
        let _ = bus_tx.send("orphan".to_string()); // wakes forwarder → send fails

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            forward_bus_to_client(bus_rx, buffer),
        )
        .await
        .expect("forwarder must terminate once its per-client receiver is dropped");

        // bus_tx is still alive here → proves termination was NOT via bus closure.
        drop(bus_tx);
    }

    // ── Trusted-proxy IP parsing (F5) ─────────────────────────────────────

    #[test]
    fn parses_trusted_ips_dropping_garbage() {
        let parsed = super::parse_trusted_ips(&[
            "127.0.0.1".to_string(),
            "::1".to_string(),
            "not-an-ip".to_string(),
        ]);
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains(&"127.0.0.1".parse().unwrap()));
        assert!(parsed.contains(&"::1".parse().unwrap()));
    }

    #[test]
    fn insecure_remote_gate_truth_table() {
        use std::net::IpAddr;
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let remote: IpAddr = "203.0.113.9".parse().unwrap();

        // Loopback is always allowed, secure or not, regardless of the flag.
        assert!(!super::refuse_insecure_remote(lo, false, false));
        assert!(!super::refuse_insecure_remote(lo, false, true));

        // Remote + insecure + not allowed ⇒ refuse.
        assert!(super::refuse_insecure_remote(remote, false, false));
        // Remote + secure ⇒ allow.
        assert!(!super::refuse_insecure_remote(remote, true, false));
        // Remote + insecure + explicitly allowed ⇒ allow.
        assert!(!super::refuse_insecure_remote(remote, false, true));
    }
}
