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
use tokio::sync::{broadcast, RwLock};
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
    /// Socket peer IP. Used for the per-IP connection cap and rate-limit
    /// identity.
    client_ip: IpAddr,
    /// Per-connection reverse-RPC registry (shared Arc with `GatewayServer`).
    /// The connection registers its `ReverseRpcChannel` here on connect and
    /// removes it on cleanup, so reverse-RPC callers can reach this socket.
    reverse_rpc: Arc<RwLock<HashMap<String, crate::cluster::ReverseRpcChannel>>>,
    /// Cluster node registry (shared Arc). The connect handler registers a
    /// `role:node` connection here and cleanup deregisters it.
    node_registry: Arc<crate::cluster::NodeRegistry>,
    /// Shared exec-approval manager for node-initiated approvals (cluster ③).
    /// `None` ⇒ `node.approval.request` is refused.
    exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
}

/// axum handler: upgrade HTTP connection to WebSocket at `/ws`
pub(super) async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<GatewaySharedState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    // IP-keyed abuse protections (per-IP cap, rate limiting) key off the
    // socket peer address verbatim. The trusted-proxy / X-Forwarded-For
    // resolution died with the LAN-trust revert.
    let client_ip = peer_addr.ip();

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
    let channel_class = if peer_addr.ip().is_loopback() {
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
            client_ip,
            reverse_rpc: state.reverse_rpc.clone(),
            node_registry: state.node_registry.clone(),
            exec_approval_manager: state.exec_approval_manager.clone(),
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
/// Only a closed bus terminates this forwarder. (Previously a `while let Ok(..)`
/// loop treated `Lagged` as fatal, silently killing the task and leaving the
/// socket open but permanently event-starved.)
async fn forward_bus_to_client(mut rx: broadcast::Receiver<String>, buffer: PerClientBuffer) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let _ = buffer.try_send(event);
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                buffer.metrics().add_overflow(n);
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Whether the given serialized event frame is a `token_rotated` notification.
/// Parses the JSON once and checks `type == "token_rotated"`. Pure, host-testable.
fn is_token_rotated_frame(event_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(event_json)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|s| s == "token_rotated")
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
    let rpc_channel = crate::cluster::ReverseRpcChannel::new(rpc_out_tx);
    let rpc_pending = rpc_channel.pending();
    // Clone kept in scope so a node-shaped connect (params carrying
    // `commands`/`tags`) can register this same channel into the NodeRegistry.
    let rpc_channel_for_node = rpc_channel.clone();
    {
        let mut reg = ctx.reverse_rpc.write().await;
        reg.insert(conn_id.clone(), rpc_channel);
    }
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
    // Last observed cumulative event-overflow count. The bus->buffer forwarder
    // accounts global-hop drops here; the ping tick below acts on any growth.
    let last_overflow: u64 = 0;

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
                                        tokio::spawn(async move {
                                            let outcome = crate::approval::run_node_approval(
                                                &manager,
                                                &event_bus,
                                                &node_id,
                                                &node_name,
                                                &tool,
                                                &reason,
                                            )
                                            .await;
                                            let resp = JsonRpcResponse::success(
                                                req_id,
                                                serde_json::json!({ "outcome": outcome }),
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
                                        let do_lane_dispatch = |text: String, lm: Arc<LaneManager>, mc: MiddlewareChain, method: String, req_id: Option<serde_json::Value>, class: ChannelClass, caller_role: Option<String>, caller_is_loopback: bool| async move {
                                            let lane_result = lm.acquire(&method, class).await;
                                            match lane_result {
                                                Ok(_permit) => crate::gateway::caller_identity::CALLER_ROLE
                                                    .scope(caller_role, crate::gateway::caller_identity::CALLER_IS_LOOPBACK
                                                        .scope(caller_is_loopback, process_request(&text, &mc)))
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
                                                                let resp = crate::gateway::caller_identity::CALLER_ROLE
                                                                    .scope(caller_role.clone(), crate::gateway::caller_identity::CALLER_IS_LOOPBACK
                                                                        .scope(ctx.client_ip.is_loopback(), process_request(&text, &ctx.middleware_chain)))
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
                                                do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone(), ctx.client_ip.is_loopback()).await
                                            }
                                        } else {
                                            // No idempotency key — standard lane dispatch
                                            do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone(), ctx.client_ip.is_loopback()).await
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
                                                    // validate the shared Gateway token presented in
                                                    // `connect` params: valid ⇒ operator (same
                                                    // authority as local, single tier), missing /
                                                    // invalid ⇒ unauthorized (login wall; the Panel
                                                    // renders a token box). The decision lives in the
                                                    // tested pure `connect::connect_authorized`.
                                                    let presented_token = req
                                                        .params
                                                        .as_ref()
                                                        .and_then(|p| p.get("token"))
                                                        .and_then(|v| v.as_str());
                                                    let authorized =
                                                        crate::gateway::handlers::connect::connect_authorized(
                                                            ctx.client_ip.is_loopback(),
                                                            presented_token,
                                                            |t| {
                                                                crate::gateway::security::SharedTokenManager::global()
                                                                    .map(|m| m.validate(t).unwrap_or(false))
                                                                    .unwrap_or(false)
                                                            },
                                                        );
                                                    let panel_role =
                                                        if authorized { "operator" } else { "guest" };
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
                                                            // Single-tier role for the login-wall gate:
                                                            // operator (authorized) or guest (walled).
                                                            state.caller_role = panel_role.to_string();
                                                        }
                                                    }
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
                                                    }
                                                    response =
                                                        serde_json::to_string(&resp).unwrap_or(response);

                                                    // Track presence for no-auth connect
                                                    {
                                                        let conns = ctx.connections.read().await;
                                                        if let Some(state) = conns.get(&conn_id) {
                                                            let presence_entry = PresenceEntry {
                                                                conn_id: conn_id.clone(),
                                                                device_id: None,
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

                                                    // Cluster Phase 0b: token roles are gone, so a
                                                    // node announces itself by request shape — the
                                                    // node client always sends `commands` + `tags`
                                                    // in its connect params, which no other client
                                                    // does. Register it so it surfaces in
                                                    // environments.list and becomes reachable via
                                                    // node_invoke/node_file, emit the
                                                    // `node.connected` lifecycle event (sibling of
                                                    // presence.joined), and stamp last_seen for the
                                                    // offline view.
                                                    if let Some(node_id) =
                                                        node_connect_identity(req.params.as_ref(), &conn_id)
                                                    {
                                                        if crate::cluster::maybe_register_node(
                                                            &ctx.node_registry,
                                                            Some("node"),
                                                            &node_id,
                                                            &conn_id,
                                                            req.params.as_ref(),
                                                            &rpc_channel_for_node,
                                                        ) {
                                                            let name = req
                                                                .params
                                                                .as_ref()
                                                                .and_then(|p| p.get("device_name"))
                                                                .and_then(|v| v.as_str())
                                                                .unwrap_or("unknown");
                                                            let _ = ctx.event_bus.publish_json(&TopicEvent::new(
                                                                "node.connected",
                                                                serde_json::json!({"node_id": &node_id, "name": name, "conn_id": &conn_id}),
                                                            ));
                                                            // Stamp the node device's last_seen_at so that
                                                            // once it goes offline, environments.list can
                                                            // show an honest "last seen".
                                                            if let Some(store) = ctx.security_store.as_ref() {
                                                                if let Err(e) = store.touch_device(&node_id) {
                                                                    debug!("failed to stamp node last_seen on connect for {}: {}", node_id, e);
                                                                }
                                                            }
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
            // Server-initiated WS Ping + inbound idle watchdog
            _ = ping_timer.tick() => {
                // Global-hop overflow watchdog. The bus->buffer forwarder drops
                // events on transient lag and records them on the shared metric
                // (without access to this socket). If it grew, apply the same
                // slow-consumer policy as the per-client drain arm: warn the
                // client to reconnect, then close 1008 so it re-syncs from the
                // hello snapshot. Bounded by the ping interval; the client just
                // misses some events until then, which the resync recovers.
                let overflow_now = buffer_metrics.overflow();
                if overflow_now > last_overflow {
                    let dropped = overflow_now - last_overflow;
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

    {
        let mut reg = ctx.reverse_rpc.write().await;
        reg.remove(&conn_id);
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

/// LAN-trust cluster-node detection at `connect` time.
///
/// Token roles are gone, so a cluster node announces itself by request shape:
/// the node client (`aleph-server node`) always sends `commands` + `tags` in
/// its connect params, which no other client does. When the shape matches,
/// returns the id to register the node under — the explicit `device_id` param
/// when present, else `device_name`, else the connection id (always
/// available). Returns `None` for every non-node connect.
fn node_connect_identity(params: Option<&serde_json::Value>, conn_id: &str) -> Option<String> {
    let p = params?;
    if p.get("commands").is_none() && p.get("tags").is_none() {
        return None;
    }
    Some(
        p.get("device_id")
            .and_then(|v| v.as_str())
            .or_else(|| p.get("device_name").and_then(|v| v.as_str()))
            .map_or_else(|| conn_id.to_string(), String::from),
    )
}

#[cfg(test)]
mod token_rotation_tests {
    use super::rotated_should_close_remote;

    const ROTATED: &str = r#"{"type":"token_rotated"}"#;

    #[test]
    fn remote_session_closes_on_token_rotated() {
        assert!(rotated_should_close_remote(ROTATED, false));
    }

    #[test]
    fn loopback_session_ignores_token_rotated() {
        assert!(!rotated_should_close_remote(ROTATED, true));
    }

    #[test]
    fn other_events_never_trigger_close() {
        assert!(!rotated_should_close_remote(
            r#"{"type":"acp_sessions_changed"}"#,
            false
        ));
        assert!(!rotated_should_close_remote(
            r#"{"topic":"alerts.system"}"#,
            false
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── LAN-trust node-shape detection + cluster registration ────────────

    #[test]
    fn node_connect_identity_detects_commands_or_tags_shape() {
        // The node client always sends both `commands` and `tags`.
        let full = serde_json::json!({
            "token": "stale:sig",
            "device_name": "build-box",
            "commands": [],
            "tags": ["linux"]
        });
        assert_eq!(
            node_connect_identity(Some(&full), "conn-1").as_deref(),
            Some("build-box")
        );
        // Either key alone is enough.
        let tags_only = serde_json::json!({"device_name": "t", "tags": []});
        assert!(node_connect_identity(Some(&tags_only), "c").is_some());
        let commands_only = serde_json::json!({"device_name": "c2", "commands": []});
        assert!(node_connect_identity(Some(&commands_only), "c").is_some());
        // Explicit device_id wins over device_name.
        let with_id = serde_json::json!({
            "device_id": "node-7", "device_name": "x", "commands": [], "tags": []
        });
        assert_eq!(
            node_connect_identity(Some(&with_id), "c").as_deref(),
            Some("node-7")
        );
        // Neither id nor name → conn_id fallback.
        let anon = serde_json::json!({"commands": []});
        assert_eq!(
            node_connect_identity(Some(&anon), "conn-9").as_deref(),
            Some("conn-9")
        );
    }

    #[test]
    fn ordinary_connect_params_are_not_node_shaped() {
        // Panel / CLI connects (no commands/tags) must not register as nodes.
        let panel = serde_json::json!({
            "device_name": "Web Panel",
            "channel_kind": "browser",
            "token": "legacy:sig"
        });
        assert!(node_connect_identity(Some(&panel), "c").is_none());
        let bare = serde_json::json!({});
        assert!(node_connect_identity(Some(&bare), "c").is_none());
        assert!(node_connect_identity(None, "c").is_none());
    }

    #[test]
    fn node_shape_connect_registers_and_disconnect_deregisters() {
        let registry = crate::cluster::NodeRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let channel = crate::cluster::ReverseRpcChannel::new(tx);
        let params = serde_json::json!({
            "token": "stale:sig",
            "device_name": "build-box",
            "commands": [{"name": "bash", "schema": {}}],
            "tags": ["linux"]
        });
        // Same decision + registration sequence the dispatch loop runs on a
        // successful connect.
        let node_id = node_connect_identity(Some(&params), "conn-1").expect("node shape");
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
            Some(("build-box".to_string(), "build-box".to_string()))
        );
        let envs = registry.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "build-box");
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
}
