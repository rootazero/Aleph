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
    JsonRpcRequest, JsonRpcResponse, ADMIN_REQUIRED_MESSAGE, AUTH_REQUIRED,
    IDEMPOTENCY_KEY_REQUIRED, INTERNAL_ERROR, PARSE_ERROR, RATE_LIMITED,
};
use crate::gateway::rate_limiter::{
    scope_for_method, RateLimitError, RateLimitKey, RateLimitScope, RateLimiter,
};
use crate::gateway::state_version::StateVersionTracker;

use super::per_client_buffer::PerClientBuffer;
use super::{ConnectionState, GatewaySharedState};
use crate::gateway::security::SecurityStore;

/// Parse configured trusted-proxy IP strings into `IpAddr`, dropping
/// unparseable entries (fail-safe: a garbage entry just isn't trusted).
///
/// The drop is deliberate; the *silence* was not. Only a bare `IpAddr` parses
/// here — there is no CIDR support in this tree — so an operator who writes
/// `trusted_ips = ["10.0.0.0/24"]` gets an EMPTY trusted set, and an empty set
/// is byte-for-byte the same downstream state as "no reverse proxy is
/// configured": every `X-Forwarded-For` is ignored, and every client behind
/// that proxy collapses onto the proxy's own address for the per-IP cap and
/// the rate limiter. A config value that silently resolves to "not trusted"
/// has no symptom at all, so it says so instead.
pub(super) fn parse_trusted_ips(raw: &[String]) -> Vec<IpAddr> {
    raw.iter()
        .filter_map(|s| match s.parse::<IpAddr>() {
            Ok(ip) => Some(ip),
            Err(e) => {
                warn!(
                    entry = %s,
                    "[gateway] trusted_ips entry is not a bare IP address and was dropped \
                     ({e}); this list takes single addresses only (no CIDR ranges), so \
                     nothing behind that proxy is trusted and its X-Forwarded-For headers \
                     are ignored"
                );
                None
            }
        })
        .collect()
}

/// Whether to refuse this upgrade for insecure transport. A non-local client
/// on an unencrypted leg is refused unless the operator set
/// `allow_insecure_remote`. A genuinely local client is always allowed.
///
/// Takes `client_is_local` ([`crate::gateway::trusted_proxy::ResolvedClient::local`])
/// rather than the resolved IP: behind a same-host reverse proxy the resolved
/// IP can fall back to the proxy's own loopback address when no
/// `X-Forwarded-For` arrives, and "I could not determine the real client" must
/// not read as "the client is local".
pub(super) fn refuse_insecure_remote(
    client_is_local: bool,
    secure: bool,
    allow_insecure_remote: bool,
) -> bool {
    !client_is_local && !secure && !allow_insecure_remote
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

/// axum handler: upgrade HTTP connection to WebSocket at `/ws`
pub(super) async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<GatewaySharedState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    // IP-keyed abuse protections (per-IP cap, rate limiting), the security
    // audit log read `client_ip`; every AUTHORITY decision — the per-IP cap, the
    // insecure-transport gate, the connect-auth loopback grant, the initial
    // `caller_role`, the rate-limit exemption and `SurfaceKind` — reads
    // `client_is_local` instead. The split is the fix: one bit says WHICH BUCKET
    // this connection belongs to, the other says WHETHER IT MAY. Before
    // 2026-08-29 there was only `client_ip`, so a same-host reverse proxy that
    // forgot `X-Forwarded-For` handed every internet client the loopback
    // operator grant.
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
    let client_is_local = resolved.local;
    let secure = state.tls_enabled || resolved.secure;

    // Insecure-transport guard: a non-local client on an unencrypted leg
    // is refused unless the operator opted into `allow_insecure_remote`.
    // A genuinely local client is always allowed. Must run before any
    // auth/origin decision is made over what could be a plaintext,
    // sniffable/tamperable leg.
    if refuse_insecure_remote(client_is_local, secure, state.allow_insecure_remote) {
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
        if per_ip_cap > 0 && !client_is_local {
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
    let channel_class = if client_is_local {
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
            client_is_local,
            node_registry: state.node_registry.clone(),
            exec_approval_manager: state.exec_approval_manager.clone(),
            audit_log: state.audit_log.clone(),
            session_store: state.session_store.clone(),
            team_store: state.team_store.clone(),
            event_visibility: state.event_visibility.clone(),
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
    serde_json::to_string(&aleph_protocol::JsonRpcRequest::notification(
        aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD,
        Some(serde_json::json!({
            "topic": "connection.warning",
            "data": {
                "reason": "events_overflow",
                "dropped": dropped,
                "total_overflow": total_overflow,
                "advice": "reconnect"
            }
        })),
    ))
    .unwrap_or_else(|_| String::new())
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

/// Extract `(topic, data)` from an already-parsed event envelope — the
/// single chokepoint every per-connection filter term reads from
/// (`EventScopeGuard`, `audience_allows`, `SubscriptionManager`,
/// `EventVisibilityIndex`).
///
/// Handles every wire shape an event can arrive in on `ctx.event_bus`:
/// - `TopicEvent` form (non-stream `GatewayEventFrame` variants, published
///   by `publish_frame`): `{"topic": "...", "data": {...}}`.
/// - `stream.*` JSON-RPC notification form (streaming `GatewayEventFrame`
///   variants): `{"method": "stream.X", "params": <frame body>}` — the
///   frame's own fields live directly under `params`, not nested under a
///   `.data` (see `event_bus.rs::publish_frame`'s doc). `data` reads `None`
///   for this shape — unchanged from before this function existed; no
///   `stream.*` frame has ever had a nested `.data` to find.
/// - The double-wrapped `TopicEvent::to_notification()` form, used by
///   producers that build a raw string and call `GatewayEventBus::publish`
///   directly rather than going through `publish_frame` (e.g.
///   `subagent_tree_relay.rs`'s `run.subagent_tree`):
///   `{"jsonrpc":"2.0","method":"event","params":{"topic":"...",
///   "data":{...},"timestamp":...}}`. **Missing this branch reads `topic`
///   as the literal string `"event"` for every producer using this shape**
///   — `EventScopeGuard` happens to default-allow an unrecognized topic
///   anyway, but `session_identity_of` ALSO defaults an unrecognized topic
///   to `Global`, so a session-scoped event published this way silently
///   skipped owner-scoping entirely (found in review, fix round 1 — see
///   `run.subagent_tree`'s entry in `event_visibility::session_identity_of`).
fn extract_topic_and_data(event_obj: &serde_json::Value) -> (&str, Option<&serde_json::Value>) {
    if event_obj.get("method").and_then(serde_json::Value::as_str)
        == Some(aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD)
    {
        if let Some(params) = event_obj.get("params") {
            let topic = params
                .get("topic")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            return (topic, params.get("data"));
        }
    }
    let topic = event_obj
        .get("topic")
        .and_then(serde_json::Value::as_str)
        .or_else(|| event_obj.get("method").and_then(serde_json::Value::as_str))
        .unwrap_or("");
    let data = event_obj
        .get("data")
        .or_else(|| event_obj.get("params").and_then(|p| p.get("data")));
    (topic, data)
}

/// The exact bytes a connection receives for one already-admitted event.
///
/// Two jobs, in this order:
///
/// 1. **Apply the per-connection payload projection**, if
///    [`EventVisibilityIndex::project_for`](crate::gateway::event_visibility::EventVisibilityIndex::project_for)
///    produced one. A projected payload replaces `.params`, because every frame
///    that method has an arm for is published through `publish_frame`'s STREAM
///    branch, which puts the frame body exactly there — the same place
///    [`extract_topic_and_data`]'s caller read it from to make the decision.
/// 2. **Wrap the bare `TopicEvent` form** (`{topic, data}`, no `method`) into a
///    JSON-RPC notification so the Panel can dispatch it via `method == "event"`.
///
/// A frame that is neither projected nor wrapped is forwarded as the ORIGINAL
/// string, not a re-serialization of the parse — byte-identical output and no
/// serialization cost for the overwhelming majority of frames. That is what
/// pays for the projection: this used to re-parse `original` from scratch just
/// to answer question 2.
fn event_wire_form(
    mut event_obj: serde_json::Value,
    projected_payload: Option<serde_json::Value>,
    original: String,
) -> String {
    let rewritten = projected_payload.is_some();
    if let (Some(payload), Some(obj)) = (projected_payload, event_obj.as_object_mut()) {
        obj.insert("params".to_string(), payload);
    }
    if event_obj.get("topic").is_some() && event_obj.get("method").is_none() {
        // The shared constructor, so this envelope carries `"jsonrpc": "2.0"`
        // like every other notification on the wire — see
        // `event_bus.rs::publish_frame` for what a hand-built one cost.
        serde_json::to_string(&aleph_protocol::JsonRpcRequest::notification(
            aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD,
            Some(event_obj),
        ))
        .unwrap_or_else(|_| String::new())
    } else if rewritten {
        event_obj.to_string()
    } else {
        original
    }
}

/// The guest login wall: may a connection stamped with `role` send `method`?
///
/// The wall is the *guest* wall and nothing else — it separates "this
/// connection presented a credential" from "it did not." Both authorized roles
/// pass every method: `"operator"` (loopback, legacy shared token, or a device
/// bound to an `admin` user) and `"member"` (a device bound to a `member`-role
/// user). The admin/member split for server-global methods is decided further
/// in, at the `process_request` chokepoint (`method_admin.rs`) — teaching this
/// predicate about it would put the same decision in two places.
///
/// Anything else — `"guest"`, an unrecognized role string, or absent
/// connection state — may only send `connect` to authorize (fail closed).
///
/// ## The wall has TWO consumers, and it had one for too long
///
/// A connection has two directions, and until 2026-08-08 this predicate was
/// evaluated on only one of them. The request arm consults it before dispatch;
/// the *event-forward* arm's verdict was `scope_allowed && audience_allows &&
/// should_receive && event_admits` — four terms, none of them authentication.
/// Every one of them passes for a connection that has never authorized:
/// `ConnectionState` is inserted into `ctx.connections` the moment the socket is
/// accepted (`permissions: []`, `caller_user: None`, `caller_role: "guest"`),
/// `can_receive` allows any topic no rule names, `should_receive` returns `true`
/// when the connection registered no filter at all, and `event_admits` short-
/// circuits on `SessionIdentity::Global` *before* it reads `caller_user`. A bare
/// remote WebSocket that sent nothing therefore received every `Global` frame —
/// including `pty.screen`, whose RPC face has been in `ADMIN_PREFIXES` all along.
///
/// So: **an authorization predicate belongs on every direction a connection
/// carries data, not on the one where the caller asks a question.** The event
/// arm now evaluates this same function on the same `caller_role` field, which
/// also means `restamp_live_connections` closes both planes at once — a
/// deactivated user's socket stops receiving in the same instant it stops being
/// served.
///
/// Pure so the wall's own logic is host-testable. The
/// `resolve_stamped_identity` tests below cover *what role gets stamped*; the
/// class of bug this function exists to prevent lives in the *predicate* — a
/// correctly-stamped `"member"` being refused every method and then
/// flood-guard-kicked as an abuser stays green under any test that scopes
/// task-locals below the wall.
///
/// A third consumer reads it with `method: ""`:
/// `metrics_endpoint::count_authenticated`. "Does this connection hold
/// authority" must have ONE derivation in the gateway, so the gauge moves when
/// `restamp_live_connections` demotes someone, exactly as the two delivery
/// planes do.
#[must_use]
pub(super) fn wall_admits(role: Option<&str>, method: &str) -> bool {
    matches!(role, Some("operator" | "member")) || method == "connect"
}

/// The authorization verdict echoed back in a `connect` response:
/// `(role, authorized, needs_token)`.
///
/// Derived from the **resolved** identity, never from the raw credential
/// verdict alone. "Was the credential valid" and "does this connection hold
/// any authority" are different questions, and P0 made them come apart: a
/// device token that is still valid but whose bound user was deactivated (or
/// whose `user_id` dangles) is a valid credential that grants nothing. It must
/// be reported with the shape the Panel already knows — `("guest", false,
/// true)`, i.e. the login wall — rather than a new close reason or verdict
/// word no client parses.
///
/// Pure so the exact wire triple is host-testable; the surrounding JSON
/// insertion has no seam (it edits a response inside the live WS loop).
#[must_use]
fn connect_verdict(credential_ok: bool, resolved_role: &str) -> (&str, bool, bool) {
    let holds_authority = credential_ok && resolved_role != "guest";
    (resolved_role, holds_authority, !holds_authority)
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
/// though its token was valid.
///
/// With **no store wired** (probe/test server) the arm splits on whether the
/// connection is device-bound. No device and no store is the pre-P0 shape
/// (loopback / legacy shared token) and keeps resolving to the implicit owner
/// as operator — unchanged from before per-user resolution existed. A device
/// *is* presented but there is no store to resolve it against ⇒ `("guest",
/// None)`, fail-closed, mirroring the ruled `Err` semantics inside
/// `resolve_connection_identity`: a binding lookup that could not be
/// performed must never be read as "unbound, therefore owner" — that is the
/// one input a remote caller controls, and it would otherwise buy full
/// operator authority on any deployment whose store failed to wire.
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
        // Device-bound but unresolvable: fail closed (see doc above).
        None if device_id.is_some() => (None, "guest"),
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
    //
    // ⚠️ KNOWN GAP (2026-08-29) — that guard is false in a SECOND case nobody
    // wrote down, and this publish is `node.disconnected`'s only producer
    // repo-wide. `NodeRegistry::forget` (the operator-deregister path, reached
    // from `cluster.deregister` and the `node_manage` tool) empties BOTH
    // `nodes_by_id` and `nodes_by_conn` before it calls `close_connection()`,
    // so by the time this cleanup runs `node_identity_by_conn` is already
    // `None` and `deregister` already returns false — i.e. an explicit
    // deregister emits nothing and skips the `touch_device` last-seen stamp.
    // A second operator's Panel keeps rendering the node "online" until a full
    // page reload, after which it vanishes entirely, so the transition is
    // never observable as an event. `registry.rs`'s own test comments that
    // "a stale conn cleanup after forget is a harmless no-op" — harmless only
    // if you do not know the event lives inside the guard.
    //
    // This arm is NOT the place to fix it: after `forget` there is nothing
    // here left to read. The publish belongs in `cluster::deregister_node`
    // (already the single shared source for the RPC and tool faces), fired
    // when it evicts a live session, with `forget` returning the evicted
    // session's `device_name` so no second lookup is needed. Leave this arm
    // as-is for the wedge/ordinary-drop path — its guard then correctly
    // suppresses the duplicate.
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
            if let Err(e) = ctx.event_bus.publish_json(&TopicEvent::new(
                "node.disconnected",
                serde_json::json!({"node_id": node_id, "name": name, "conn_id": &conn_id}),
            )) {
                tracing::warn!(
                    error = %e,
                    node_id = %node_id,
                    conn_id = %conn_id,
                    "failed to publish node.disconnected event"
                );
            }
        }
    }

    // Remove presence and emit departure event
    if let Some(_entry) = ctx.presence.remove(&conn_id) {
        ctx.state_versions.bump_presence();
        if let Err(e) = ctx.event_bus.publish_json(
            &TopicEvent::new("presence.left", serde_json::json!({"conn_id": &conn_id}))
                .with_state_version(ctx.state_versions.snapshot()),
        ) {
            tracing::warn!(
                error = %e,
                conn_id = %conn_id,
                "failed to publish presence.left event"
            );
        }
    }

    // Remove subscriptions for this connection
    ctx.subscription_manager.remove_connection(&conn_id).await;

    // Release any PTY viewport constraints this connection held, so a
    // crashed/closed tab does not permanently pin a shared terminal's size
    // (`caller_identity::CALLER_CONN_ID` / `PtyManager::note_viewport`). This
    // is the sixth per-connection subsystem cleaned up here, alongside conns
    // / reverse-RPC / node registry / presence / subscriptions above.
    crate::gateway::pty::manager().release_conn(&conn_id);

    info!("Connection closed: {}", conn_id);
    Ok(())
}

/// Scope `process_request` with the caller-identity task-locals + P1 scope
/// attribution that must surround every dispatched request. Single source of
/// truth shared by both dispatch stations (`do_lane_dispatch`'s closure and
/// the idempotency `Proceed` arm) so the two call sites cannot drift apart —
/// see `src/gateway/CLAUDE.md`'s note that `CALLER_ROLE`/`CALLER_USER`/
/// `CALLER_IS_LOOPBACK`/`CALLER_CONN_ID` must be scoped around
/// `process_request` at both sites. `scope::with_scope` is the outermost
/// (4th) layer: a `caller_user` seeds a personal-scope attribution,
/// observable via `scope::current_scope` for the lifetime of this dispatch
/// (spec P1 §5).
async fn dispatch_with_caller_context(
    text: &str,
    mc: &MiddlewareChain,
    caller_role: Option<String>,
    caller_user: Option<String>,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
) -> String {
    crate::scope::with_scope(
        caller_user
            .clone()
            .map(|u| crate::scope::ScopeAttribution::personal(&u)),
        crate::gateway::caller_identity::CALLER_USER.scope(
            caller_user,
            crate::gateway::caller_identity::CALLER_ROLE.scope(
                caller_role,
                crate::gateway::caller_identity::CALLER_IS_LOOPBACK.scope(
                    caller_is_loopback,
                    crate::gateway::caller_identity::CALLER_CONN_ID
                        .scope(caller_conn_id, process_request(text, mc)),
                ),
            ),
        ),
    )
    .await
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

    // Multi-user role gate (spec §4.6): members cannot reach server-global
    // config/credential methods. One chokepoint covers both dispatch paths —
    // CALLER_ROLE is scoped around process_request at both call sites
    // (`do_lane_dispatch` and the idempotency `Proceed` arm). `None`
    // (internal/cron) and `"operator"` pass; `"guest"` never reaches here for
    // non-connect methods (the login wall above refuses it first).
    if crate::gateway::method_admin::method_requires_admin(&request.method)
        && crate::gateway::caller_identity::caller_is_member()
    {
        return serde_json::to_string(&JsonRpcResponse::error(
            request.id.clone(),
            AUTH_REQUIRED,
            // Shared with the Panel through `aleph_protocol` — the wording is
            // not local to this arm, because the cluster page keys its role
            // explanation off these exact words.
            ADMIN_REQUIRED_MESSAGE.to_string(),
        ))
        .unwrap_or_default();
    }

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
                user_id: None,
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
    fn no_store_and_no_device_falls_back_to_owner_when_authorized() {
        // probe/test server with no security store, and a connection that is
        // not device-bound (loopback / legacy shared token): the pre-P0 shape,
        // LAN-trust degrade preserved.
        let (user, role) = resolve_stamped_identity(true, false, None, None);
        assert_eq!(
            user.as_deref(),
            Some(crate::gateway::security::store::OWNER_USER_ID)
        );
        assert_eq!(role, "operator");
    }

    #[test]
    fn no_store_but_device_bound_fails_closed_to_guest() {
        // The other half of the same arm: a device id WAS presented but there
        // is no store to resolve its binding against. "Could not look it up"
        // must not read as "unbound, therefore owner" — the device id is
        // remote-controlled input, so fail closed exactly like the store-`Err`
        // arm inside `resolve_connection_identity`.
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-x"), None);
        assert_eq!(user, None);
        assert_eq!(role, "guest");
    }

    // ── The connect response's echoed verdict (connect_verdict) ──────────
    // Composed with `resolve_stamped_identity` so each case runs the real
    // chain the handshake runs: credential verdict + store → resolved role →
    // wire triple.

    #[test]
    fn connect_response_reports_member_authority_to_a_member() {
        // The regression: the response used to be computed from the
        // credential verdict alone, so a member was told `role: "operator"`
        // and rendered an operator UI whose every admin surface then failed.
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let (_, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        assert_eq!(connect_verdict(true, role), ("member", true, false));
    }

    #[test]
    fn connect_response_walls_a_deactivated_users_valid_device() {
        // Valid credential, no principal. Reported with the EXISTING walled
        // shape — no new vocabulary, no new close reason.
        use crate::gateway::security::store::UserStatus;
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        store
            .update_user("u-alice", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let (user, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        assert_eq!(user, None);
        assert_eq!(
            connect_verdict(true, role),
            ("guest", false, true),
            "a credential that grants nothing must not claim operator authority"
        );
    }

    #[test]
    fn connect_response_is_unchanged_for_operator_and_walled_connections() {
        // Zero-change guarantee: every pre-P0 shape produces exactly the
        // triple the old credential-only overlay produced.
        // Loopback / legacy shared token (no store, no device).
        let (_, lo_role) = resolve_stamped_identity(true, true, None, None);
        assert_eq!(connect_verdict(true, lo_role), ("operator", true, false));
        // Remote, admin-bound device.
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (_, adm_role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(connect_verdict(true, adm_role), ("operator", true, false));
        // Rejected credential.
        let (_, guest_role) = resolve_stamped_identity(false, false, None, Some(&store));
        assert_eq!(connect_verdict(false, guest_role), ("guest", false, true));
    }

    // ── The event scope stamped alongside that verdict ────────────────────
    // The stamping itself has no seam (it edits ConnectionState inside the
    // live WS loop), but both its inputs are pure, so the composition the
    // handshake actually evaluates is testable: resolved role → scope.

    #[test]
    fn connect_stamps_a_member_out_of_the_admin_event_scope() {
        // The finding: a member holds authority (the login wall admits him),
        // and the stamping used to key on that, handing him `"*"` — which
        // short-circuits every EventScopeGuard rule. So a member's socket was
        // delivered exec approval cards including the command text.
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let (_, role) = resolve_stamped_identity(true, false, Some("dev-a"), Some(&store));
        let scope = crate::gateway::event_scope::scope_for_role(role);
        assert!(scope.is_empty(), "a member must not be stamped `*`");

        let guard = crate::gateway::event_scope::EventScopeGuard::default_rules();
        // The raw approval CARDS are no longer this guard's business (they are
        // owner-scoped per frame since 2026-08-08, so a member gets their own
        // and no one else's). What a member must still not hold is the
        // superuser scope, which is what would hand them everybody's.
        // ...and that same predicate is now what decides the R5 BANNER too: it
        // left this table on 2026-08-09 once it began carrying the session key
        // it is derived from, so `can_receive("surface.approval", …)` no longer
        // answers anything. `is_superuser_scope` above IS the admin arm of the
        // banner's owner check — the assertion did not go away, it merged into
        // the line before this comment.
        assert!(!crate::gateway::event_scope::is_superuser_scope(&scope));
        assert!(guard.can_receive("surface.approval", &scope));
        assert!(!guard.can_receive("config.changed", &scope));
        assert!(!guard.can_receive("pairing.requested", &scope));
        assert!(!guard.can_receive("pty.screen", &scope));
        // `approval.requested` deliberately passes THIS table since 2026-08-08
        // — a member must be able to answer the gate blocking their own run.
        // The per-session decision is made in `event_visibility`, pinned there.
        assert!(guard.can_receive("approval.requested", &scope));
        // ...while his daily surfaces are untouched (default-allow guard).
        assert!(guard.can_receive("agent.run.started", &scope));
        assert!(guard.can_receive("chat.message", &scope));
    }

    #[test]
    fn connect_stamps_operator_and_walled_scopes_unchanged() {
        // Zero-change guarantee on the scope axis, mirroring
        // `connect_response_is_unchanged_for_operator_and_walled_connections`.
        let star = vec!["*".to_string()];
        // Loopback / legacy shared token.
        let (_, lo_role) = resolve_stamped_identity(true, true, None, None);
        assert_eq!(crate::gateway::event_scope::scope_for_role(lo_role), star);
        // Remote, admin-bound device.
        let store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let (_, adm_role) = resolve_stamped_identity(true, false, Some("dev-r"), Some(&store));
        assert_eq!(crate::gateway::event_scope::scope_for_role(adm_role), star);
        // Rejected credential ⇒ walled, no scope (as before).
        let (_, guest_role) = resolve_stamped_identity(false, false, None, Some(&store));
        assert!(crate::gateway::event_scope::scope_for_role(guest_role).is_empty());
    }

    #[test]
    fn scope_keyed_on_role_matches_holds_authority_except_for_members() {
        // The stamping switched from `holds_authority` to the resolved role.
        // That is safe because `resolve_stamped_identity` returns "guest"
        // whenever `!authorized`, so `holds_authority == (role != "guest")`
        // for every shape — the ONE divergence is the member, which is the
        // fix. Pinned here so a future change to either function that breaks
        // the equivalence is loud rather than a silent scope widening.
        let store = store_with_device_user(
            "dev-a",
            "u-alice",
            crate::gateway::security::store::UserRole::Member,
        );
        let admin_store = store_with_device_user(
            "dev-r",
            "u-root",
            crate::gateway::security::store::UserRole::Admin,
        );
        let cases = [
            // (authorized, loopback, device, store)
            (true, true, None, None),
            (true, false, None, None),
            (false, false, None, None),
            (true, false, Some("dev-x"), None),
            (true, false, Some("dev-a"), Some(&store)),
            (false, false, Some("dev-a"), Some(&store)),
            (true, false, Some("dev-r"), Some(&admin_store)),
        ];
        for (authorized, loopback, device, st) in cases {
            let (_, role) = resolve_stamped_identity(authorized, loopback, device, st);
            let (_, holds_authority, _) = connect_verdict(authorized, role);
            let scope = crate::gateway::event_scope::scope_for_role(role);
            if role == "member" {
                assert!(
                    holds_authority && scope.is_empty(),
                    "a member holds authority yet must hold no event scope"
                );
            } else {
                assert_eq!(
                    holds_authority,
                    !scope.is_empty(),
                    "non-member scope must still track holds_authority \
                     (authorized={authorized}, loopback={loopback}, role={role})"
                );
            }
        }
    }

    // ── The login wall's own predicate (wall_admits) ──────────────────────
    // These drive the ACTUAL wall expression the dispatch loop evaluates.
    // The `resolve_stamped_identity` tests above prove a member connection is
    // *stamped* "member"; only these prove the wall then lets it through —
    // the distinction is not academic, it is precisely how "member is refused
    // every method and then flood-kicked as an abuser" stayed green.

    #[test]
    fn wall_admits_member_on_a_daily_method() {
        assert!(
            wall_admits(Some("member"), "chat.send"),
            "a member connection must clear the guest wall; the admin/member \
             split is method_admin.rs's job, deeper in"
        );
        assert!(wall_admits(Some("member"), "sessions.list"));
        assert!(wall_admits(Some("member"), "connect"));
    }

    /// The event arm evaluates the wall with `method: ""` — there is no
    /// `connect` exemption to grant on a frame nobody asked for. Both halves
    /// matter and the POSITIVE one is load-bearing: gating the delivery plane
    /// fails *silently* (a withheld frame produces no error to any client), so
    /// a wrong role assumption would dark a real surface with no symptom. The
    /// two authorized roles must still receive.
    #[test]
    fn the_event_arm_wall_refuses_a_guest_and_still_serves_both_authorized_roles() {
        // The state a socket carries before it has sent anything at all:
        // `ConnectionState::new` stamps `caller_role: "guest"`.
        assert!(
            !wall_admits(Some("guest"), ""),
            "an unauthorized socket must receive no event frame; `pty.screen` \
             is Global-classified and carries the operator's live terminal \
             content"
        );
        // …and the same is true for a role word nobody stamps.
        assert!(!wall_admits(Some("bogus"), ""));
        assert!(!wall_admits(None, ""));

        // The half that would go silently dark if the predicate were wrong.
        // A cluster node resolves through `resolve_connection_identity`'s
        // unbound-device arm to `("u-owner", "operator")`, so nodes are on
        // this side of the wall too.
        assert!(
            wall_admits(Some("operator"), ""),
            "operator connections — Panel, CLI and cluster nodes alike — must \
             still receive events"
        );
        assert!(
            wall_admits(Some("member"), ""),
            "a member's own stream.* frames must still arrive; this wall is \
             the GUEST wall, and the per-user filter is event_visibility's job"
        );
    }

    #[test]
    fn wall_admits_operator_on_everything() {
        assert!(wall_admits(Some("operator"), "chat.send"));
        assert!(wall_admits(Some("operator"), "connect"));
        assert!(wall_admits(Some("operator"), "config.patch"));
    }

    #[test]
    fn wall_refuses_guest_except_connect() {
        assert!(
            !wall_admits(Some("guest"), "chat.send"),
            "the wall must stay the guest wall"
        );
        assert!(
            wall_admits(Some("guest"), "connect"),
            "connect is the only way to authorize, so it is always admitted"
        );
    }

    #[test]
    fn wall_fails_closed_on_absent_or_unknown_roles() {
        // Pre-handshake / vanished connection state, and any role string the
        // wall does not know, are refused everything but `connect`.
        assert!(!wall_admits(None, "chat.send"));
        assert!(wall_admits(None, "connect"));
        assert!(!wall_admits(Some("admin"), "chat.send")); // wire word is "operator"
        assert!(!wall_admits(Some(""), "chat.send"));
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
        const LOCAL: bool = true;
        const REMOTE: bool = false;

        // A genuinely local client is always allowed, secure or not.
        assert!(!super::refuse_insecure_remote(LOCAL, false, false));
        assert!(!super::refuse_insecure_remote(LOCAL, false, true));

        // Remote + insecure + not allowed ⇒ refuse.
        assert!(super::refuse_insecure_remote(REMOTE, false, false));
        // Remote + secure ⇒ allow.
        assert!(!super::refuse_insecure_remote(REMOTE, true, false));
        // Remote + insecure + explicitly allowed ⇒ allow.
        assert!(!super::refuse_insecure_remote(REMOTE, false, true));
    }

    /// The gate must key on the trusted-proxy-aware `local` bit, not on
    /// `ip.is_loopback()`. A same-host proxy that forwards without
    /// `X-Forwarded-For` resolves `ip` back to its own loopback address; if
    /// the gate read that, an internet client on a plaintext leg would be
    /// admitted as "loopback" and then go on to collect every other loopback
    /// privilege in this file.
    #[test]
    fn the_insecure_transport_gate_reads_the_local_bit_not_the_resolved_ip() {
        use crate::gateway::trusted_proxy::resolve_client;
        use axum::http::HeaderMap;
        use std::net::IpAddr;

        let proxy: IpAddr = "127.0.0.1".parse().unwrap();
        let resolved = resolve_client(proxy, &HeaderMap::new(), true, &[proxy]);
        assert!(
            resolved.ip.is_loopback(),
            "precondition: with no XFF the resolved IP really is loopback — \
             that is what made reading it wrong"
        );
        assert!(super::refuse_insecure_remote(resolved.local, false, false));
    }

    // ── P1 scope attribution around dispatch (dispatch_with_caller_context) ──
    // `dispatch_with_caller_context` is the single function BOTH dispatch
    // stations call (`do_lane_dispatch`'s closure and the idempotency
    // `Proceed` arm — see the call sites above). Exercising it once proves
    // both stations by construction: neither wraps `process_request` any
    // other way, so there is no second code path to drift out of sync. This
    // mirrors how `resolve_stamped_identity`/`connect_verdict` are tested
    // above rather than the live WS loop itself — that loop has no
    // injectable seam (it operates on a real `axum::extract::ws::WebSocket`).

    #[tokio::test]
    async fn both_dispatch_stations_seed_scope() {
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;

        // A probe method that reports what `scope::current_scope()` sees
        // from inside `process_request`'s dispatch.
        let mut registry = HandlerRegistry::new();
        registry.register("probe.scope", |req| async move {
            let owner = crate::scope::current_scope().map(|attr| attr.owner_user_id);
            JsonRpcResponse::success(req.id, serde_json::json!({ "owner_user_id": owner }))
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"probe.scope","params":{}}"#;

        let resp = dispatch_with_caller_context(
            text,
            &mc,
            Some("member".to_string()),
            Some("u-alice".to_string()),
            false,
            None,
        )
        .await;
        assert!(
            resp.contains("\"owner_user_id\":\"u-alice\""),
            "scope must be observable inside process_request's dispatch: {resp}"
        );
    }

    /// `cron.create` reached the way a Panel or CLI caller reaches it — a real
    /// dispatch, not a hand-seeded scope — must leave the caller's identity on
    /// the row that lands in the store.
    ///
    /// The effect asserted is the PERSISTED job, re-read from a freshly loaded
    /// `CronStore`, not the RPC's own response: the response used to be a
    /// perfectly successful `{"job": …}` while both columns were NULL.
    #[tokio::test]
    async fn cron_create_through_dispatch_persists_the_caller_as_owner() {
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;
        use crate::tasks::cron::store::CronStore;
        use crate::tasks::cron::{CronConfig, CronService};

        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("cron.db");
        let service = CronService::new(CronConfig {
            db_path: db_path.to_string_lossy().to_string(),
            ..CronConfig::default()
        })
        .unwrap();
        let cron = Arc::new(tokio::sync::Mutex::new(service));

        let mut registry = HandlerRegistry::new();
        let handler_cron = cron.clone();
        registry.register("cron.create", move |req| {
            let cron = handler_cron.clone();
            async move { crate::gateway::handlers::cron::handle_create(req, cron).await }
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"cron.create","params":{
            "name":"nightly-digest","agent_id":"main","prompt":"digest",
            "schedule_kind":{"kind":"every","every_ms":60000}}}"#;

        let resp = dispatch_with_caller_context(
            text,
            &mc,
            Some("operator".to_string()),
            Some("u-x".to_string()),
            true,
            None,
        )
        .await;
        assert!(
            !resp.contains("\"error\""),
            "precondition: the create itself must succeed: {resp}"
        );

        let store = CronStore::load(db_path).unwrap();
        let job = store
            .jobs()
            .iter()
            .find(|j| j.name == "nightly-digest")
            .expect("the job the RPC reported creating must be on disk");
        assert_eq!(
            job.owner_user_id.as_deref(),
            Some("u-x"),
            "a job created over the wire belongs to the caller who created it"
        );
        assert_eq!(
            job.scope_id.as_deref(),
            Some(
                crate::scope::ScopeId::Personal("u-x".to_string())
                    .render()
                    .as_str()
            ),
            "the scope column must be the rendered personal boundary, not just \
             a repeat of the owner id"
        );
    }

    #[tokio::test]
    async fn dispatch_with_caller_context_leaves_scope_unset_for_no_caller_user() {
        // Loopback / legacy shared-token connections resolve to `caller_user:
        // None` — must not seed a scope attribution (no owner to attribute to).
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;

        let mut registry = HandlerRegistry::new();
        registry.register("probe.scope", |req| async move {
            let owner = crate::scope::current_scope().map(|attr| attr.owner_user_id);
            JsonRpcResponse::success(req.id, serde_json::json!({ "owner_user_id": owner }))
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"probe.scope","params":{}}"#;

        let resp =
            dispatch_with_caller_context(text, &mc, Some("operator".to_string()), None, true, None)
                .await;
        assert!(
            resp.contains("\"owner_user_id\":null"),
            "no caller_user must mean no scope attribution: {resp}"
        );
    }

    /// `pty.resize` refuses to guess a connection identity — it reads
    /// `CALLER_CONN_ID`, which only `dispatch_with_caller_context` scopes.
    /// This proves that scope actually carries the real connection id from
    /// the dispatch boundary down into `process_request`'s handler dispatch,
    /// the same way `both_dispatch_stations_seed_scope` proves it for
    /// `CALLER_USER`.
    #[tokio::test]
    async fn dispatch_with_caller_context_seeds_conn_id() {
        use crate::gateway::handlers::HandlerRegistry;
        use crate::gateway::rate_limiter::RateLimitConfig;

        let mut registry = HandlerRegistry::new();
        registry.register("probe.conn_id", |req| async move {
            let conn_id = crate::gateway::caller_identity::current_caller_conn_id();
            JsonRpcResponse::success(req.id, serde_json::json!({ "conn_id": conn_id }))
        });
        let mc = MiddlewareChain::new(
            Arc::new(registry),
            Arc::new(RateLimiter::new(RateLimitConfig::default())),
        );
        let text = r#"{"jsonrpc":"2.0","id":1,"method":"probe.conn_id","params":{}}"#;

        let resp = dispatch_with_caller_context(
            text,
            &mc,
            Some("operator".to_string()),
            None,
            true,
            Some("127.0.0.1:9999".to_string()),
        )
        .await;
        assert!(
            resp.contains("\"conn_id\":\"127.0.0.1:9999\""),
            "conn id must be observable inside process_request's dispatch: {resp}"
        );
    }

    // ── extract_topic_and_data — wire-envelope tests (P1 fix round 1) ─────
    // The event_visibility.rs suite hand-builds post-extraction `data` and
    // never runs the REAL envelope through the REAL extraction. These tests
    // feed literal production wire JSON — generated via the actual publish
    // path wherever practical — through `extract_topic_and_data` itself, so
    // a future producer/wrapper-shape mismatch (like the "event"-wrapped
    // double-nesting fix round 1 found) shows up here, not just in a
    // classification unit test that never saw the real bytes.

    use crate::gateway::events::frame::GatewayEventFrame;

    /// The bare `TopicEvent` form — real producer: `publish_frame` on a
    /// non-stream `GatewayEventFrame` variant.
    #[test]
    fn extract_topic_and_data_handles_the_real_bare_topic_event_wire_form() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let frame = GatewayEventFrame::SessionLifecycleChanged {
            session_key: "agent:main:main".to_string(),
            old_state: None,
            new_state: "active".to_string(),
            reason: None,
        };
        bus.publish_frame(&frame).unwrap();
        let wire = rx
            .try_recv()
            .expect("publish_frame must deliver synchronously");
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();

        let (topic, data) = extract_topic_and_data(&event_obj);
        assert_eq!(topic, "session.lifecycle.changed");
        assert_eq!(
            data.and_then(|d| d.get("session_key"))
                .and_then(|v| v.as_str()),
            Some("agent:main:main")
        );
    }

    /// The `stream.*` JSON-RPC notification form — real producer:
    /// `publish_frame` on a streaming `GatewayEventFrame` variant. `data`
    /// stays `None` here by design (no stream.* frame nests a second `.data`
    /// inside `.params`) — the WS loop's `visibility_payload` fallback
    /// (`event_data.or_else(|| event_obj.get("params"))`) is what reaches
    /// into `.params` for this shape; exercised end-to-end below.
    #[test]
    fn extract_topic_and_data_handles_the_real_stream_wire_form() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let frame = GatewayEventFrame::RunAccepted {
            run_id: "r1".to_string(),
            session_key: "agent:main:main".to_string(),
            accepted_at: "t".to_string(),
        };
        bus.publish_frame(&frame).unwrap();
        let wire = rx
            .try_recv()
            .expect("publish_frame must deliver synchronously");
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();

        let (topic, data) = extract_topic_and_data(&event_obj);
        assert_eq!(topic, "stream.run_accepted");
        assert_eq!(
            data, None,
            "no stream.* frame nests a second .data in .params"
        );
        // The frame's own fields live directly under .params — same value
        // the WS loop's visibility_payload fallback reaches for.
        assert_eq!(
            event_obj
                .get("params")
                .and_then(|p| p.get("run_id"))
                .and_then(|v| v.as_str()),
            Some("r1")
        );
    }

    /// The double-wrapped `TopicEvent::to_notification()` form — real
    /// producer: `subagent_tree_relay.rs`'s exact construction
    /// (`TopicEvent::new(topic, data).to_notification()`, published as a raw
    /// string via `GatewayEventBus::publish`, bypassing `publish_frame`
    /// entirely). Before fix round 1, `extract_topic_and_data`'s
    /// predecessor read `topic` as the literal string `"event"` here —
    /// this pins the fix.
    #[test]
    fn extract_topic_and_data_unwraps_the_real_double_nested_event_envelope() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let tree_event = serde_json::json!({
            "kind": "settled",
            "node_id": "n1",
            "root_session": "agent:main:main",
            "lifecycle": "completed",
            "duration_ms": 100,
            "iterations": 1,
            "tool_calls_made": 1,
            "total_tokens": 10,
        });
        let notification = TopicEvent::new("run.subagent_tree", tree_event).to_notification();
        let json = serde_json::to_string(&notification).unwrap();
        bus.publish(json);
        let wire = rx.try_recv().expect("publish must deliver synchronously");
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();

        // Prove the envelope really is double-nested (method == "event", no
        // top-level "topic") — otherwise this test would pass for the wrong
        // reason.
        assert_eq!(
            event_obj.get("method").and_then(|m| m.as_str()),
            Some("event")
        );
        assert!(event_obj.get("topic").is_none());

        let (topic, data) = extract_topic_and_data(&event_obj);
        assert_eq!(
            topic, "run.subagent_tree",
            "must unwrap to the REAL topic, not the literal \"event\" wrapper method"
        );
        assert_eq!(
            data.and_then(|d| d.get("root_session"))
                .and_then(|v| v.as_str()),
            Some("agent:main:main")
        );
    }

    fn visibility_test_store() -> (
        crate::gateway::session_store::file_backend::FileSessionStore,
        tempfile::TempDir,
    ) {
        use crate::gateway::session_store::file_backend::{
            FileSessionStore, FileSessionStoreConfig,
        };
        let temp = tempfile::TempDir::new().unwrap();
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        (store, temp)
    }

    /// End-to-end: real `publish_frame` wire bytes → `extract_topic_and_data`
    /// → the SAME `visibility_payload` fallback the WS loop computes →
    /// `EventVisibilityIndex::note_frame`/`event_admits`. Proves the owner
    /// scoping this task adds actually receives a resolvable `run_id`/
    /// `session_key` from a REAL `RunAccepted`→`AgentTrace` run, not just a
    /// hand-built payload shaped to look like one.
    #[tokio::test]
    async fn owner_scoping_round_trips_through_the_real_publish_path() {
        use crate::gateway::event_visibility::EventVisibilityIndex;
        use crate::gateway::router::SessionKey;
        use crate::gateway::session_store::SessionStore;

        let (store, _temp) = visibility_test_store();
        let key = SessionKey::main("main");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("alice")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let index = EventVisibilityIndex::new();

        // Seed: real RunAccepted, real wire bytes, real extraction.
        let accepted = GatewayEventFrame::RunAccepted {
            run_id: "r1".to_string(),
            session_key: key.to_key_string(),
            accepted_at: "t".to_string(),
        };
        bus.publish_frame(&accepted).unwrap();
        let wire = rx.try_recv().unwrap();
        let event_obj: serde_json::Value = serde_json::from_str(&wire).unwrap();
        let (topic, event_data) = extract_topic_and_data(&event_obj);
        let visibility_payload = event_data.or_else(|| event_obj.get("params"));
        index.note_frame(topic, visibility_payload).await;

        // A later same-run frame, resolved purely through the seed above —
        // real wire bytes, real extraction, same fallback the loop uses.
        let trace = GatewayEventFrame::AgentTrace {
            run_id: "r1".to_string(),
            seq: 1,
            event: aleph_protocol::AgentTraceEvent::TurnStarted { iteration: 1 },
        };
        bus.publish_frame(&trace).unwrap();
        let wire2 = rx.try_recv().unwrap();
        let event_obj2: serde_json::Value = serde_json::from_str(&wire2).unwrap();
        let (topic2, event_data2) = extract_topic_and_data(&event_obj2);
        let visibility_payload2 = event_data2.or_else(|| event_obj2.get("params"));

        assert!(
            index
                .event_admits(
                    topic2,
                    visibility_payload2,
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    topic2,
                    visibility_payload2,
                    Some("bob"),
                    false,
                    &store,
                    None
                )
                .await
        );
    }

    /// The running-set projection end to end, through the SAME four steps the
    /// delivery loop runs — real `publish_frame` bytes → one parse →
    /// `extract_topic_and_data` → the loop's `visibility_payload` fallback →
    /// `project_for` → [`event_wire_form`] — and asserted on the BYTES that
    /// would be written to the socket, not on the projection's return value.
    ///
    /// The frame stays admitted (`Global`); what changes is what it says.
    #[tokio::test]
    async fn the_running_set_frame_reaches_the_wire_narrowed_to_this_connection() {
        use crate::gateway::event_visibility::EventVisibilityIndex;
        use crate::gateway::router::SessionKey;
        use crate::gateway::session_store::SessionStore;

        let (store, _temp) = visibility_test_store();
        let alice_key = SessionKey::main("wire-alice");
        let bob_key = SessionKey::main("wire-bob");
        for (key, owner) in [(&alice_key, "alice"), (&bob_key, "bob")] {
            crate::scope::with_scope(
                Some(crate::scope::ScopeAttribution::personal(owner)),
                store.get_or_create(key),
            )
            .await
            .unwrap();
        }
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        bus.publish_frame(&GatewayEventFrame::RunningSetChanged {
            seq: 12,
            running: vec![alice_key.to_key_string(), bob_key.to_key_string()],
        })
        .unwrap();
        let event_json = rx.try_recv().unwrap();
        assert!(
            event_json.contains(&bob_key.to_key_string()),
            "the frame as PUBLISHED carries every user's key — otherwise this \
             test would pass without any projection at all"
        );

        let parsed: serde_json::Value = serde_json::from_str(&event_json).unwrap();
        let (topic, event_data) = extract_topic_and_data(&parsed);
        let visibility_payload = event_data.or_else(|| parsed.get("params"));

        let index = EventVisibilityIndex::new();
        assert!(
            index
                .event_admits(
                    topic,
                    visibility_payload,
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await,
            "the frame itself stays Global — suppressing it would latch alice's \
             red dot on the seq guard"
        );
        let projected = index
            .project_for(topic, visibility_payload, Some("alice"), &store)
            .await;

        let wire = event_wire_form(parsed, projected, event_json.clone());
        assert_ne!(wire, event_json, "the bytes on the wire must have changed");
        let sent: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            sent["method"], "stream.running_set_changed",
            "still the same notification the Panel dispatches on"
        );
        assert_eq!(
            sent["params"]["running"],
            serde_json::json!([alice_key.to_key_string()]),
            "alice is told about her own session and nobody else's"
        );
        assert_eq!(
            sent["params"]["seq"], 12,
            "the client's ordering guard must survive the rewrite verbatim"
        );
    }

    /// The other half of [`event_wire_form`]'s contract, and the reason the
    /// projection is affordable: a frame with nothing to project is forwarded
    /// as the ORIGINAL bytes — no re-serialization, byte-identical — while the
    /// bare `TopicEvent` form is still wrapped exactly as before.
    #[test]
    fn an_unprojected_frame_is_forwarded_as_its_original_bytes() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        bus.publish_frame(&GatewayEventFrame::RunAccepted {
            run_id: "r1".to_string(),
            session_key: "agent:main:main".to_string(),
            accepted_at: "t".to_string(),
        })
        .unwrap();
        let stream_json = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stream_json).unwrap();
        assert_eq!(
            event_wire_form(parsed, None, stream_json.clone()),
            stream_json,
            "a stream-form frame nobody projected must be forwarded verbatim"
        );

        bus.publish_frame(&GatewayEventFrame::SessionLifecycleChanged {
            session_key: "agent:main:main".to_string(),
            old_state: None,
            new_state: "active".to_string(),
            reason: None,
        })
        .unwrap();
        let topic_json = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&topic_json).unwrap();
        let wrapped: serde_json::Value =
            serde_json::from_str(&event_wire_form(parsed, None, topic_json)).unwrap();
        assert_eq!(
            wrapped["method"], "event",
            "the bare TopicEvent form is still wrapped for the Panel"
        );
        assert_eq!(wrapped["params"]["topic"], "session.lifecycle.changed");
    }

    /// Source-level pin for the delivery loop, which has no unit-testable seam
    /// of its own — it is one `tokio::select!` arm inside the socket task, so
    /// every function it calls can be green while the loop calls none of them.
    /// Two facts about it are invisible from everywhere else:
    ///
    /// 1. the event JSON is parsed exactly ONCE per frame (it was parsed a
    ///    second time for years, purely to decide whether to wrap a
    ///    `TopicEvent` — deleting that is what pays for the projection), and
    /// 2. `EventVisibilityIndex::project_for` is actually CALLED there.
    ///
    /// Only the PRODUCTION half of the file is inspected (everything above the
    /// first test module) and the needles are assembled at runtime, so neither
    /// the tests above nor this one can count as a match.
    #[test]
    fn the_delivery_loop_parses_each_event_once_and_projects_it() {
        let src = include_str!("handler.rs");
        let production = src
            .split(&format!("#[cfg{}]", "(test)"))
            .next()
            .expect("the file has a production half");

        let parse_needle = format!(
            "serde_json::from_str::<serde_json::Value>(&{}_json)",
            "event"
        );
        assert_eq!(
            production.matches(&parse_needle).count(),
            1,
            "the delivery loop must parse each event exactly once; a second \
             `{parse_needle}` means the double parse is back"
        );

        let project_needle = format!("{}_for(", "project");
        assert_eq!(
            production.matches(&project_needle).count(),
            1,
            "the payload projection must be wired into the delivery loop — \
             `project_for` is fully tested in `event_visibility`, and with no \
             call site here that proves nothing"
        );
    }
}
