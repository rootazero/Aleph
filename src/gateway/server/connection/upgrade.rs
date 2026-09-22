//! Upgrade phase — HTTP → WebSocket handshake + connection-cap + origin gates.
//!
//! Owns the axum entry-point `ws_upgrade_handler` and the two pure gates
//! (`parse_trusted_ips`, `refuse_insecure_remote`) it composes. The
//! `ConnectionContext` is built here at upgrade time and handed to the
//! `handle_connection` orchestrator in `connection/mod.rs` once the socket
//! is taken over.
//!
//! All items are `pub(crate)` so `handler.rs` can re-export the public
//! axum entry-point + the cross-module gates (`refuse_insecure_remote` is
//! also used by `artifact_route.rs` / `canvas_asset_route.rs`,
//! `parse_trusted_ips` by `server/mod.rs`).

use crate::sync_primitives::Arc;
use crate::gateway::lane::ChannelClass;
use axum::{
    extract::{
        ws::{WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    http::{header, HeaderMap},
    response::IntoResponse,
};
use std::net::{IpAddr, SocketAddr};
use tracing::{error, warn};

use super::{ConnectionContext, handle_connection};
use crate::gateway::server::GatewaySharedState;

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
pub fn parse_trusted_ips(raw: &[String]) -> Vec<IpAddr> {
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
pub fn refuse_insecure_remote(
    client_is_local: bool,
    secure: bool,
    allow_insecure_remote: bool,
) -> bool {
    !client_is_local && !secure && !allow_insecure_remote
}

/// axum handler: upgrade HTTP connection to WebSocket at `/ws`
pub async fn ws_upgrade_handler(
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

    ws.on_upgrade(move |socket: WebSocket| async move {
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