//! WebSocket connection lifecycle orchestrator.
//!
//! Splits the original monolithic `handler.rs` (3527 lines) into five
//! per-phase submodules — `upgrade`, `auth`, `dispatch`, `forward`,
//! `cleanup` — plus this orchestrator. Each phase file owns the pure
//! helpers and (where applicable) the per-phase live logic; this file
//! owns the per-connection state ([`ConnectionContext`]) and the single
//! `tokio::select!` loop that interleaves inbound RPC dispatch,
//! outbound event forwarding, reverse-RPC writes, the idle/ping
//! watchdog, and the slow-consumer wedge detector.
//!
//! The split is structural only — every observable behavior (RPC
//! responses, log lines, metrics, side-effect ordering) is byte-for-byte
//! identical to the pre-split handler. The test module in `handler.rs`
//! is kept whole and re-resolves its `super::*` imports via the
//! `connection::*` re-exports declared at the bottom of `handler.rs`.

use crate::sync_primitives::Arc;
use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket};
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
use crate::gateway::rate_limiter::{
    scope_for_method, RateLimitError, RateLimitKey, RateLimitScope, RateLimiter,
};
use crate::gateway::state_version::StateVersionTracker;

use super::per_client_buffer::PerClientBuffer;
use super::ConnectionState;
use crate::gateway::security::SecurityStore;

pub mod upgrade;
pub mod auth;
pub mod dispatch;
pub mod forward;
pub mod cleanup;

// Bring every helper used by the orchestrator into this module's scope.
// (The test module in `handler.rs` reaches them via the re-exports
// declared at the bottom of this file.)
pub use forward::{
    device_revoked_id, device_revoked_should_close, event_wire_form, extract_topic_and_data,
    forward_bus_to_client, is_token_rotated_frame, overflow_warning_frame,
    rotated_should_close_remote, DEVICE_REVOKED_TOPIC, TOKEN_ROTATED_TOPIC,
};
pub use auth::{
    connect_verdict, node_connect_claim, resolve_stamped_identity, wall_admits, NodeConnectClaim,
};
pub use dispatch::{dispatch_with_caller_context, process_request};
pub use upgrade::{parse_trusted_ips, refuse_insecure_remote, ws_upgrade_handler};

// `cleanup::run_cleanup` is invoked by name only, no import needed.

/// Shared context for handling a WebSocket connection.
pub struct ConnectionContext {
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
    /// connection cap, the rate-limit identity and audit rows — i.e. for
    /// *bucketing*. Never for authority: that is [`Self::client_is_local`].
    client_ip: IpAddr,
    /// Whether this connection is genuinely local — loopback peer AND not a
    /// trusted-proxy hop. Every loopback *privilege* (zero-config operator at
    /// `connect`, per-IP cap exemption, rate-limit exemption, desktop lane
    /// pool, "do not kick on token rotation") reads THIS, not
    /// `client_ip.is_loopback()`: behind a same-host reverse proxy that emits
    /// no `X-Forwarded-For`, the resolved IP falls back to the proxy's own
    /// loopback address, which would turn "I could not determine the real
    /// client" into full unauthenticated operator. See
    /// [`crate::gateway::trusted_proxy::ResolvedClient::local`].
    client_is_local: bool,
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
    /// Session store for the owner-scoped WS event filter (P1 data isolation,
    /// spec §5.4). `None` ⇒ that 4th filter term is skipped (zero-change
    /// guarantee — see `GatewaySharedState::session_store`).
    session_store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    /// Team store for the same filter's `team.<id>.*` plane (team chat bodies,
    /// published as raw `{topic,data}` strings). `None` ⇒ those frames are
    /// denied — see `GatewaySharedState::team_store`.
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    /// Process-shared run→session / session→owner cache backing the filter.
    /// See `crate::gateway::event_visibility`.
    event_visibility: Arc<crate::gateway::event_visibility::EventVisibilityIndex>,
}


pub async fn handle_connection(
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
        conns.insert(
            conn_id.clone(),
            ConnectionState::new(ctx.client_ip, ctx.client_is_local),
        );
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
                                    // clients behind it), not identity (device_id).
                                    //
                                    // This layer keys on the CLIENT IP — it is the
                                    // network-origin isolator (the principal-axis
                                    // check lives in middleware/rate_limit.rs).
                                    // `connect` would otherwise map to the strict
                                    // Auth scope (10/min + 5-min lockout): every
                                    // user behind one NAT / reverse-proxy egress
                                    // shares that single IP bucket, so one client
                                    // retrying a stale token locked every other
                                    // user at that address out of the handshake
                                    // for the full lockout. Remap to the
                                    // lockout-free default bucket instead — the
                                    // same remap middleware/rate_limit.rs performs
                                    // on the pooled "rpc" identity — so a token
                                    // storm only exhausts the offending origin's
                                    // own window, which recovers as the window
                                    // slides.
                                    if !ctx.client_is_local {
                                    let rl_identity = ctx.client_ip.to_string();
                                    let rl_scope_raw = scope_for_method(&req.method);
                                    let rl_scope = if matches!(rl_scope_raw, RateLimitScope::Auth) {
                                        RateLimitScope::RpcDefault
                                    } else {
                                        rl_scope_raw
                                    };
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
                                            if ctx.client_is_local {
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
                                        ctx.client_is_local
                                            .then(|| crate::gateway::security::store::OWNER_USER_ID.to_string())
                                    });

                                    // Login wall (Gateway-token model): an
                                    // unauthorized connection — a remote Panel
                                    // that has not presented a valid Gateway
                                    // token — may only (re)issue `connect` to
                                    // authorize. Every other method is refused
                                    // until a valid credential is presented.
                                    // Loopback, token-authorized and
                                    // member-resolved connections pass freely
                                    // here; the admin/member split is a
                                    // *separate*, deeper gate (`method_admin.rs`
                                    // inside `process_request`). See
                                    // `wall_admits` for the predicate and why it
                                    // is extracted (host-testable).
                                    if !wall_admits(caller_role.as_deref(), &req.method) {
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
                                                )).await;
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

                                        // Extract idempotency_key from params (optional).
                                        //
                                        // **Namespaced slot**: the raw caller-supplied string is
                                        // NEVER the cache key by itself. The slot is scoped to
                                        // (principal, method, key) — without the first two
                                        // components a replay crosses identities and methods:
                                        //   * cross-method: a `config.patch` result cached under
                                        //     bare "1" would be served to any later
                                        //     `tools.invoke` reusing "1" within the TTL;
                                        //   * cross-principal / gate bypass: `method_requires_admin`
                                        //     and `method_visibility` run INSIDE `process_request`,
                                        //     but the Cached/Waiting arms return before it — a
                                        //     response computed for an operator would be replayed
                                        //     to a member unfiltered.
                                        // `caller_user` / `caller_role` are resolved ABOVE this
                                        // block (login-wall section), so they are already in
                                        // scope here.
                                        const IDEM_NS_SEP: char = '\u{1f}';
                                        let raw_idem_key = req.params
                                            .as_ref()
                                            .and_then(|p| p.get("idempotency_key"))
                                            .and_then(|v| v.as_str());

                                        // A caller-supplied key containing the namespace
                                        // separator could forge the (principal, method)
                                        // prefix of another slot — reject it outright.
                                        if let Some(k) = raw_idem_key {
                                            if k.contains(IDEM_NS_SEP) {
                                                let resp = JsonRpcResponse::error(
                                                    req.id.clone(),
                                                    -32602, // JSON-RPC invalid params
                                                    "idempotency_key contains a reserved separator character",
                                                );
                                                let resp_str = serde_json::to_string(&resp).unwrap_or_default();
                                                if let Err(e) = write.send(WsMessage::Text(resp_str.into())).await {
                                                    error!("Failed to send idempotency-key error to {}: {}", conn_id, e);
                                                    break;
                                                }
                                                continue;
                                            }
                                        }

                                        let idempotency_key = raw_idem_key.map(|k| {
                                            let composed = format!(
                                                "{}{}{}{}{}",
                                                caller_user.as_deref().unwrap_or("<anon>"),
                                                IDEM_NS_SEP,
                                                req.method,
                                                IDEM_NS_SEP,
                                                k
                                            );
                                            debug_assert!(
                                                composed.matches(IDEM_NS_SEP).count() == 2,
                                                "namespaced idempotency key must carry exactly two separators"
                                            );
                                            composed
                                        });

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
                                        let do_lane_dispatch = |text: String, lm: Arc<LaneManager>, mc: MiddlewareChain, method: String, req_id: Option<serde_json::Value>, class: ChannelClass, caller_role: Option<String>, caller_user: Option<String>, caller_is_loopback: bool, caller_conn_id: Option<String>| async move {
                                            let lane_result = lm.acquire(&method, class).await;
                                            match lane_result {
                                                Ok(_permit) => dispatch_with_caller_context(&text, &mc, caller_role, caller_user, caller_is_loopback, caller_conn_id).await,
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
                                                                let resp = dispatch_with_caller_context(&text, &ctx.middleware_chain, caller_role.clone(), caller_user.clone(), ctx.client_is_local, Some(conn_id.clone())).await;
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
                                                do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone(), caller_user.clone(), ctx.client_is_local, Some(conn_id.clone())).await
                                            }
                                        } else {
                                            // No idempotency key — standard lane dispatch
                                            do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone(), caller_user.clone(), ctx.client_is_local, Some(conn_id.clone())).await
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
                                                            ctx.client_is_local,
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
                                                            ctx.client_is_local,
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

                                                    // Credential verdict only — "did this connection
                                                    // present something valid". The *role* it maps to
                                                    // is no longer decided here: per-user resolution
                                                    // (`resolve_stamped_identity`, below) owns that
                                                    // for both the stamp and the response, so there
                                                    // is exactly one answer to "who is this".
                                                    let (authorized, issued_device_token, authed_device_id) = match &auth_outcome {
                                                        crate::gateway::handlers::connect::ConnectAuthOutcome::Authorized { device_id } => (true, None, device_id.clone()),
                                                        crate::gateway::handlers::connect::ConnectAuthOutcome::BootstrapExchanged { device_token, device_id } => (true, Some(device_token.clone()), Some(device_id.clone())),
                                                        crate::gateway::handlers::connect::ConnectAuthOutcome::Unauthorized => (false, None, None),
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
                                                        ctx.client_is_local,
                                                    ) {
                                                        if let Some(log) = ctx.audit_log.as_ref() {
                                                            log.log(crate::security::audit::AuditEntry::auth_failure(
                                                                ctx.client_ip.to_string(),
                                                                "remote connect rejected: no valid Gateway credential",
                                                            )).await;
                                                        }
                                                    }
                                                    // Role + user for the login-wall gate and the
                                                    // config-tier tool gate. See
                                                    // `resolve_stamped_identity` (pure, unit-tested)
                                                    // for the decision rules. Resolved BEFORE taking
                                                    // the connection-map lock (it is a pure function;
                                                    // holding the lock across it buys nothing) so the
                                                    // very same verdict can be echoed to the client
                                                    // in the response overlay below.
                                                    let (resolved_user, resolved_role) = resolve_stamped_identity(
                                                        authorized,
                                                        ctx.client_is_local,
                                                        authed_device_id.as_deref(),
                                                        ctx.security_store.as_deref(),
                                                    );
                                                    // What the client is told, and what the event
                                                    // scope grants. A connect can be
                                                    // credential-authorized yet resolve to no
                                                    // principal (deactivated / dangling user); the
                                                    // login wall already treats that as guest, so the
                                                    // response and the scope must agree — otherwise
                                                    // the client is told `authorized: true` and
                                                    // handed a dead UI in which every later frame is
                                                    // refused, while a wildcard scope keeps streaming
                                                    // guarded topics (approval banners,
                                                    // config.changed) to a principal that no longer
                                                    // exists. For every pre-P0 shape (loopback,
                                                    // shared token, unbound device, unauthorized)
                                                    // this triple is byte-identical to what the old
                                                    // credential-only `panel_role`/`authorized` pair
                                                    // produced.
                                                    let (echo_role, holds_authority, needs_token) =
                                                        connect_verdict(authorized, resolved_role);
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
                                                                crate::gateway::surface::SurfaceKind::Unknown if ctx.client_is_local => {
                                                                    crate::gateway::surface::SurfaceKind::Desktop
                                                                }
                                                                other => other,
                                                            };
                                                            state.channel_kind = Some(kind);
                                                            state.first_message = false;
                                                            // Event scope follows the RESOLVED role,
                                                            // through the single authority shared with
                                                            // the live re-stamp in `handlers::users`.
                                                            // Operator ⇒ the `"*"` wildcard, so
                                                            // EventScopeGuard keeps delivering guarded
                                                            // topics (approval banners, config.changed).
                                                            // Member and walled ⇒ no scopes; that is not
                                                            // a blackout, `can_receive` is default-allow
                                                            // and only the guarded prefixes stop.
                                                            //
                                                            // Keying on the role rather than
                                                            // `holds_authority` is equivalent for every
                                                            // pre-P0 shape — `resolve_stamped_identity`
                                                            // returns `"guest"` whenever `!authorized`,
                                                            // so `holds_authority == (role != "guest")`.
                                                            // It differs in exactly one case, which is
                                                            // the bug being fixed: a member holds
                                                            // authority (the login wall admits him) yet
                                                            // must not hold the admin event scope.
                                                            state.permissions =
                                                                crate::gateway::event_scope::scope_for_role(
                                                                    resolved_role,
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
                                                                if let Err(e) = ctx.event_bus.publish_json(&TopicEvent::new(
                                                                    "node.connected",
                                                                    serde_json::json!({"node_id": node_id, "name": &claim.device_name, "conn_id": &conn_id}),
                                                                )) {
                                                                tracing::warn!(
                                                                    error = %e,
                                                                    node_id = %node_id,
                                                                    conn_id = %conn_id,
                                                                    "failed to publish node.connected event"
                                                                );
                                                            }
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
                                                    // the app when authorized. The echoed role is the
                                                    // RESOLVED one (`resolve_stamped_identity`), not
                                                    // the credential-only `panel_role` guess — the
                                                    // client must be told the authority it actually
                                                    // holds, or a member renders an operator UI whose
                                                    // admin surfaces all fail, and a deactivated
                                                    // user's still-valid device is told it is a fully
                                                    // authorized operator while the wall refuses
                                                    // every frame it sends. No new vocabulary: a
                                                    // walled resolution reuses the existing
                                                    // guest/needs_token shape.
                                                    if let Some(obj) = resp
                                                        .result
                                                        .as_mut()
                                                        .and_then(serde_json::Value::as_object_mut)
                                                    {
                                                        obj.insert(
                                                            "role".to_string(),
                                                            serde_json::Value::String(
                                                                echo_role.to_string(),
                                                            ),
                                                        );
                                                        obj.insert(
                                                            "authorized".to_string(),
                                                            serde_json::Value::Bool(holds_authority),
                                                        );
                                                        obj.insert(
                                                            "needs_token".to_string(),
                                                            serde_json::Value::Bool(needs_token),
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
                                                            if let Err(e) = ctx.event_bus.publish_json(&TopicEvent::new("presence.joined", serde_json::json!({"conn_id": &conn_id})).with_state_version(ctx.state_versions.snapshot())) {
                                                                tracing::warn!(
                                                                    error = %e,
                                                                    conn_id = %conn_id,
                                                                    "failed to publish presence.joined event"
                                                                );
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
                        if rotated_should_close_remote(&event_json, ctx.client_is_local) {
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
                        // Parse ONCE. This value is what the filter chain reads
                        // and what the wire-form step below rewrites/wraps —
                        // it used to be parsed a second time purely to decide
                        // whether to wrap, which is what paid for the payload
                        // projection now folded in here.
                        let parsed = serde_json::from_str::<serde_json::Value>(&event_json).ok();
                        // Try to extract topic from event for filtering
                        let (should_forward, projected_payload) = if let Some(event_obj) = parsed.as_ref() {
                            let (topic, event_data) = extract_topic_and_data(event_obj);

                            // The login wall + permission-based scope guard check +
                            // surface audience + caller identity for the owner-scoped
                            // event filter (P1, spec §5.4). All four read the same
                            // ConnectionState under one lock — extend the tuple, don't
                            // take the lock twice.
                            let (
                                wall_ok,
                                scope_allowed,
                                channel_kind,
                                event_caller_user,
                                event_caller_role,
                            ) = {
                                let conns = ctx.connections.read().await;
                                match conns.get(&conn_id) {
                                    Some(s) => (
                                        // 0th term: the login wall, the same predicate
                                        // and the same `caller_role` field the request
                                        // arm evaluates at the top of the dispatch loop
                                        // — so a live demotion through
                                        // `restamp_live_connections` closes the event
                                        // plane in the same instant it closes the RPC
                                        // plane. The method argument is `""`: there is
                                        // no `connect` exemption to grant here, because
                                        // an event is never the frame that authorizes a
                                        // connection.
                                        wall_admits(Some(s.caller_role.as_str()), ""),
                                        ctx.event_scope_guard.can_receive(topic, &s.permissions),
                                        s.channel_kind,
                                        s.caller_user.clone(),
                                        // The 5th term reads the role too: a
                                        // fleet-scoped frame has no owner to
                                        // compare against, and the topic-prefix
                                        // term above cannot tell it apart from
                                        // its session-scoped siblings.
                                        s.caller_role.clone(),
                                    ),
                                    None => (false, false, None, None, String::new()),
                                }
                            };

                            // The owner-scoped filter needs the frame's OWN fields
                            // (run_id/session_key), which for stream.* notifications
                            // live directly under `.params` — `event_data` above only
                            // resolves for the TopicEvent `.data` and the
                            // double-wrapped `TopicEvent::to_notification()`
                            // `.params.data` shapes (both handled by
                            // `extract_topic_and_data`), not the bare stream-form
                            // `.params` (no `GatewayEventFrame` stream.* variant
                            // nests a second `.data` inside it), so fall back to the
                            // raw `.params` object for that one remaining case.
                            let visibility_payload =
                                event_data.or_else(|| event_obj.get("params"));

                            // `note_frame` runs unconditionally, before the filter —
                            // every connection's loop keeps the shared, process-wide
                            // index warm (first writer wins) regardless of whether
                            // THIS connection ends up receiving the frame.
                            ctx.event_visibility
                                .note_frame(topic, visibility_payload)
                                .await;

                            let admits = wall_ok
                                && scope_allowed
                                && crate::gateway::surface::delivery::audience_allows(
                                    event_data,
                                    channel_kind,
                                )
                                && ctx.subscription_manager.should_receive(&conn_id, topic, event_data).await
                                && match ctx.session_store.as_ref() {
                                    Some(store) => {
                                        ctx.event_visibility
                                            .event_admits_for(
                                                topic,
                                                visibility_payload,
                                                event_caller_user.as_deref(),
                                                Some(event_caller_role.as_str()),
                                                store,
                                                ctx.team_store.as_ref(),
                                            )
                                            .await
                                    }
                                    // No store wired (probe/legacy wiring): skip the
                                    // 4th term entirely — zero-change guarantee, see
                                    // `GatewaySharedState::session_store`.
                                    None => true,
                                };

                            // 5th term, and the only one that is not pass/fail:
                            // one frame (`stream.running_set_changed`) carries a
                            // set spanning every user, so it is admitted whole and
                            // its ARRAY is narrowed for this connection instead.
                            // `None` for every other topic — see
                            // `EventVisibilityIndex::project_for`, including why a
                            // narrowed-to-empty frame must still be SENT.
                            let projected = match (admits, ctx.session_store.as_ref()) {
                                (true, Some(store)) => {
                                    ctx.event_visibility
                                        .project_for(
                                            topic,
                                            visibility_payload,
                                            event_caller_user.as_deref(),
                                            store,
                                        )
                                        .await
                                }
                                _ => None,
                            };
                            (admits, projected)
                        } else {
                            // Can't parse event, forward by default
                            (true, None)
                        };

                        if should_forward {
                            debug!("Forwarding event to {}", conn_id);
                            // Apply the projection (if any) and wrap the bare
                            // TopicEvent form — see `event_wire_form`.
                            let wire_json = match parsed {
                                Some(event_obj) => {
                                    event_wire_form(event_obj, projected_payload, event_json)
                                }
                                None => event_json,
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


    cleanup::run_cleanup(&conn_id, &ctx, &rpc_pending).await;
    Ok(())
}
