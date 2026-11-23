//! Native-TLS browser ingress. The Cookie references an existing Core device credential.

use std::{collections::HashMap, net::SocketAddr, sync::Mutex, time::Duration};

use aleph_client::local_ipc::parse_payload;
use aleph_protocol::{
    framing::{encode_header, split_frame, HEADER_BYTES},
    system_hello::HelloLimits,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, DefaultBodyLimit, FromRequest, OriginalUri, Request, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tokio::{
    sync::Semaphore,
    time::{timeout, timeout_at, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    cleanup,
    dispatch::{CallerSnapshot, DispatchPeer},
    rpc, ConnectionContext,
};
use crate::{
    cluster::PendingInvokes,
    gateway::{
        handlers::system_hello,
        lane::ChannelClass,
        protocol::{
            JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, INTERNAL_ERROR, METHOD_NOT_FOUND,
        },
        rate_limiter::{RateLimitKey, RateLimitScope},
        security::{store::SecurityStore, DeviceTokenManager},
        server::{ConnectionState, GatewaySharedState},
    },
    sync_primitives::Arc,
};

const COOKIE: &str = "__Host-aleph-v1";
const SUBPROTOCOL: &str = "aleph.protocol.v1";
const TTL: Duration = Duration::from_secs(300);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MESSAGE: u32 = 1024 * 1024;
const MAX_SESSIONS: usize = 256;
const MAX_SOCKETS: usize = 8;

struct WebIngress {
    gateway: Arc<GatewaySharedState>,
    sessions: Mutex<HashMap<Uuid, WebBinding>>,
    sockets: std::sync::Arc<Semaphore>,
    stop: CancellationToken,
}

#[derive(Clone)]
pub(super) struct WebBinding {
    store: Arc<SecurityStore>,
    origin: String,
    token_id: String,
    device_id: String,
    user: String,
    expires: Instant,
}

impl WebBinding {
    pub(super) fn current_role(&self) -> Option<&'static str> {
        if self.expires <= Instant::now() {
            return None;
        }
        let (user, role) = self
            .store
            .active_device_principal(&self.token_id, &self.device_id)
            .ok()??;
        (self.user == user).then_some(role.wire_role())
    }

    fn snapshot(&self, role: &'static str) -> CallerSnapshot {
        CallerSnapshot {
            role: Some(role.to_owned()),
            user: Some(self.user.clone()),
            verified_user: Some(self.user.clone()),
            device_id: Some(self.device_id.clone()),
        }
    }
}

pub(in crate::gateway::server) fn routes(
    gateway: Arc<GatewaySharedState>,
    stop: CancellationToken,
) -> Router {
    Router::new()
        .route(
            "/v1/session",
            post(exchange).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/v1/ws", get(upgrade))
        .with_state(Arc::new(WebIngress {
            gateway,
            sessions: Mutex::new(HashMap::new()),
            sockets: std::sync::Arc::new(Semaphore::new(MAX_SOCKETS)),
            stop,
        }))
}

fn one_header(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn same_origin<'a>(headers: &'a HeaderMap, uri: &Uri) -> Option<&'a str> {
    if uri.query().is_some() {
        return None;
    }
    let origin = one_header(headers, header::ORIGIN)?;
    let host = one_header(headers, header::HOST);
    if headers.contains_key(header::HOST) && host.is_none() {
        return None;
    }
    let authority = uri.authority().map(|authority| authority.as_str());
    if let (Some(host), Some(authority)) = (host, authority) {
        if !host.eq_ignore_ascii_case(authority) {
            return None;
        }
    }
    let authority = authority.or(host)?;
    // A URL's authority must not be interpreted as a path or username.
    if authority.contains(['@', '/', '\\', '?', '#']) {
        return None;
    }
    let expected = url::Url::parse(&format!("https://{authority}/")).ok()?;
    (expected.origin().ascii_serialization() == origin).then_some(origin)
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Credentials {
    DeviceToken(String),
    BootstrapTicket(String),
}

impl Credentials {
    fn redeem(
        self,
        manager: &DeviceTokenManager,
    ) -> Result<crate::gateway::security::store::DeviceTokenRow, StatusCode> {
        let token = match self {
            Self::DeviceToken(token) if !token.is_empty() => token,
            Self::BootstrapTicket(ticket) if !ticket.is_empty() => {
                manager
                    .exchange_principal_bound_bootstrap_ticket(&ticket)
                    .map_err(|_| StatusCode::UNAUTHORIZED)?
                    .device_token
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        manager
            .validate_device_token(&token)
            .ok()
            .flatten()
            .ok_or(StatusCode::UNAUTHORIZED)
    }
}

fn throttle(state: &GatewaySharedState, addr: SocketAddr) -> Result<(), StatusCode> {
    let key = RateLimitKey::new(&format!("web-v1:{}", addr.ip()), RateLimitScope::Auth);
    state
        .rate_limiter
        .check_and_record(&key)
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)
}

async fn exchange(
    State(state): State<Arc<WebIngress>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    request: Request,
) -> Result<impl IntoResponse, StatusCode> {
    let origin = same_origin(request.headers(), &uri)
        .ok_or(StatusCode::FORBIDDEN)?
        .to_owned();
    throttle(&state.gateway, addr)?;
    let Json(credentials) = timeout(
        IO_TIMEOUT,
        Json::<Credentials>::from_request(request, &state),
    )
    .await
    .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
    .map_err(|error| match error.status() {
        StatusCode::UNPROCESSABLE_ENTITY => StatusCode::BAD_REQUEST,
        status => status,
    })?;
    let gateway = &state.gateway;
    let store = gateway
        .security_store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let manager = gateway
        .device_token_mgr
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    // Reserve capacity before redeeming a one-shot pairing ticket. No await
    // occurs with this guard held; verification always consults Core separately.
    let mut sessions = state
        .sessions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    sessions.retain(|_, binding| binding.expires > Instant::now());
    if sessions.len() >= MAX_SESSIONS {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let token = credentials.redeem(manager)?;
    let (user, _) = store
        .active_device_principal(&token.token_id, &token.device_id)
        .ok()
        .flatten()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let handle = Uuid::new_v4();
    sessions.insert(
        handle,
        WebBinding {
            store: store.clone(),
            origin,
            token_id: token.token_id,
            device_id: token.device_id,
            user,
            expires: Instant::now() + TTL,
        },
    );
    let cookie = HeaderValue::from_str(&format!(
        "{COOKIE}={handle}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        TTL.as_secs()
    ))
    .expect("static attributes and UUID");
    Ok((
        StatusCode::NO_CONTENT,
        [
            (header::SET_COOKIE, cookie),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    ))
}

fn cookie_handle(headers: &HeaderMap) -> Option<Uuid> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        for raw in value.to_str().ok()?.split(';') {
            if let Some((name, value)) = raw.trim().split_once('=') {
                if name == COOKIE {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(value.parse().ok()?);
                }
            }
        }
    }
    found
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<WebIngress>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let origin = same_origin(&headers, &uri).ok_or(StatusCode::FORBIDDEN)?;
    throttle(&state.gateway, addr)?;
    if one_header(&headers, header::SEC_WEBSOCKET_PROTOCOL) != Some(SUBPROTOCOL) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = cookie_handle(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let binding = state
        .sessions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&id)
        .cloned()
        .filter(|binding| binding.origin == origin)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    binding.current_role().ok_or(StatusCode::UNAUTHORIZED)?;
    let permit = state
        .sockets
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let conn_id = format!("web-v1-{}", Uuid::new_v4());
    let reservation = {
        let mut connections = state.gateway.connections.write().await;
        if state.stop.is_cancelled() || at_capacity(&connections, &state.gateway, addr) {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        binding.current_role().ok_or(StatusCode::UNAUTHORIZED)?;
        connections.insert(conn_id.clone(), ConnectionState::new(addr.ip(), false));
        PendingAdmission {
            gateway: state.gateway.clone(),
            conn_id: Some(conn_id),
        }
    };
    let wire_limit = MAX_MESSAGE as usize + HEADER_BYTES;
    Ok(ws
        .protocols([SUBPROTOCOL])
        .max_message_size(wire_limit)
        .max_frame_size(wire_limit)
        .on_upgrade(move |mut socket| async move {
            let _permit = permit;
            let mut reservation = reservation;
            serve(
                &mut socket,
                &state.gateway,
                binding,
                addr,
                &state.stop,
                &mut reservation,
            )
            .await;
            // Finish normal teardown before the peer observes socket closure.
            reservation.release().await;
        }))
}

// Axum drops the callback without invoking it if the transport upgrade fails.
// The callback owns the reservation through handshake and normal cleanup.
struct PendingAdmission {
    gateway: Arc<GatewaySharedState>,
    conn_id: Option<String>,
}

impl PendingAdmission {
    async fn release(&mut self) {
        if let Some(conn_id) = &self.conn_id {
            self.gateway.connections.write().await.remove(conn_id);
            self.conn_id = None;
        }
    }
}

impl Drop for PendingAdmission {
    fn drop(&mut self) {
        if let Some(conn_id) = self.conn_id.take() {
            let gateway = self.gateway.clone();
            tokio::spawn(async move {
                gateway.connections.write().await.remove(&conn_id);
            });
        }
    }
}

fn at_capacity(
    connections: &HashMap<String, ConnectionState>,
    gateway: &GatewaySharedState,
    addr: SocketAddr,
) -> bool {
    connections.len() >= gateway.max_connections
        || (gateway.max_connections_per_ip != 0
            && connections
                .values()
                .filter(|row| row.client_ip == addr.ip())
                .count()
                >= gateway.max_connections_per_ip)
}

async fn serve(
    socket: &mut WebSocket,
    gateway: &GatewaySharedState,
    binding: WebBinding,
    addr: SocketAddr,
    stop: &CancellationToken,
    reservation: &mut PendingAdmission,
) {
    let first = tokio::select! {
        biased;
        _ = stop.cancelled() => return,
        first = timeout_at(binding.expires.min(Instant::now() + IO_TIMEOUT), read(socket)) => first,
    };
    let Some((_, request)) = first.ok().flatten() else {
        return;
    };
    let conn_id = reservation
        .conn_id
        .as_ref()
        .expect("reserved before upgrade")
        .clone();
    let hello = if request.method == "system.hello" {
        system_hello::negotiate(
            request,
            conn_id.clone(),
            HelloLimits {
                max_message_bytes: u64::from(MAX_MESSAGE),
                max_inflight_requests: 1,
            },
        )
    } else {
        JsonRpcResponse::error(request.id, AUTH_REQUIRED, "system.hello must be first")
    };
    if hello.error.is_some() {
        let _ = send_response(socket, &hello).await;
        return;
    }
    let expected = {
        let mut connections = gateway.connections.write().await;
        if stop.is_cancelled() {
            return;
        }
        let (Some(role), Some(row)) = (binding.current_role(), connections.get_mut(&conn_id))
        else {
            return;
        };
        let expected = binding.snapshot(role);
        row.first_message = false;
        row.caller_user = expected.user.clone();
        row.verified_user = expected.verified_user.clone();
        row.device_id = expected.device_id.clone();
        row.caller_role = role.to_owned();
        expected
    };
    let ctx = ConnectionContext::from_shared(gateway, ChannelClass::Bot, addr.ip(), false);
    let expires = binding.expires;
    let peer = DispatchPeer::web(
        gateway.connections.clone(),
        conn_id.clone(),
        expected,
        binding,
    );
    let executor = rpc::RpcExecutor {
        middleware: ctx.middleware_chain.clone(),
        lanes: ctx.lane_manager.clone(),
        idempotency: ctx.idempotency_guard.clone(),
        class: ctx.channel_class,
    };
    let traffic = async {
        send_response(socket, &hello).await?;
        while let Some((text, request)) = read(socket).await {
            let Some(caller) = peer.snapshot().await else {
                break;
            };
            if !matches!(request.method.as_str(), "health" | "sessions.list") {
                // Rejected traffic doesn't consume legacy RPC or auth buckets.
                let key = RateLimitKey::new(
                    &format!("web-v1-unsupported:{}", addr.ip()),
                    RateLimitScope::Auth,
                );
                if gateway.rate_limiter.check_and_record(&key).is_err() {
                    break;
                }
                let rejection = JsonRpcResponse::error(
                    request.id,
                    METHOD_NOT_FOUND,
                    "method unavailable on Web v1",
                );
                send_response(socket, &rejection).await?;
                continue;
            }
            if let Err(rejection) =
                rpc::check_origin_rate(&gateway.rate_limiter, Some(addr.ip()), &request)
            {
                send_response(socket, &rejection).await?;
                continue;
            }
            match rpc::prepare(&request, &caller, gateway.require_idempotency_key) {
                Ok(plan) => {
                    let response = executor
                        .execute(&text, &request, plan.without_cache(), &peer, &caller)
                        .await;
                    if peer.snapshot().await.is_none() {
                        break;
                    }
                    send(socket, &response, request.id, INTERNAL_ERROR).await?;
                }
                Err(error) => send_response(socket, &error).await?,
            }
        }
        Some(())
    };
    let revoked = async {
        let mut ticks = tokio::time::interval(Duration::from_secs(1));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if peer.snapshot().await.is_none() {
                break;
            }
        }
    };
    tokio::select! {
        biased;
        _ = stop.cancelled() => {},
        _ = revoked => {},
        _ = timeout_at(expires, traffic) => {},
    }
    cleanup::run_cleanup(&conn_id, &ctx, &Arc::new(PendingInvokes::new())).await;
    reservation.conn_id = None;
}

async fn read(socket: &mut WebSocket) -> Option<(String, JsonRpcRequest)> {
    loop {
        match socket.recv().await?.ok()? {
            Message::Binary(bytes) => {
                let (text, tail) = split_frame(&bytes, MAX_MESSAGE).ok()??;
                if !tail.is_empty() {
                    return None;
                }
                let request: JsonRpcRequest =
                    serde_json::from_value(parse_payload(text).ok()?).ok()?;
                if request.jsonrpc != "2.0"
                    || !request
                        .id
                        .as_ref()
                        .is_some_and(|id| id.is_string() || id.is_i64() || id.is_u64())
                {
                    return None;
                }
                return Some((text.to_owned(), request));
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) | Message::Text(_) => return None,
        }
    }
}

async fn send_response(socket: &mut WebSocket, response: &JsonRpcResponse) -> Option<()> {
    let text = serde_json::to_string(response).ok()?;
    let code = response
        .error
        .as_ref()
        .map_or(INTERNAL_ERROR, |error| error.code);
    send(socket, &text, response.id.clone(), code).await
}

async fn send(
    socket: &mut WebSocket,
    text: &str,
    id: Option<serde_json::Value>,
    code: i32,
) -> Option<()> {
    let fallback;
    let text = if text.len() > MAX_MESSAGE as usize || parse_payload(text).is_err() {
        fallback = serde_json::to_string(&JsonRpcResponse::error(
            id,
            code,
            "response exceeds Web v1 limits",
        ))
        .ok()?;
        &fallback
    } else {
        text
    };
    let prefix = encode_header(text.len(), MAX_MESSAGE).ok()?;
    let mut wire = Vec::with_capacity(HEADER_BYTES + text.len());
    wire.extend_from_slice(&prefix);
    wire.extend_from_slice(text.as_bytes());
    timeout(IO_TIMEOUT, socket.send(Message::Binary(wire.into())))
        .await
        .ok()?
        .ok()
}

#[cfg(test)]
mod tests;
