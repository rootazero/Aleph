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

use crate::gateway::config::AuthMode;
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
    PARSE_ERROR, PERMISSION_DENIED, RATE_LIMITED,
};
use crate::gateway::rate_limiter::{
    scope_for_method, RateLimitError, RateLimitKey, RateLimitScope, RateLimiter,
};
use crate::gateway::state_version::StateVersionTracker;

use super::per_client_buffer::PerClientBuffer;
use super::{ConnectionState, GatewaySharedState, MAX_AUTH_ATTEMPTS};
use crate::gateway::security::TokenManager;

/// Shared context for handling a WebSocket connection.
struct ConnectionContext {
    middleware_chain: MiddlewareChain,
    event_bus: Arc<GatewayEventBus>,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    subscription_manager: Arc<SubscriptionManager>,
    guest_session_manager: Option<Arc<crate::gateway::security::GuestSessionManager>>,
    auth_mode: AuthMode,
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
    /// Locally-stored shared token to auto-inject into the first `connect`
    /// when the peer is loopback AND presented a valid `aleph_session`
    /// cookie at WS handshake. `Some` ⇒ the panel WebSocket inherits the
    /// same trust as the cookie that loaded the panel HTML, so the user
    /// is not prompted to approve a pairing code on a fresh app start.
    /// `None` ⇒ existing flow (Case 0/1/2/3 in `connect.rs`) runs
    /// untouched — non-loopback peers, dev wiring, and clients that
    /// already carry their own token / shared_token / invitation_token
    /// land here.
    bootstrap_shared_token: Option<String>,
    /// Device-token manager for the per-dispatch revocation re-check. `None`
    /// disables the check (auth-disabled / legacy wiring). Only consulted for
    /// connections that authenticated via a device token (i.e. whose
    /// `ConnectionState.device_token_hash` is `Some`).
    token_manager: Option<Arc<TokenManager>>,
    /// Resolved real client IP. Equals the socket peer IP unless the peer is a
    /// configured trusted proxy, in which case it is taken from
    /// `X-Forwarded-For`. Used for the per-IP connection cap, rate-limit
    /// identity, and auth-failure lockout so those abuse protections key off
    /// the actual client rather than a shared proxy address.
    client_ip: IpAddr,
}

/// Extract the `aleph_session` cookie value from a request's `Cookie` header.
///
/// Returns `None` when the header is missing, malformed, or does not
/// contain an `aleph_session=…` pair. Mirrors the helper in
/// [`crate::gateway::auth_middleware`] — duplicated here to avoid
/// re-exporting a private item across modules.
fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .filter_map(|c| {
                    let (name, value) = c.trim().split_once('=')?;
                    if name == "aleph_session" {
                        Some(value.to_string())
                    } else {
                        None
                    }
                })
                .next()
        })
}

/// Bridge an `aleph_session` cookie into a WS-level shared-token bootstrap.
///
/// Returns `Some(shared_token)` only when ALL hold:
///   1. peer is on a loopback interface,
///   2. shared state has both `session_mgr` and `shared_token_mgr` plumbed
///      (production wiring; dev/auth-none/legacy wiring leaves them `None`),
///   3. the `aleph_session` cookie is present and validates against the
///      session store.
///
/// Anything short of all three yields `None`, and the connection falls
/// through to the existing token / shared_token / pairing flow. Refusing
/// non-loopback peers is what keeps a LAN attacker who somehow obtains a
/// session cookie from short-circuiting auth — `/auth/bootstrap` is
/// already loopback-gated upstream, but defence-in-depth costs nothing.
/// Record a failed `connect` against the per-source-IP `Auth` rate-limit
/// bucket and report whether the source is now locked out.
///
/// Loopback peers are exempt (same trust class as the local desktop shell,
/// consistent with the rest of the dispatch path). Returns the retry/lockout
/// hint in milliseconds when the source has exhausted its auth-failure budget,
/// otherwise `None`. This is the cross-connection backstop to the
/// per-connection `auth_attempts` counter (openclaw #87148).
fn record_auth_failure_lockout(
    rate_limiter: &RateLimiter,
    peer_ip: std::net::IpAddr,
) -> Option<u64> {
    if peer_ip.is_loopback() {
        return None;
    }
    let key = RateLimitKey::new(&peer_ip.to_string(), RateLimitScope::Auth);
    match rate_limiter.check_and_record(&key) {
        Ok(()) => None,
        Err(RateLimitError::Exceeded { retry_after_ms, .. }) => Some(retry_after_ms),
        Err(RateLimitError::LockedOut {
            lockout_remaining_ms,
            ..
        }) => Some(lockout_remaining_ms),
    }
}

fn resolve_bootstrap_shared_token(
    state: &Arc<GatewaySharedState>,
    peer_addr: &SocketAddr,
    headers: &HeaderMap,
) -> Option<String> {
    if !peer_addr.ip().is_loopback() {
        return None;
    }
    let session_mgr = state.session_mgr.as_ref()?;
    let shared_token_mgr = state.shared_token_mgr.as_ref()?;
    let session_id = extract_session_cookie(headers)?;
    match session_mgr.validate_session(&session_id) {
        Ok(true) => {}
        Ok(false) => return None,
        Err(e) => {
            warn!(
                error = %e,
                "ws upgrade: session_mgr.validate_session failed; falling back to pairing"
            );
            return None;
        }
    }
    // Prefer the in-memory cache populated at boot; fall through to a DB
    // read if this process has not loaded the token yet (e.g. in tests
    // that construct a fresh manager). Both paths are O(1) on the hot
    // upgrade path, so the cost is negligible.
    shared_token_mgr
        .get_current_token()
        .or_else(|| shared_token_mgr.try_load_token_from_db())
}

/// Rewrite the first `connect` JSON-RPC frame to carry the locally-known
/// shared token, so a cookie-bootstrapped Panel rides Case 0 in
/// `connect.rs` instead of falling into the device-pairing branch.
///
/// Returns the original `text` unchanged when ANY of these hold:
///   * no bootstrap token is available (Cookie absent / invalid / non-loopback),
///   * the method is not exactly `connect` (e.g. `connect.challenge`
///     never carries credentials),
///   * the client already supplied an explicit `token`, `shared_token`,
///     or `invitation_token` (we never overwrite client intent),
///   * the JSON cannot be re-parsed as an object with an object `params`
///     (defensive — the upstream `serde_json::from_str` already succeeded
///     into a typed `JsonRpcRequest`, but a `params: null` payload is
///     legal in JSON-RPC and there is nowhere to insert the field).
///
/// The injection is purely additive (one extra string field), so the
/// re-serialised frame remains a valid JSON-RPC 2.0 request.
fn maybe_inject_bootstrap_shared_token(
    text: &str,
    req: &JsonRpcRequest,
    bootstrap_shared_token: Option<&str>,
) -> String {
    let Some(token) = bootstrap_shared_token else {
        return text.to_string();
    };
    if req.method != "connect" {
        return text.to_string();
    }
    // Sniff the parsed params for any already-set credential field. We
    // intentionally read the typed `req.params` rather than the raw JSON
    // — `JsonRpcRequest::params` is `Option<Value>` so this stays a
    // single allocation-free pointer-deref.
    if let Some(params) = req.params.as_ref().and_then(|v| v.as_object()) {
        if params.contains_key("token")
            || params.contains_key("shared_token")
            || params.contains_key("invitation_token")
        {
            return text.to_string();
        }
    }

    // Re-serialise the frame with `params.shared_token` injected.
    let Ok(mut envelope) = serde_json::from_str::<serde_json::Value>(text) else {
        return text.to_string();
    };
    // Insert into `params` (creating it if `params` is null / missing).
    let params_slot = envelope
        .as_object_mut()
        .map(|m| m.entry("params").or_insert_with(|| serde_json::json!({})));
    let Some(params) = params_slot else {
        return text.to_string();
    };
    let Some(params_obj) = params.as_object_mut() else {
        return text.to_string();
    };
    params_obj.insert(
        "shared_token".to_string(),
        serde_json::Value::String(token.to_string()),
    );
    serde_json::to_string(&envelope).unwrap_or_else(|_| text.to_string())
}

/// axum handler: upgrade HTTP connection to WebSocket at `/ws`
pub(super) async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<GatewaySharedState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    // Resolve the real client IP up front. Equals the socket peer IP unless
    // the peer is a configured trusted reverse proxy, in which case it is
    // taken from `X-Forwarded-For`. All IP-keyed abuse protections (per-IP
    // cap, rate limiting, auth-failure lockout) use this so they isolate the
    // real client rather than a shared proxy address.
    let client_ip = state
        .trusted_proxies
        .real_client_ip(peer_addr.ip(), &headers);

    // Cross-origin / DNS-rebinding guard. A browser always attaches an
    // `Origin` header to a WS upgrade and cannot forge it, so a malicious page
    // that reaches this loopback socket is rejected when its origin is neither
    // same-origin nor allow-listed. Native clients (CLI, bots, bridges) send
    // no `Origin` and pass through untouched. Enforces the long-documented
    // `[gateway.auth] allowed_origins` contract.
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

    let bootstrap_shared_token = resolve_bootstrap_shared_token(&state, &peer_addr, &headers);
    if bootstrap_shared_token.is_some() {
        debug!(
            "ws upgrade: loopback peer {} presented a valid aleph_session cookie; \
             shared token will be auto-injected into the first connect",
            peer_addr
        );
    }

    ws.on_upgrade(move |socket| async move {
        let ctx = ConnectionContext {
            // Shared chain built once at server construction (cloning shares the
            // global request-state registry instead of resetting it per connect).
            middleware_chain: state.middleware_chain.clone(),
            event_bus: state.event_bus.clone(),
            connections: state.connections.clone(),
            subscription_manager: state.subscription_manager.clone(),
            guest_session_manager: state.guest_session_manager.clone(),
            auth_mode: state.auth_mode.clone(),
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
            bootstrap_shared_token,
            token_manager: state.token_manager.clone(),
            client_ip,
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

/// Handle a single WebSocket connection
async fn handle_connection(
    socket: WebSocket,
    peer_addr: SocketAddr,
    ctx: ConnectionContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut write, mut read) = socket.split();
    let conn_id = format!("{}", peer_addr);

    info!("New WebSocket connection: {}", conn_id);

    let (buffer, mut client_event_rx) = PerClientBuffer::new();
    let buffer_metrics = buffer.metrics().clone();

    tokio::spawn(forward_bus_to_client(ctx.event_bus.subscribe(), buffer));

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
                        let preview_end = text.char_indices().take_while(|(i, _)| *i < 200).last().map(|(i, c)| i + c.len_utf8()).unwrap_or(text.len());
                        debug!("WS recv from {}: {}", conn_id, &text[..preview_end]);

                        // Parse request to check method for auth gating
                        let request: Result<JsonRpcRequest, _> = serde_json::from_str(&text);

                        let response = match request {
                            Ok(ref req) => {
                                // Check authentication requirement
                                let (is_first, is_authenticated, device_token_hash) = {
                                    let conns = ctx.connections.read().await;
                                    let state = conns.get(&conn_id);
                                    (
                                        state.is_none_or(|s| s.first_message),
                                        state.is_some_and(|s| s.authenticated),
                                        state.and_then(|s| s.device_token_hash.clone()),
                                    )
                                };

                                // Per-dispatch device-token revocation re-check.
                                // A connection that authenticated with a device
                                // token (Case 1) carries its token hash; if that
                                // token has since been revoked (token rotation or
                                // device removal), close the connection instead of
                                // serving further requests. Connections that
                                // authenticated by any other means carry no hash
                                // and are exempt, so the local panel
                                // (shared-token/loopback) is never affected.
                                // (openclaw #70707)
                                if is_authenticated {
                                    if let (Some(tm), Some(hash)) =
                                        (ctx.token_manager.as_ref(), device_token_hash.as_ref())
                                    {
                                        if tm.is_token_hash_revoked(hash) {
                                            warn!(
                                                "Connection {} device token revoked, disconnecting",
                                                conn_id
                                            );
                                            let response_str = serde_json::to_string(
                                                &JsonRpcResponse::error(
                                                    req.id.clone(),
                                                    AUTH_REQUIRED,
                                                    "Device token revoked; please re-authenticate",
                                                ),
                                            )
                                            .unwrap_or_default();
                                            let _ = write
                                                .send(WsMessage::Text(response_str.into()))
                                                .await;
                                            break;
                                        }
                                    }
                                }

                                // Pairing wizard bootstrap exception:
                                // a same-machine panel that just got a
                                // `pairing_required` error has no token yet,
                                // but needs to drive `wizard.*` to obtain one.
                                // Allow it ONLY on loopback so LAN clients
                                // can't bypass auth. wizard.* never mutates
                                // is_authenticated/first_message; panel will
                                // reconnect() after wizard.answer issues a
                                // token, going through the normal `connect`
                                // path with the real credentials.
                                let allow_unauth_wizard =
                                    allow_unauth_loopback_wizard(&peer_addr, &req.method)
                                        || allow_unauth_browser_pairing(&req.method);

                                // Auth gating logic
                                if ctx.auth_mode.is_auth_required()
                                    && !is_authenticated
                                    && !allow_unauth_wizard
                                {
                                    // Allow `connect.challenge` pre-auth too — clients need
                                    // to fetch a nonce before they can sign a `connect`.
                                    let is_connect_family =
                                        req.method == "connect" || req.method == "connect.challenge";

                                    // First message must be "connect" (or "connect.challenge")
                                    if is_first && !is_connect_family {
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
                                    if !is_first && !is_connect_family {
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
                                        // Cookie-bootstrap injection: a loopback peer that
                                        // presented a valid `aleph_session` cookie at WS
                                        // handshake (validated in `ws_upgrade_handler`)
                                        // should not be asked to pair again — the cookie
                                        // and the shared token are the same trust class.
                                        // We rewrite the first `connect` to carry
                                        // `shared_token=<local>` so it rides Case 0 in
                                        // `connect.rs` and skips the device-pairing
                                        // branch. We never overwrite an explicit
                                        // `token` / `shared_token` / `invitation_token`
                                        // — the client may legitimately want a different
                                        // identity than the local desktop session.
                                        let text_for_dispatch = maybe_inject_bootstrap_shared_token(
                                            &text,
                                            req,
                                            ctx.bootstrap_shared_token.as_deref(),
                                        );

                                        // Handle connect request
                                        let response = process_request(&text_for_dispatch, &ctx.middleware_chain).await;

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

                                                // Role drives the method-level authorization gate. Guest sessions are
                                                // always "guest"; device connects carry the role from the connect
                                                // response, derived from the device's permissions (chat tier → "guest",
                                                // config tier → "operator"). The fallback is "guest" (the safe,
                                                // least-privilege default): every connect success path sets `role`
                                                // explicitly, so this only guards a future path that forgets to.
                                                let role = if guest_session_id.is_some() {
                                                    Some("guest".to_string())
                                                } else {
                                                    Some(
                                                        resp.result
                                                            .as_ref()
                                                            .and_then(|r| r.get("role"))
                                                            .and_then(|v| v.as_str())
                                                            .unwrap_or("guest")
                                                            .to_string(),
                                                    )
                                                };

                                                let mut conns = ctx.connections.write().await;
                                                if let Some(state) = conns.get_mut(&conn_id) {
                                                    state.authenticate(device_id.clone(), permissions, role);
                                                    state.guest_session_id = guest_session_id.clone();
                                                    state.first_message = false;
                                                    // Capture the device-token hash ONLY when this
                                                    // connect authenticated via an existing device
                                                    // token (Case 1: `params.token` present). Every
                                                    // other path (shared-token bootstrap, guest,
                                                    // approved-device issuance) leaves it None and is
                                                    // exempt from the dispatch-time revocation re-check.
                                                    // Guest connections never carry a revocable device
                                                    // token, so skip them explicitly.
                                                    state.device_token_hash = if guest_session_id.is_some()
                                                    {
                                                        None
                                                    } else {
                                                        ctx.token_manager.as_ref().and_then(|tm| {
                                                            req.params
                                                                .as_ref()
                                                                .and_then(|p| p.get("token"))
                                                                .and_then(|t| t.as_str())
                                                                .and_then(|tok| tok.split_once(':'))
                                                                .map(|(token, _sig)| tm.token_hash(token))
                                                        })
                                                    };
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
                                                        role: crate::gateway::presence::ConnectionRole::User,
                                                        connected_at: chrono::Utc::now(),
                                                        last_heartbeat: chrono::Utc::now(),
                                                    };
                                                    ctx.presence.upsert(conn_id.clone(), presence_entry);
                                                    ctx.state_versions.bump_presence();
                                                    let _ = ctx.event_bus.publish_json(&TopicEvent::new("presence.joined", serde_json::json!({"conn_id": &conn_id})).with_state_version(ctx.state_versions.snapshot()));
                                                }
                                            }
                                        }

                                        // Mark first_message = false even if connect failed
                                        // Track failed auth attempts and disconnect if limit reached
                                        let auth_failed = {
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
                                                    true
                                                } else {
                                                    false
                                                }
                                            } else {
                                                false
                                            }
                                        };

                                        // Per-source-IP auth-failure rate limit (loopback exempt).
                                        // The per-connection `auth_attempts` counter above resets
                                        // every time a remote peer reconnects (fresh ConnectionState),
                                        // so on its own it does not bound a reconnect-driven
                                        // brute-force of device tokens. Record failures against the
                                        // `Auth` scope keyed by source IP so a flood of failed
                                        // `connect`s from one remote address is locked out across
                                        // connections. (openclaw #87148)
                                        if auth_failed {
                                            if let Some(retry_after_ms) =
                                                record_auth_failure_lockout(&ctx.rate_limiter, ctx.client_ip)
                                            {
                                                warn!(
                                                    "Connection {} auth-failure rate limited by source IP, disconnecting",
                                                    conn_id
                                                );
                                                let response_str = serde_json::to_string(
                                                    &JsonRpcResponse::error_with_data(
                                                        req.id.clone(),
                                                        RATE_LIMITED,
                                                        "Too many failed authentication attempts",
                                                        serde_json::json!({ "retry_after_ms": retry_after_ms }),
                                                    ),
                                                )
                                                .unwrap_or_default();
                                                let _ = write.send(WsMessage::Text(response_str.into())).await;
                                                break;
                                            }
                                        }

                                        response
                                    }
                                } else {
                                    // No auth required OR already authenticated

                                    // --- Rate limit check ---
                                    // Loopback exemption is based on network origin
                                    // (the resolved client IP, so a reverse proxy
                                    // running on loopback never exempts the remote
                                    // clients behind it), not identity (device_id). For
                                    // authenticated connections the rl_identity is the
                                    // device_id which never looks like a loopback IP.
                                    if !ctx.client_ip.is_loopback() {
                                    let peer_ip_str = ctx.client_ip.to_string();
                                    let rl_identity = {
                                        let conns = ctx.connections.read().await;
                                        conns.get(&conn_id)
                                            .and_then(|s| s.device_id.clone())
                                            .unwrap_or_else(|| peer_ip_str.clone())
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
                                    } // end loopback exemption

                                    // --- Method-level authorization gate ---
                                    // Authentication is binary at the transport
                                    // layer, but a conservative set of
                                    // administrative / secret-bearing
                                    // control-plane methods additionally require
                                    // operator role. Guest (and future node-role)
                                    // connections are rejected from those so an
                                    // invitation-scoped session cannot reach
                                    // `config.apply`, `daemon.shutdown`,
                                    // `secret.*`, etc. Only enforced when auth is
                                    // required — a no-auth local daemon stays
                                    // fully open (existing contract).
                                    if ctx.auth_mode.is_auth_required()
                                        && crate::gateway::method_authz::required_privilege(&req.method)
                                            == crate::gateway::method_authz::MethodPrivilege::Operator
                                    {
                                        let is_operator = {
                                            let conns = ctx.connections.read().await;
                                            conns.get(&conn_id).is_some_and(|s| s.is_operator())
                                        };
                                        if !is_operator {
                                            warn!(
                                                "Connection {} (non-operator) denied operator-only method '{}'",
                                                conn_id, req.method
                                            );
                                            let resp_str = serde_json::to_string(&JsonRpcResponse::error(
                                                req.id.clone(),
                                                PERMISSION_DENIED,
                                                "Operator privileges required for this method",
                                            ))
                                            .unwrap_or_default();
                                            if let Err(e) = write.send(WsMessage::Text(resp_str.into())).await {
                                                error!("Failed to send authz-denied response to {}: {}", conn_id, e);
                                                break;
                                            }
                                            continue;
                                        }
                                    }

                                    // Originating-connection role for the
                                    // config-tier tool gate. ONLY in auth mode:
                                    // a no-auth local daemon leaves this None so
                                    // every run is trusted (zero regression —
                                    // B1's role fallback can be "guest" even
                                    // locally, which must NOT gate config here).
                                    let caller_role: Option<String> =
                                        if ctx.auth_mode.is_auth_required() {
                                            let conns = ctx.connections.read().await;
                                            conns.get(&conn_id).and_then(|s| s.role.clone())
                                        } else {
                                            None
                                        };

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
                                        let do_lane_dispatch = |text: String, lm: Arc<LaneManager>, mc: MiddlewareChain, method: String, req_id: Option<serde_json::Value>, class: ChannelClass, caller_role: Option<String>| async move {
                                            let lane_result = lm.acquire(&method, class).await;
                                            match lane_result {
                                                Ok(_permit) => crate::gateway::caller_identity::CALLER_ROLE
                                                    .scope(caller_role, process_request(&text, &mc))
                                                    .await,
                                                Err(_) => serde_json::to_string(&JsonRpcResponse::error(
                                                    req_id,
                                                    INTERNAL_ERROR,
                                                    "Service congested, try again later",
                                                )).unwrap_or_default()
                                            }
                                        };

                                        // Check idempotency guard (only for non-Query lanes with a key)
                                        let response = if let Some(ref key) = idempotency_key {
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
                                                                    .scope(caller_role.clone(), process_request(&text, &ctx.middleware_chain))
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
                                                do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone()).await
                                            }
                                        } else {
                                            // No idempotency key — standard lane dispatch
                                            do_lane_dispatch(text.to_string(), ctx.lane_manager.clone(), ctx.middleware_chain.clone(), req.method.clone(), req.id.clone(), ctx.channel_class, caller_role.clone()).await
                                        };
                                        // --- End idempotency + lane block ---

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
            event = client_event_rx.recv() => {
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

                            // Extract payload data for field-level filter predicates.
                            // TopicEvent shape stores it at .data; JSON-RPC notifications
                            // store it at .params (then nested .data for our wrapper).
                            let event_data = event_obj
                                .get("data")
                                .or_else(|| event_obj.get("params").and_then(|p| p.get("data")));

                            scope_allowed && ctx.subscription_manager.should_receive(&conn_id, topic, event_data).await
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
                            state_version: None,
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
                format!("Parse error: {}", e),
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

/// Pairing-wizard bootstrap bypass for the WS auth gate.
///
/// Returns `true` when an unauthenticated request should be allowed to reach
/// the handler pipeline because it is part of the same-machine pairing
/// handshake. The bypass is intentionally narrow:
///   * peer must be loopback (rejects all LAN/WAN callers)
///   * method must be a `wizard.*` RPC (the only surface that can mint a token
///     without prior auth, and only after `PairingFlow::confirm_pairing` runs)
fn allow_unauth_loopback_wizard(peer: &SocketAddr, method: &str) -> bool {
    peer.ip().is_loopback() && method.starts_with("wizard.")
}

/// Cold-browser pairing bypass for the WS auth gate.
///
/// Returns `true` for the two anonymous methods used by the `/pair` HTML
/// page (`pairing.start_browser`, `pairing.poll`). Unlike the wizard
/// bypass, this one is NOT loopback-gated — a remote LAN browser (mobile,
/// other laptop) hitting `/pair` is the primary use case. The security
/// boundary is the operator's 1-click approve from the already-
/// authenticated Panel; rate limiting is the existing
/// `PairingManager::MAX_PENDING_REQUESTS = 10` cap on pending pairing
/// requests in the DB.
fn allow_unauth_browser_pairing(method: &str) -> bool {
    matches!(method, "pairing.start_browser" | "pairing.poll")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(ip: &str) -> SocketAddr {
        format!("{ip}:0").parse().unwrap()
    }

    #[test]
    fn auth_failure_lockout_trips_after_budget_and_exempts_loopback() {
        use crate::gateway::rate_limiter::{RateLimitConfig, WindowConfig};

        let cfg = RateLimitConfig {
            auth: WindowConfig {
                max_requests: 2,
                window_secs: 60,
                lockout_secs: Some(300),
            },
            ..RateLimitConfig::default()
        };
        let rl = RateLimiter::new(cfg);
        let ip = sa("203.0.113.7").ip(); // non-loopback (TEST-NET-3)

        // Within budget: first two failures pass.
        assert!(record_auth_failure_lockout(&rl, ip).is_none());
        assert!(record_auth_failure_lockout(&rl, ip).is_none());
        // Budget exhausted: subsequent failures are locked out.
        assert!(record_auth_failure_lockout(&rl, ip).is_some());

        // Loopback is always exempt regardless of prior failures.
        let lo = sa("127.0.0.1").ip();
        for _ in 0..5 {
            assert!(record_auth_failure_lockout(&rl, lo).is_none());
        }
    }

    #[test]
    fn auth_failure_lockout_is_per_source_ip() {
        use crate::gateway::rate_limiter::{RateLimitConfig, WindowConfig};

        let cfg = RateLimitConfig {
            auth: WindowConfig {
                max_requests: 1,
                window_secs: 60,
                lockout_secs: Some(300),
            },
            ..RateLimitConfig::default()
        };
        let rl = RateLimiter::new(cfg);
        let a = sa("198.51.100.1").ip();
        let b = sa("198.51.100.2").ip();

        assert!(record_auth_failure_lockout(&rl, a).is_none()); // a: 1 ok
        assert!(record_auth_failure_lockout(&rl, a).is_some()); // a: locked
                                                                // A different source IP has its own independent budget.
        assert!(record_auth_failure_lockout(&rl, b).is_none()); // b: 1 ok
    }

    #[test]
    fn bypass_allows_wizard_on_ipv4_loopback() {
        assert!(allow_unauth_loopback_wizard(
            &sa("127.0.0.1"),
            "wizard.start"
        ));
        assert!(allow_unauth_loopback_wizard(
            &sa("127.0.0.1"),
            "wizard.answer"
        ));
        assert!(allow_unauth_loopback_wizard(
            &sa("127.0.0.1"),
            "wizard.cancel"
        ));
    }

    #[test]
    fn bypass_allows_wizard_on_ipv6_loopback() {
        assert!(allow_unauth_loopback_wizard(&sa("[::1]"), "wizard.next"));
    }

    #[test]
    fn bypass_rejects_wizard_on_lan_address() {
        assert!(!allow_unauth_loopback_wizard(
            &sa("192.168.1.5"),
            "wizard.start"
        ));
        assert!(!allow_unauth_loopback_wizard(
            &sa("10.0.0.7"),
            "wizard.start"
        ));
    }

    #[test]
    fn bypass_rejects_non_wizard_methods_on_loopback() {
        assert!(!allow_unauth_loopback_wizard(
            &sa("127.0.0.1"),
            "memory.search"
        ));
        assert!(!allow_unauth_loopback_wizard(&sa("127.0.0.1"), "connect"));
        assert!(!allow_unauth_loopback_wizard(
            &sa("127.0.0.1"),
            "agents.list"
        ));
    }

    #[test]
    fn bypass_requires_dot_separator_in_method() {
        // "wizardx.foo" must not match — guards against accidental prefix
        // collisions if a future method were named "wizardry" etc.
        assert!(!allow_unauth_loopback_wizard(
            &sa("127.0.0.1"),
            "wizardx.foo"
        ));
    }

    #[test]
    fn browser_pairing_bypass_admits_only_two_methods() {
        assert!(allow_unauth_browser_pairing("pairing.start_browser"));
        assert!(allow_unauth_browser_pairing("pairing.poll"));
        // Everything else — including the existing authenticated pairing
        // methods — stays gated.
        assert!(!allow_unauth_browser_pairing("pairing.approve"));
        assert!(!allow_unauth_browser_pairing("pairing.reject"));
        assert!(!allow_unauth_browser_pairing("pairing.list"));
        assert!(!allow_unauth_browser_pairing("memory.search"));
        assert!(!allow_unauth_browser_pairing(""));
    }

    fn headers_with_cookie(raw: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, raw.parse().unwrap());
        h
    }

    #[test]
    fn extract_session_cookie_picks_aleph_session_value() {
        let h = headers_with_cookie("foo=bar; aleph_session=abc-123; baz=qux");
        assert_eq!(extract_session_cookie(&h), Some("abc-123".to_string()));
    }

    #[test]
    fn extract_session_cookie_handles_no_aleph_session() {
        let h = headers_with_cookie("foo=bar; other=val");
        assert_eq!(extract_session_cookie(&h), None);
    }

    #[test]
    fn extract_session_cookie_is_none_when_header_missing() {
        let h = HeaderMap::new();
        assert_eq!(extract_session_cookie(&h), None);
    }

    fn parse_req(text: &str) -> JsonRpcRequest {
        serde_json::from_str(text).expect("text must be a valid JsonRpcRequest")
    }

    #[test]
    fn inject_skips_when_no_bootstrap_token() {
        let text =
            r#"{"jsonrpc":"2.0","id":1,"method":"connect","params":{"device_name":"Web Panel"}}"#;
        let req = parse_req(text);
        let out = maybe_inject_bootstrap_shared_token(text, &req, None);
        assert_eq!(out, text);
    }

    #[test]
    fn inject_skips_non_connect_methods() {
        // `connect.challenge` is in the connect family but never carries
        // credentials. We must not inject into it.
        let text =
            r#"{"jsonrpc":"2.0","id":1,"method":"connect.challenge","params":{"device_id":"d"}}"#;
        let req = parse_req(text);
        let out = maybe_inject_bootstrap_shared_token(text, &req, Some("tok-XYZ"));
        assert_eq!(out, text);
    }

    #[test]
    fn inject_skips_when_client_already_carries_token() {
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"connect","params":{"token":"existing:sig","device_name":"Web Panel"}}"#;
        let req = parse_req(text);
        let out = maybe_inject_bootstrap_shared_token(text, &req, Some("tok-XYZ"));
        assert_eq!(out, text);
    }

    #[test]
    fn inject_skips_when_client_already_carries_shared_token() {
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"connect","params":{"shared_token":"client-supplied","device_name":"Web Panel"}}"#;
        let req = parse_req(text);
        let out = maybe_inject_bootstrap_shared_token(text, &req, Some("tok-XYZ"));
        assert_eq!(out, text);
    }

    #[test]
    fn inject_skips_when_client_already_carries_invitation_token() {
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"connect","params":{"invitation_token":"guest-tok","device_name":"Web Panel"}}"#;
        let req = parse_req(text);
        let out = maybe_inject_bootstrap_shared_token(text, &req, Some("tok-XYZ"));
        assert_eq!(out, text);
    }

    #[test]
    fn inject_adds_shared_token_to_anonymous_connect() {
        let text =
            r#"{"jsonrpc":"2.0","id":1,"method":"connect","params":{"device_name":"Web Panel"}}"#;
        let req = parse_req(text);
        let out = maybe_inject_bootstrap_shared_token(text, &req, Some("tok-XYZ"));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["params"]["shared_token"].as_str(),
            Some("tok-XYZ"),
            "shared_token must be injected verbatim"
        );
        assert_eq!(
            v["params"]["device_name"].as_str(),
            Some("Web Panel"),
            "existing params fields must be preserved"
        );
        assert_eq!(v["method"].as_str(), Some("connect"));
        assert_eq!(v["id"].as_i64(), Some(1));
    }

    #[test]
    fn inject_creates_params_when_missing() {
        // A `connect` with no params at all should still be upgraded.
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"connect"}"#;
        let req = parse_req(text);
        let out = maybe_inject_bootstrap_shared_token(text, &req, Some("tok-XYZ"));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["params"]["shared_token"].as_str(), Some("tok-XYZ"));
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
