//! `/viewer/browser/...` — the r4 capability-limited browser viewer route.
//!
//! # This is a third ingress
//!
//! Like `artifact_route`, nothing here inherits `/ws`'s `connect` handshake,
//! device tier, or JSON-RPC middleware — a fragment URL in a browser cannot
//! run those. Every guard is restated explicitly (transport refusal, origin
//! policy, strict per-grant origin binding), and authorization is the r4
//! capability chain itself: fragment nonce -> exchange ticket -> session
//! secret rotation, all single-use, all hashed at rest in
//! [`ViewerGrantStore`]. A request without the current secret gets one
//! generic denial and learns nothing.
//!
//! # Secrets never travel in the URL
//!
//! The nonce lives in the URL *fragment* (`/viewer/browser#grant=...`), which
//! browsers never send to the server; the SPA hands it to JS, which POSTs it
//! to `/viewer/browser/consume`. Nothing in this module reads a query string,
//! cookie, header, or subprotocol for credentials.
//!
//! # AuthOk-before-live
//!
//! After a successful rotation commit the winner socket is written
//! `AuthOk { next_secret }`. Only if that write succeeds does the session
//! become live: `TargetControl::register_viewer` runs after the write, never
//! before, and the loser of a commit race gets `Conflict` and a close. If
//! the AuthOk write fails, the rotation decision is final (the client never
//! saw `next_secret`); the socket closes unregistered and the grant is
//! REVOKED (fail closed) so it needs re-issue — that is the contract, not a
//! bug.
//!
//! # One live connection per grant (no silent secret loss)
//!
//! The prepared viewer (screencast subscription + control plane) is parked
//! exactly once, at ticket exchange. A grant therefore supports exactly ONE
//! authenticated WS: the parked viewer is claimed BEFORE the rotation commit,
//! so a hello that has nothing to attach to (second connection, expired
//! parking, lost race) is refused `Conflict` WITHOUT rotating — the secret is
//! never consumed to hand back an `AuthOk` that cannot go live. When the
//! single connection ends for any reason the grant is revoked, so
//! `next_secret` (reconnect) is explicitly unsupported rather than a secret
//! that works on paper and is silently dead.
//!
//! # No business logic here (R4)
//!
//! Everything CDP is behind [`ViewerBackend`]; this module parses, checks,
//! and moves bytes.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use aleph_protocol::browser_live::{
    decode_viewer_auth_client_message, AuthError, AuthErrorCode, AuthHello, AuthOk,
    BrowserLiveClosed, ClosedReason, ConsumeFragmentNonceRequest, ConsumeFragmentNonceResponse,
    ExchangeViewerGrantRequest, ExchangeViewerGrantResponse, HumanInputResult, InterventionEnd,
    ResourceBinding, ViewerAuthClientMessage, ViewerAuthServerMessage, ViewerClientMessage,
    ViewerOrigin, ViewerPermission, ViewerServerMessage, ViewerWsFrame, ViewerWsFrameHeader,
    VIEWER_PROTOCOL_VERSION,
};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;

use crate::browser::live::grant::ViewerGrantStore;
use crate::browser::live::viewer_backend::{PreparedViewer, ViewerBackend};
use crate::gateway::origin_policy::OriginPolicy;
use crate::gateway::trusted_proxy::resolve_client;
use crate::sync_primitives::Arc;

use super::handler::refuse_insecure_remote;

/// WS path handed to clients in `ExchangeViewerGrantResponse`.
pub(crate) const VIEWER_WS_PATH: &str = "/viewer/browser/ws";

/// First-frame deadline: exactly one TEXT AuthHello within two seconds of
/// the upgrade, per the r4 contract.
const AUTH_TIMEOUT: Duration = Duration::from_secs(2);

/// Body cap for the two JSON posts; both requests are tiny.
const JSON_BODY_MAX: usize = 4 * 1024;

/// Inbound WS message cap. Post-auth the client only ever sends small JSON
/// control/input messages; binary inbound is rejected outright.
const WS_INBOUND_MAX: usize = 8 * 1024;

/// How long an exchanged-but-never-authenticated prepared viewer is parked
/// before its control plane is torn down.
const PARKED_TTL: Duration = Duration::from_secs(120);

/// Everything the viewer route needs — and nothing else. Deliberately not
/// `GatewaySharedState`, for the same reason as `ArtifactRouteState`: the
/// connection table, handler registry, and event bus are none of this
/// route's business, and a narrow state keeps the guards testable without
/// booting a gateway.
pub(crate) struct BrowserViewerRouteState {
    store: ViewerGrantStore,
    backend: Arc<dyn ViewerBackend>,
    origin_policy: Arc<OriginPolicy>,
    trusted_proxy_enabled: bool,
    trusted_proxy_ips: Vec<IpAddr>,
    allow_insecure_remote: bool,
    tls_enabled: bool,
    /// Prepared viewers between ticket exchange and AuthOk commit. Keyed by
    /// grant id; at most one entry per grant because the ticket is
    /// single-use.
    parked: Mutex<HashMap<String, ParkedViewer>>,
    /// First-frame deadline; 2s in production, shortened in tests.
    auth_timeout: Duration,
}

struct ParkedViewer {
    prepared: PreparedViewer,
    deadline: Instant,
}

impl BrowserViewerRouteState {
    pub(crate) fn new(
        store: ViewerGrantStore,
        backend: Arc<dyn ViewerBackend>,
        origin_policy: Arc<OriginPolicy>,
        trusted_proxy_enabled: bool,
        trusted_proxy_ips: Vec<IpAddr>,
        allow_insecure_remote: bool,
        tls_enabled: bool,
    ) -> Self {
        Self {
            store,
            backend,
            origin_policy,
            trusted_proxy_enabled,
            trusted_proxy_ips,
            allow_insecure_remote,
            tls_enabled,
            parked: Mutex::new(HashMap::new()),
            auth_timeout: AUTH_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn set_auth_timeout(&mut self, timeout: Duration) {
        self.auth_timeout = timeout;
    }

    fn lock_parked(&self) -> MutexGuard<'_, HashMap<String, ParkedViewer>> {
        self.parked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Park a prepared viewer for its one WS. A timer drops it (and revokes
    /// the grant, whose `S0` can no longer go live) if nobody authenticates
    /// within [`PARKED_TTL`]. Dropping — never invalidating — the control
    /// plane: it may be shared with other viewers of the same target.
    fn park(self: &Arc<Self>, grant_id: String, prepared: PreparedViewer) {
        let deadline = Instant::now() + PARKED_TTL;
        self.lock_parked()
            .insert(grant_id.clone(), ParkedViewer { prepared, deadline });
        let weak = std::sync::Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(PARKED_TTL).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            if state.expire_parked(&grant_id) {
                state.store.revoke(&grant_id);
            }
        });
    }

    /// Drop a parked viewer whose deadline passed. True when one was dropped.
    fn expire_parked(&self, grant_id: &str) -> bool {
        let mut parked = self.lock_parked();
        match parked.get(grant_id) {
            Some(entry) if Instant::now() >= entry.deadline => {
                parked.remove(grant_id);
                true
            }
            _ => false,
        }
    }

    /// Claim the parked viewer (single-use). An overdue entry is dropped, not
    /// handed out.
    fn take_parked(&self, grant_id: &str) -> Option<PreparedViewer> {
        let mut parked = self.lock_parked();
        let entry = parked.remove(grant_id)?;
        if Instant::now() >= entry.deadline {
            return None;
        }
        Some(entry.prepared)
    }

    #[cfg(test)]
    fn parked_len(&self) -> usize {
        self.lock_parked().len()
    }
}

/// The viewer routes, ready to `merge` into the gateway router BEFORE the
/// control-plane SPA fallback.
pub(crate) fn browser_viewer_routes(state: Arc<BrowserViewerRouteState>) -> Router {
    Router::new()
        .route("/viewer/browser/consume", post(consume_nonce))
        .route("/viewer/browser/exchange", post(exchange_ticket))
        .route(VIEWER_WS_PATH, get(viewer_ws_upgrade))
        .layer(DefaultBodyLimit::max(JSON_BODY_MAX))
        .with_state(state)
}

/// The one generic refusal: unknown, unauthorized, expired, and malformed
/// credentials are indistinguishable.
fn denied() -> Response {
    (StatusCode::FORBIDDEN, "denied").into_response()
}

/// Transport + origin guards, restated from `/ws` (see `artifact_route` for
/// why none of `/ws`'s protections are inherited). Returns the strictly
/// parsed request origin on success: the r4 viewer flow is browser-only and
/// a missing or unparseable `Origin` is a refusal, never a wildcard.
fn preflight(
    state: &BrowserViewerRouteState,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> Result<ViewerOrigin, (StatusCode, &'static str)> {
    let resolved = resolve_client(
        peer.ip(),
        headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let secure = state.tls_enabled || resolved.secure;
    if refuse_insecure_remote(resolved.local, secure, state.allow_insecure_remote) {
        return Err((StatusCode::UPGRADE_REQUIRED, "transport requires TLS"));
    }
    let origin_raw = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !state.origin_policy.is_allowed(origin_raw, host) {
        return Err((StatusCode::FORBIDDEN, "origin not allowed"));
    }
    let Some(origin_raw) = origin_raw else {
        return Err((StatusCode::FORBIDDEN, "origin required"));
    };
    parse_viewer_origin(origin_raw).ok_or((StatusCode::FORBIDDEN, "origin rejected"))
}

/// Parse an `Origin` header value into the strict (scheme, host, port)
/// triple. Rejects credentials, non-root paths, query, fragment, unsupported
/// schemes, and the literal `null`. Hosts compare lowercase; ports are
/// effective (80/443 defaults applied).
fn parse_viewer_origin(raw: &str) -> Option<ViewerOrigin> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
        return None;
    }
    let uri = raw.parse::<Uri>().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = uri.authority()?;
    // Userinfo means credentials in the origin; those are not origins.
    if authority.as_str().contains('@') {
        return None;
    }
    if let Some(pq) = uri.path_and_query() {
        if pq.as_str() != "/" {
            return None;
        }
    }
    let host = uri.host()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let port = uri.port_u16().unwrap_or(default_port);
    Some(ViewerOrigin { scheme, host, port })
}

/// `POST /viewer/browser/consume` — fragment nonce -> exchange ticket.
async fn consume_nonce(
    State(state): State<Arc<BrowserViewerRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let origin = match preflight(&state, peer, &headers) {
        Ok(origin) => origin,
        Err(refusal) => return refusal.into_response(),
    };
    let Ok(req) = serde_json::from_slice::<ConsumeFragmentNonceRequest>(&body) else {
        return denied();
    };
    match state
        .store
        .consume_nonce(&req.grant_id, &req.nonce, Some(&origin))
    {
        Ok(consumed) => axum::Json(ConsumeFragmentNonceResponse {
            ticket: consumed.ticket,
            expires_at_ms: consumed.expires_at_ms,
        })
        .into_response(),
        Err(_) => denied(),
    }
}

/// `POST /viewer/browser/exchange` — exchange ticket -> session secret S0 +
/// snapshot. The viewer session is prepared (screencast attached, control
/// plane bound) here and parked until the AuthOk write commits.
async fn exchange_ticket(
    State(state): State<Arc<BrowserViewerRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let origin = match preflight(&state, peer, &headers) {
        Ok(origin) => origin,
        Err(refusal) => return refusal.into_response(),
    };
    let Ok(req) = serde_json::from_slice::<ExchangeViewerGrantRequest>(&body) else {
        return denied();
    };
    let grant = match state.store.exchange(&req.ticket, Some(&origin)) {
        Ok(grant) => grant,
        Err(_) => return denied(),
    };
    let prepared = match state.backend.prepare(&grant).await {
        Ok(prepared) => prepared,
        Err(_) => {
            // The ticket is already consumed; fail closed by revoking the
            // fresh session so nothing half-prepared lingers.
            state.store.revoke(&grant.grant_id);
            return denied();
        }
    };
    let response = ExchangeViewerGrantResponse {
        viewer_session_id: grant.viewer_session_id,
        grant_id: grant.grant_id.clone(),
        rotation_generation: grant.rotation_generation,
        current_secret: grant.current_secret,
        ws_path: VIEWER_WS_PATH.to_string(),
        expires_at_ms: grant.expires_at_ms,
        snapshot: prepared.snapshot.clone(),
    };
    state.park(grant.grant_id, prepared);
    axum::Json(response).into_response()
}

/// `GET /viewer/browser/ws` — unauthenticated at upgrade by design; the
/// first frame authenticates. Secrets never appear in the URL, headers, or
/// subprotocol.
async fn viewer_ws_upgrade(
    State(state): State<Arc<BrowserViewerRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let origin = match preflight(&state, peer, &headers) {
        Ok(origin) => origin,
        Err(refusal) => return refusal.into_response(),
    };
    ws.max_message_size(WS_INBOUND_MAX)
        .on_upgrade(move |socket| run_viewer_socket(socket, state, origin))
}

/// One socket's whole life: first-frame auth, rotation commit, AuthOk write,
/// then the live loop. Any pre-auth failure sends its `AuthError` (always
/// `close: true`) and ends the socket.
async fn run_viewer_socket(
    mut socket: WebSocket,
    state: Arc<BrowserViewerRouteState>,
    origin: ViewerOrigin,
) {
    let hello = match read_auth_hello(&mut socket, state.auth_timeout).await {
        Ok(hello) => hello,
        Err(error) => {
            send_auth_error(&mut socket, error).await;
            return;
        }
    };
    if hello.protocol_version != VIEWER_PROTOCOL_VERSION {
        send_auth_error(
            &mut socket,
            AuthError {
                code: AuthErrorCode::UnsupportedVersion,
                close: true,
            },
        )
        .await;
        return;
    }
    let attempt = match state.store.begin_auth(&hello) {
        Ok(attempt) => attempt,
        Err(code) => {
            send_auth_error(&mut socket, AuthError { code, close: true }).await;
            return;
        }
    };
    // The socket's own origin must equal the grant's bound origin — checked
    // before commit so a cross-origin socket neither commits nor burns the
    // attempt registration (drop aborts it).
    if !same_origin(attempt.grant_origin(), &origin) {
        send_auth_error(
            &mut socket,
            AuthError {
                code: AuthErrorCode::Unauthorized,
                close: true,
            },
        )
        .await;
        return;
    }
    // Claim the parked viewer BEFORE the rotation commit: nothing to attach
    // to means a refusal that leaves the secret unconsumed (see module docs).
    let Some(prepared) = state.take_parked(&hello.grant_id) else {
        send_auth_error(
            &mut socket,
            AuthError {
                code: AuthErrorCode::Conflict,
                close: true,
            },
        )
        .await;
        return;
    };
    let commit = match state.store.commit_auth(attempt) {
        Ok(commit) => commit,
        Err(code) => {
            // Revoked/expired between begin and commit; `prepared` is dropped
            // (the receiver detaches, the shared control plane is untouched).
            send_auth_error(&mut socket, AuthError { code, close: true }).await;
            return;
        }
    };
    let auth_ok = ViewerAuthServerMessage::AuthOk {
        ok: AuthOk {
            protocol_version: VIEWER_PROTOCOL_VERSION,
            ws_generation: commit.ws_generation,
            next_secret: commit.next_secret,
        },
    };
    let Ok(text) = serde_json::to_string(&auth_ok) else {
        state.store.revoke(&hello.grant_id);
        return;
    };
    if socket.send(Message::Text(text.into())).await.is_err() {
        // Rotation decision is final and the client never saw next_secret.
        // The winner is NOT live (no register, no frames) and the grant is
        // revoked so no zombie secret remains.
        state.store.revoke(&hello.grant_id);
        return;
    }
    let lease = match prepared.control.register_viewer(
        &commit.viewer_session_id,
        commit.ws_generation,
        prepared.expires_at_ms,
        prepared.permissions.clone(),
    ) {
        Ok(lease) => lease,
        Err(_) => {
            state.store.revoke(&hello.grant_id);
            return;
        }
    };
    if !state
        .store
        .publish_live(&hello.grant_id, prepared.control.clone(), lease.clone())
    {
        // Revoked/expired between commit and publish: close the lease now.
        let _ = prepared.control.revoke(&lease, InterventionEnd::Revoked);
        return;
    }
    let control = prepared.control.clone();
    run_live_loop(&mut socket, &state, prepared, &lease).await;
    // Single-connection grant: the session is over, so end its authority.
    // Revoking the lease hands a Human controller back to the agent; revoking
    // the grant leaves no usable secret behind.
    let _ = control.revoke(&lease, InterventionEnd::Revoked);
    state.store.revoke(&hello.grant_id);
}

fn same_origin(a: &ViewerOrigin, b: &ViewerOrigin) -> bool {
    a.scheme == b.scheme && a.host == b.host && a.port == b.port
}

/// Read the one pre-auth frame. Any deviation — timeout, close, binary,
/// oversize, malformed JSON, wrong shape — maps to its `AuthError`.
async fn read_auth_hello(
    socket: &mut WebSocket,
    timeout: Duration,
) -> Result<AuthHello, AuthError> {
    let malformed = || AuthError {
        code: AuthErrorCode::Malformed,
        close: true,
    };
    let message = match tokio::time::timeout(timeout, socket.recv()).await {
        Ok(Some(Ok(message))) => message,
        Ok(_) => return Err(malformed()),
        Err(_) => {
            return Err(AuthError {
                code: AuthErrorCode::Timeout,
                close: true,
            })
        }
    };
    let Message::Text(text) = message else {
        return Err(malformed());
    };
    match decode_viewer_auth_client_message(text.as_bytes()) {
        Ok(ViewerAuthClientMessage::AuthHello { hello }) => {
            hello.validate_shape().map_err(|_| malformed())?;
            Ok(hello)
        }
        Err(_) => Err(malformed()),
    }
}

async fn send_auth_error(socket: &mut WebSocket, error: AuthError) {
    let message = ViewerAuthServerMessage::AuthError { error };
    if let Ok(text) = serde_json::to_string(&message) {
        let _ = socket.send(Message::Text(text.into())).await;
    }
    let _ = socket.send(Message::Close(None)).await;
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The authenticated loop: frames and control flow server->client as
/// `ViewerWsFrame` binaries and `ViewerServerMessage` text; client->server
/// only `ViewerClientMessage` text. Client binary is always rejected.
async fn run_live_loop(
    socket: &mut WebSocket,
    state: &Arc<BrowserViewerRouteState>,
    prepared: PreparedViewer,
    lease: &crate::browser::live::control::ViewerLease,
) {
    let PreparedViewer {
        snapshot,
        control,
        mut frames,
        binding,
        resource_id,
        permissions,
        expires_at_ms,
    } = prepared;
    let target_id = binding.target.target_id.clone();
    let can_frames = permissions.contains(&ViewerPermission::Frames);
    let mut close_rx = lease.close_signal();
    loop {
        tokio::select! {
            // `wait_for`, not `changed`: a revoke that landed before this
            // receiver was cloned must still close the socket.
            _ = async { let _ = close_rx.wait_for(|closed| *closed).await; } => {
                let reason = if !control.is_live() {
                    ClosedReason::TargetClosed
                } else if now_ms() >= expires_at_ms {
                    ClosedReason::Expired
                } else {
                    ClosedReason::Revoked
                };
                let closed = ViewerServerMessage::Closed {
                    closed: BrowserLiveClosed {
                        resource_id: resource_id.clone(),
                        binding: ResourceBinding::Browser {
                            viewer: binding.clone(),
                        },
                        reason,
                        at_ms: now_ms(),
                    },
                };
                let _ = send_server(socket, &closed).await;
                return;
            }
            frame = frames.next(), if can_frames => {
                let Some(frame) = frame else {
                    // The screencast ended under us: the target is gone.
                    let closed = ViewerServerMessage::Closed {
                        closed: BrowserLiveClosed {
                            resource_id: resource_id.clone(),
                            binding: ResourceBinding::Browser {
                                viewer: binding.clone(),
                            },
                            reason: ClosedReason::TargetClosed,
                            at_ms: now_ms(),
                        },
                    };
                    let _ = send_server(socket, &closed).await;
                    return;
                };
                // LiveFrame.engine_ack_id is read by NOBODY here — it is an
                // engine-side detail that never reaches the wire.
                let ws_frame = ViewerWsFrame {
                    header: ViewerWsFrameHeader {
                        version: 1,
                        resource_id: resource_id.clone(),
                        binding: binding.clone(),
                        connection_epoch: snapshot.connection_epoch,
                        stream_epoch: frame.epoch,
                        seq: frame.seq,
                        captured_at_ms: frame
                            .timestamp
                            .map(|t| t as i64)
                            .unwrap_or_else(now_ms),
                        viewport_css_px: snapshot.viewport_css_px.clone(),
                        content_type: "image/jpeg".to_string(),
                    },
                    jpeg_payload: frame.data.to_vec(),
                };
                let Ok(bytes) = ws_frame.encode() else {
                    continue;
                };
                if socket.send(Message::Binary(bytes.into())).await.is_err() {
                    return;
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_client_message(
                            socket, state, &control, lease, &snapshot,
                            &target_id, &text,
                        ).await {
                            return;
                        }
                    }
                    // Client binary is always rejected post-auth.
                    Some(Ok(Message::Binary(_))) => return,
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    // Ping/Pong are answered by the transport.
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// One post-auth client message. Returns false when the socket should close.
async fn handle_client_message(
    socket: &mut WebSocket,
    state: &Arc<BrowserViewerRouteState>,
    control: &Arc<crate::browser::live::control::TargetControl>,
    lease: &crate::browser::live::control::ViewerLease,
    snapshot: &aleph_protocol::browser_live::BrowserLiveSnapshot,
    target_id: &str,
    text: &str,
) -> bool {
    let Ok(message) = serde_json::from_str::<ViewerClientMessage>(text) else {
        return send_server(
            socket,
            &ViewerServerMessage::Error {
                code: "malformed".to_string(),
            },
        )
        .await;
    };
    match message {
        ViewerClientMessage::Attach { .. } => {
            if !send_server(
                socket,
                &ViewerServerMessage::Snapshot {
                    snapshot: snapshot.clone(),
                },
            )
            .await
            {
                return false;
            }
            send_server(
                socket,
                &ViewerServerMessage::State {
                    state: control.snapshot(),
                },
            )
            .await
        }
        ViewerClientMessage::Take { request } => {
            let result = control.take(lease, request).await;
            if !send_server(socket, &ViewerServerMessage::ControlResult { result }).await {
                return false;
            }
            send_server(
                socket,
                &ViewerServerMessage::State {
                    state: control.snapshot(),
                },
            )
            .await
        }
        ViewerClientMessage::Cede { request } => {
            let result = control.cede(lease, request).await;
            if !send_server(socket, &ViewerServerMessage::ControlResult { result }).await {
                return false;
            }
            send_server(
                socket,
                &ViewerServerMessage::State {
                    state: control.snapshot(),
                },
            )
            .await
        }
        ViewerClientMessage::Input { request } => {
            let seq = request.seq;
            match control.queue_input(lease, request) {
                Ok(queued) => {
                    let backend = state.backend.clone();
                    let target = target_id.to_string();
                    match control
                        .dispatch(queued, move |event| async move {
                            backend.dispatch_input(&target, event).await
                        })
                        .await
                    {
                        Ok(result) => {
                            send_server(socket, &ViewerServerMessage::InputResult { result }).await
                        }
                        Err(_) => {
                            send_server(
                                socket,
                                &ViewerServerMessage::Error {
                                    code: "dispatch_failed".to_string(),
                                },
                            )
                            .await
                        }
                    }
                }
                Err(reason) => {
                    send_server(
                        socket,
                        &ViewerServerMessage::InputResult {
                            result: HumanInputResult {
                                seq,
                                accepted: false,
                                reason: Some(reason),
                            },
                        },
                    )
                    .await
                }
            }
        }
    }
}

async fn send_server(socket: &mut WebSocket, message: &ViewerServerMessage) -> bool {
    let Ok(text) = serde_json::to_string(message) else {
        return false;
    };
    socket.send(Message::Text(text.into())).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        AttachRequest, BrowserLiveSnapshot, BrowserLiveState, BrowserTargetBinding, ControlRequest,
        Controller, ConversationBinding, DisplayPolicy, HumanInputEvent, IssuerIdentity,
        PreviewKind, PreviewProvenance, PreviewResource, PreviewSource, RedactionStatus,
        ViewerBinding, ViewportCssPx,
    };
    use axum::body::Body;
    use axum::http::Request;
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
    use tower::ServiceExt;

    use crate::browser::error::BrowserError;
    use crate::browser::live::control::{InterventionSink, TargetControl};
    use crate::browser::live::grant::ExchangedGrant;
    use crate::browser::live::stream_hub::LatestFrameHub;
    use crate::browser::live::viewer_backend::BoxFuture;

    const ORIGIN: &str = "http://127.0.0.1:8787";
    /// Loopback, so the coarse OriginPolicy passes it, but NOT the origin
    /// the grant is bound to — the strict check must still refuse it.
    const ALT_ORIGIN: &str = "http://localhost:8787";
    const CONSUME_PATH: &str = "/viewer/browser/consume";
    const EXCHANGE_PATH: &str = "/viewer/browser/exchange";

    fn grant_origin() -> ViewerOrigin {
        ViewerOrigin {
            scheme: "http".into(),
            host: "127.0.0.1".into(),
            port: 8787,
        }
    }

    fn test_binding(target_id: &str) -> ViewerBinding {
        ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: "user-1".into(),
            },
            task: ConversationBinding {
                session_key: "agent:main:demo".into(),
                run_id: "run-1".into(),
            },
            target: BrowserTargetBinding {
                profile: "default".into(),
                browser_session_id: "bs-1".into(),
                target_id: target_id.into(),
            },
        }
    }

    const PERMS: [ViewerPermission; 3] = [
        ViewerPermission::Frames,
        ViewerPermission::HumanInput,
        ViewerPermission::Navigate,
    ];

    /// In-memory backend. Its `prepare` REFUSES a grant scoped to a
    /// different resource or target — the cross-scope test leans on that.
    struct FakeBackend {
        control: Arc<TargetControl>,
        hub: Mutex<LatestFrameHub>,
        dispatched: Mutex<Vec<(String, HumanInputEvent)>>,
        resource_id: String,
        binding: ViewerBinding,
        expires_at_ms: i64,
    }

    impl FakeBackend {
        fn new(binding: &ViewerBinding, resource_id: &str) -> Self {
            let live = BrowserLiveState {
                resource_id: resource_id.into(),
                binding: binding.clone(),
                connection_epoch: 7,
                stream_epoch: 3,
                viewport_css_px: ViewportCssPx {
                    width: 1280,
                    height: 800,
                },
                controller: Controller::Agent,
                control_epoch: 0,
            };
            let sink: InterventionSink =
                Arc::new(|_summary| -> Result<(), BrowserError> { Ok(()) });
            Self {
                control: TargetControl::new(live, sink),
                hub: Mutex::new(LatestFrameHub::new()),
                dispatched: Mutex::new(Vec::new()),
                resource_id: resource_id.into(),
                binding: binding.clone(),
                expires_at_ms: now_ms() + 600_000,
            }
        }

        fn snapshot(&self) -> BrowserLiveSnapshot {
            let live = self.control.snapshot();
            BrowserLiveSnapshot {
                resource: PreviewResource {
                    resource_id: self.resource_id.clone(),
                    revision: 0,
                    binding: ResourceBinding::Browser {
                        viewer: self.binding.clone(),
                    },
                    kind: PreviewKind::Page,
                    display: DisplayPolicy::AutoRevealCurrentRun,
                    source: PreviewSource::BrowserTool,
                    provenance: PreviewProvenance::NotManaged,
                    expires_at_ms: self.expires_at_ms,
                    redaction: RedactionStatus::Pending,
                    companion_candidates: Vec::new(),
                },
                connection_epoch: live.connection_epoch,
                stream_epoch: live.stream_epoch,
                last_frame_seq: 0,
                viewport_css_px: live.viewport_css_px,
                controller: live.controller,
                control_epoch: live.control_epoch,
            }
        }
    }

    impl ViewerBackend for FakeBackend {
        fn prepare<'a>(
            &'a self,
            grant: &'a ExchangedGrant,
        ) -> BoxFuture<'a, Result<PreparedViewer, BrowserError>> {
            let scoped = grant.resource_id == self.resource_id && grant.binding == self.binding;
            let frames = self.hub.lock().unwrap().subscribe();
            Box::pin(async move {
                if !scoped {
                    return Err(BrowserError::ActionFailed(
                        "grant scope does not match this backend".into(),
                    ));
                }
                Ok(PreparedViewer {
                    snapshot: self.snapshot(),
                    control: self.control.clone(),
                    frames,
                    binding: self.binding.clone(),
                    resource_id: self.resource_id.clone(),
                    permissions: PERMS.to_vec(),
                    expires_at_ms: self.expires_at_ms,
                })
            })
        }

        fn dispatch_input<'a>(
            &'a self,
            target_id: &'a str,
            event: HumanInputEvent,
        ) -> BoxFuture<'a, Result<(), BrowserError>> {
            self.dispatched
                .lock()
                .unwrap()
                .push((target_id.to_string(), event));
            Box::pin(async { Ok(()) })
        }
    }

    fn test_state(
        auth_timeout: Duration,
    ) -> (
        Arc<BrowserViewerRouteState>,
        ViewerGrantStore,
        Arc<FakeBackend>,
    ) {
        let store = ViewerGrantStore::default();
        let binding = test_binding("target-1");
        let backend = Arc::new(FakeBackend::new(&binding, "res-1"));
        let mut state = BrowserViewerRouteState::new(
            store.clone(),
            backend.clone(),
            Arc::new(OriginPolicy::loopback_only()),
            false,
            Vec::new(),
            true,
            false,
        );
        state.set_auth_timeout(auth_timeout);
        (Arc::new(state), store, backend)
    }

    /// Mint a grant the honest way; returns (grant_id, plaintext nonce).
    fn issue(store: &ViewerGrantStore) -> (String, String) {
        let issued = store
            .issue(
                IssuerIdentity::User {
                    user_id: "user-1".into(),
                },
                test_binding("target-1"),
                "res-1",
                &PERMS,
                grant_origin(),
                Duration::from_secs(600),
            )
            .expect("issue");
        (issued.grant_id, issued.nonce.0)
    }

    fn post(uri: &str, origin: Option<&str>, body: String) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        let mut request = builder.body(Body::from(body)).expect("request");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
        request
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn viewer_credential_cannot_cross_scope() {
        let (state, store, _backend) = test_state(Duration::from_secs(2));
        let app = browser_viewer_routes(state);
        let (grant_a, nonce_a) = issue(&store);
        let (grant_b, _nonce_b) = issue(&store);
        let consume = |grant: &str, nonce: &str| {
            serde_json::json!({"grant_id": grant, "nonce": nonce}).to_string()
        };

        // Grant A's nonce against grant B's id.
        let resp = app
            .clone()
            .oneshot(post(
                CONSUME_PATH,
                Some(ORIGIN),
                consume(&grant_b, &nonce_a),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        // Missing Origin: the viewer flow is browser-only.
        let resp = app
            .clone()
            .oneshot(post(CONSUME_PATH, None, consume(&grant_a, &nonce_a)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        // A loopback-but-foreign Origin is denied AND must not burn the nonce.
        let resp = app
            .clone()
            .oneshot(post(
                CONSUME_PATH,
                Some(ALT_ORIGIN),
                consume(&grant_a, &nonce_a),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // The honest pair succeeds.
        let resp = app
            .clone()
            .oneshot(post(
                CONSUME_PATH,
                Some(ORIGIN),
                consume(&grant_a, &nonce_a),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ticket = body_json(resp).await["ticket"]
            .as_str()
            .expect("ticket")
            .to_string();

        // Nonce replay is denied.
        let resp = app
            .clone()
            .oneshot(post(
                CONSUME_PATH,
                Some(ORIGIN),
                consume(&grant_a, &nonce_a),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Ticket exchange from a foreign origin: denied, ticket survives.
        let resp = app
            .clone()
            .oneshot(post(
                EXCHANGE_PATH,
                Some(ALT_ORIGIN),
                serde_json::json!({"ticket": ticket}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // The honest exchange succeeds and hands back the session secret.
        let resp = app
            .clone()
            .oneshot(post(
                EXCHANGE_PATH,
                Some(ORIGIN),
                serde_json::json!({"ticket": ticket}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["ws_path"], VIEWER_WS_PATH);
        assert_eq!(json["rotation_generation"], 0);
        assert_eq!(
            json["current_secret"].as_str().expect("secret").len(),
            43,
            "S0 is a 256-bit base64url secret"
        );
        // Ticket replay is denied.
        let resp = app
            .clone()
            .oneshot(post(
                EXCHANGE_PATH,
                Some(ORIGIN),
                serde_json::json!({"ticket": ticket}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn oversized_json_body_is_refused_before_parsing() {
        let (state, _store, _backend) = test_state(Duration::from_secs(2));
        let app = browser_viewer_routes(state);
        let resp = app
            .oneshot(post(
                CONSUME_PATH,
                Some(ORIGIN),
                "x".repeat(JSON_BODY_MAX + 1),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    type Ws = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn spawn_ws(state: Arc<BrowserViewerRouteState>) -> String {
        let app = browser_viewer_routes(state);
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve");
        });
        format!("ws://{addr}{VIEWER_WS_PATH}")
    }

    async fn connect(url: &str, origin: &str) -> Ws {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = url.into_client_request().expect("request");
        request
            .headers_mut()
            .insert("origin", origin.parse().expect("origin header"));
        let (socket, _response) = tokio_tungstenite::connect_async(request)
            .await
            .expect("ws connect");
        socket
    }

    async fn recv_text(socket: &mut Ws) -> String {
        match socket.next().await {
            Some(Ok(TungsteniteMessage::Text(text))) => text.to_string(),
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    async fn expect_auth_error(socket: &mut Ws, code: AuthErrorCode) {
        let text = recv_text(socket).await;
        let message: ViewerAuthServerMessage =
            serde_json::from_str(&text).expect("auth error json");
        match message {
            ViewerAuthServerMessage::AuthError { error } => {
                assert_eq!(error.code, code);
                assert!(error.close, "pre-auth errors always close");
            }
            other => panic!("expected AuthError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ws_without_hello_times_out() {
        let (state, _store, _backend) = test_state(Duration::from_millis(150));
        let url = spawn_ws(state).await;
        let mut socket = connect(&url, ORIGIN).await;
        expect_auth_error(&mut socket, AuthErrorCode::Timeout).await;
    }

    #[tokio::test]
    async fn ws_rejects_malformed_binary_and_oversize_hello() {
        // Malformed JSON.
        let (state, _store, _backend) = test_state(Duration::from_secs(2));
        let url = spawn_ws(state).await;
        let mut socket = connect(&url, ORIGIN).await;
        socket
            .send(TungsteniteMessage::Text("not json".into()))
            .await
            .unwrap();
        expect_auth_error(&mut socket, AuthErrorCode::Malformed).await;

        // Binary first frame.
        let (state, _store, _backend) = test_state(Duration::from_secs(2));
        let url = spawn_ws(state).await;
        let mut socket = connect(&url, ORIGIN).await;
        socket
            .send(TungsteniteMessage::Binary(vec![1, 2, 3].into()))
            .await
            .unwrap();
        expect_auth_error(&mut socket, AuthErrorCode::Malformed).await;

        // Over the 256-byte first-frame cap.
        let (state, _store, _backend) = test_state(Duration::from_secs(2));
        let url = spawn_ws(state).await;
        let mut socket = connect(&url, ORIGIN).await;
        socket
            .send(TungsteniteMessage::Text("x".repeat(300).into()))
            .await
            .unwrap();
        expect_auth_error(&mut socket, AuthErrorCode::Malformed).await;
    }

    #[tokio::test]
    async fn ws_unknown_grant_is_unauthorized() {
        let (state, _store, _backend) = test_state(Duration::from_secs(2));
        let url = spawn_ws(state).await;
        let mut socket = connect(&url, ORIGIN).await;
        let hello = ViewerAuthClientMessage::AuthHello {
            hello: AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: "no-such-grant".into(),
                rotation_generation: 0,
                secret: "A".repeat(43),
            },
        };
        socket
            .send(TungsteniteMessage::Text(
                serde_json::to_string(&hello).unwrap().into(),
            ))
            .await
            .unwrap();
        expect_auth_error(&mut socket, AuthErrorCode::Unauthorized).await;
    }

    /// Consume + exchange over HTTP; returns (grant_id, plaintext S0).
    async fn handshake_http(app: &Router, store: &ViewerGrantStore) -> (String, String) {
        let (grant_id, nonce) = issue(store);
        let resp = app
            .clone()
            .oneshot(post(
                CONSUME_PATH,
                Some(ORIGIN),
                serde_json::json!({"grant_id": grant_id, "nonce": nonce}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ticket = body_json(resp).await["ticket"]
            .as_str()
            .unwrap()
            .to_string();
        let resp = app
            .clone()
            .oneshot(post(
                EXCHANGE_PATH,
                Some(ORIGIN),
                serde_json::json!({"ticket": ticket}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        (
            grant_id,
            json["current_secret"].as_str().unwrap().to_string(),
        )
    }

    async fn send_hello(socket: &mut Ws, grant_id: &str, generation: u64, secret: &str) {
        let hello = ViewerAuthClientMessage::AuthHello {
            hello: AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: grant_id.into(),
                rotation_generation: generation,
                secret: secret.into(),
            },
        };
        socket
            .send(TungsteniteMessage::Text(
                serde_json::to_string(&hello).unwrap().into(),
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ws_origin_mismatch_is_unauthorized() {
        let (state, store, _backend) = test_state(Duration::from_secs(2));
        let app = browser_viewer_routes(state.clone());
        let (grant_id, s0) = handshake_http(&app, &store).await;
        let url = spawn_ws(state).await;
        // The socket arrives from a loopback-but-foreign origin.
        let mut socket = connect(&url, ALT_ORIGIN).await;
        send_hello(&mut socket, &grant_id, 0, &s0).await;
        expect_auth_error(&mut socket, AuthErrorCode::Unauthorized).await;
        // The secret was not consumed: the honest origin can still auth.
        let mut socket = connect(&url, ORIGIN).await;
        send_hello(&mut socket, &grant_id, 0, &s0).await;
        let text = recv_text(&mut socket).await;
        let message: ViewerAuthServerMessage = serde_json::from_str(&text).unwrap();
        assert!(matches!(message, ViewerAuthServerMessage::AuthOk { .. }));
    }

    #[tokio::test]
    async fn ws_full_session_frames_input_and_revoke() {
        let (state, store, backend) = test_state(Duration::from_secs(2));
        let app = browser_viewer_routes(state.clone());
        let (grant_id, s0) = handshake_http(&app, &store).await;
        let url = spawn_ws(state).await;
        let mut socket = connect(&url, ORIGIN).await;
        send_hello(&mut socket, &grant_id, 0, &s0).await;

        // AuthOk: generation 1, fresh next_secret.
        let text = recv_text(&mut socket).await;
        let auth_ok = match serde_json::from_str::<ViewerAuthServerMessage>(&text).unwrap() {
            ViewerAuthServerMessage::AuthOk { ok } => ok,
            other => panic!("expected AuthOk, got {other:?}"),
        };
        assert_eq!(auth_ok.ws_generation, 1);
        assert_eq!(auth_ok.next_secret.len(), 43);
        assert_ne!(auth_ok.next_secret, s0);

        // A screencast frame arrives as a scope-checked binary; the engine's
        // ack id never reaches the wire.
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
        backend
            .hub
            .lock()
            .unwrap()
            .publish(3, jpeg.clone(), Some(123.0), 42);
        let bytes = match socket.next().await {
            Some(Ok(TungsteniteMessage::Binary(bytes))) => bytes,
            other => panic!("expected a frame, got {other:?}"),
        };
        assert!(
            !String::from_utf8_lossy(&bytes).contains("engine_ack_id"),
            "engine ack id must never reach the viewer"
        );
        let frame = ViewerWsFrame::decode_for(&bytes, "res-1", &test_binding("target-1"), 7, 3)
            .expect("frame decodes for its exact scope");
        assert_eq!(frame.jpeg_payload, jpeg);
        assert_eq!(frame.header.seq, 1);

        // Attach replays snapshot + state.
        let attach = ViewerClientMessage::Attach {
            request: AttachRequest {
                last_frame_seq: None,
            },
        };
        socket
            .send(TungsteniteMessage::Text(
                serde_json::to_string(&attach).unwrap().into(),
            ))
            .await
            .unwrap();
        let snapshot =
            match serde_json::from_str::<ViewerServerMessage>(&recv_text(&mut socket).await)
                .unwrap()
            {
                ViewerServerMessage::Snapshot { snapshot } => snapshot,
                other => panic!("expected Snapshot, got {other:?}"),
            };
        assert_eq!(snapshot.resource.resource_id, "res-1");
        let state_msg =
            match serde_json::from_str::<ViewerServerMessage>(&recv_text(&mut socket).await)
                .unwrap()
            {
                ViewerServerMessage::State { state } => state,
                other => panic!("expected State, got {other:?}"),
            };
        assert_eq!(state_msg.controller, Controller::Agent);

        // Take control, then input flows to the backend dispatch.
        let take = ViewerClientMessage::Take {
            request: ControlRequest {
                expected_control_epoch: 0,
                action_seq: 1,
                expires_at_ms: now_ms() + 60_000,
            },
        };
        socket
            .send(TungsteniteMessage::Text(
                serde_json::to_string(&take).unwrap().into(),
            ))
            .await
            .unwrap();
        let control_epoch =
            match serde_json::from_str::<ViewerServerMessage>(&recv_text(&mut socket).await)
                .unwrap()
            {
                ViewerServerMessage::ControlResult { result } => {
                    assert!(result.accepted, "take refused: {:?}", result.reason);
                    result.control_epoch
                }
                other => panic!("expected ControlResult, got {other:?}"),
            };
        // The State update after take.
        let _ = recv_text(&mut socket).await;

        let input = ViewerClientMessage::Input {
            request: aleph_protocol::browser_live::HumanInputRequest {
                connection_epoch: 7,
                stream_epoch: 3,
                control_epoch,
                seq: 1,
                expires_at_ms: now_ms() + 60_000,
                event: HumanInputEvent::Text {
                    text: "hello".into(),
                },
            },
        };
        socket
            .send(TungsteniteMessage::Text(
                serde_json::to_string(&input).unwrap().into(),
            ))
            .await
            .unwrap();
        match serde_json::from_str::<ViewerServerMessage>(&recv_text(&mut socket).await).unwrap() {
            ViewerServerMessage::InputResult { result } => {
                assert!(result.accepted, "input refused: {:?}", result.reason)
            }
            other => panic!("expected InputResult, got {other:?}"),
        }
        let dispatched = backend.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].0, "target-1");
        assert!(matches!(&dispatched[0].1, HumanInputEvent::Text { text } if text == "hello"));
        drop(dispatched);

        // Revoke closes the socket with a Closed message.
        assert!(store.revoke(&grant_id));
        let closed =
            match serde_json::from_str::<ViewerServerMessage>(&recv_text(&mut socket).await)
                .unwrap()
            {
                ViewerServerMessage::Closed { closed } => closed,
                other => panic!("expected Closed, got {other:?}"),
            };
        assert_eq!(closed.reason, ClosedReason::Revoked);
    }
}
