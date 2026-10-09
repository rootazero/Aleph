//! `/mcp` — the Streamable HTTP transport (MCP revisions 2025-03-26 … 2025-11-25).
//!
//! `POST /mcp` carries one JSON-RPC message (or, for 2025-03-26 clients, a
//! batch); a request is answered `200 application/json`, a notification
//! `202`. `GET /mcp` opens the server→client SSE stream a session uses for
//! `notifications/tools/list_changed`. `DELETE /mcp` ends a session. The
//! session id travels in `Mcp-Session-Id`, minted by `initialize`.
//!
//! Every request passes the same three guards as `/ws` and the artifact byte
//! route (trusted-proxy client resolution → plaintext-remote refusal →
//! Origin policy) and then `auth::authorize`, so a leaked session id buys
//! nothing without the bearer. Like `artifact_route`, the state is narrow on
//! purpose: nothing here can reach the connection table or the RPC registry.
//!
//! Not implemented, on purpose: JSON-RPC responses sent by the client (this
//! face makes no server→client requests, so there is nothing to answer),
//! SSE resumability (`Last-Event-ID`), and the `MCP-Protocol-Version` header
//! check (a check that only ever logged would be a check in name only).

use std::net::{IpAddr, SocketAddr};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::Value;
use tokio_stream::wrappers::ReceiverStream;

use super::auth::{authorize, AuthRefusal, McpCaller, SharedTokenValidator};
use super::protocol::{handle_message, requires_session, Outcome};
use super::session::SessionView;
use super::McpFace;
use crate::gateway::origin_policy::OriginPolicy;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_REQUEST, PARSE_ERROR};
use crate::gateway::rate_limiter::{
    RateLimitConfig, RateLimitKey, RateLimitScope, RateLimiter, WindowConfig,
};
use crate::gateway::security::{DeviceTokenManager, SecurityStore};
use crate::gateway::server::handler::refuse_insecure_remote;
use crate::gateway::trusted_proxy::resolve_client;
use crate::sync_primitives::Arc;

/// The one endpoint. Also what `packages/pi-aleph/mcp.json` points at.
pub const MCP_PATH: &str = "/mcp";
/// Header name, lower-case (HTTP headers are case-insensitive; `HeaderMap`
/// normalizes lookups).
pub const SESSION_HEADER: &str = "mcp-session-id";
/// Remote `POST /mcp` per client IP per minute. A private bucket (not the
/// gateway's) so an MCP host in a tight loop cannot stall `chat.send`, and
/// loopback-exempt like every other loopback privilege. dsh/pi issue one
/// POST per tool call plus a `tools/list` per resync; 120/min is two a
/// second, far above any agent's real cadence and far below a flood.
pub const MCP_REMOTE_POSTS_PER_MINUTE: u32 = 120;

/// Narrow state for the three handlers — see the module doc.
pub struct McpRouteState {
    face: Arc<McpFace>,
    origin_policy: Arc<OriginPolicy>,
    trusted_proxy_enabled: bool,
    trusted_proxy_ips: Vec<IpAddr>,
    allow_insecure_remote: bool,
    tls_enabled: bool,
    device_tokens: Arc<DeviceTokenManager>,
    security_store: Arc<SecurityStore>,
    validate_shared: SharedTokenValidator,
    /// See [`MCP_REMOTE_POSTS_PER_MINUTE`]. `RateLimitConfig::max_entries`
    /// bounds it without a pruning task (same as the artifact route).
    rate_limiter: RateLimiter,
}

/// What `admit` hands the handlers: who, and which bucket they draw from.
struct Admitted {
    caller: McpCaller,
    client_ip: IpAddr,
}

impl McpRouteState {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        face: Arc<McpFace>,
        origin_policy: Arc<OriginPolicy>,
        trusted_proxy_enabled: bool,
        trusted_proxy_ips: Vec<IpAddr>,
        allow_insecure_remote: bool,
        tls_enabled: bool,
        device_tokens: Arc<DeviceTokenManager>,
        security_store: Arc<SecurityStore>,
        validate_shared: SharedTokenValidator,
    ) -> Self {
        Self {
            face,
            origin_policy,
            trusted_proxy_enabled,
            trusted_proxy_ips,
            allow_insecure_remote,
            tls_enabled,
            device_tokens,
            security_store,
            validate_shared,
            rate_limiter: RateLimiter::new(RateLimitConfig {
                rpc_heavy: WindowConfig {
                    max_requests: MCP_REMOTE_POSTS_PER_MINUTE,
                    window_secs: 60,
                    lockout_secs: None,
                },
                ..RateLimitConfig::default()
            }),
        }
    }
}

/// The route, ready to `merge` into the gateway router.
pub fn mcp_routes(state: Arc<McpRouteState>) -> Router {
    Router::new()
        .route(
            MCP_PATH,
            get(handle_get).post(handle_post).delete(handle_delete),
        )
        .with_state(state)
}

/// Guards 1–3 (transport, origin) then authorization. `Err` is the refusal
/// response, ready to return.
#[allow(clippy::result_large_err)] // house shape for Result<_, Response> gates
fn admit(
    state: &McpRouteState,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> Result<Admitted, Response> {
    let resolved = resolve_client(
        peer.ip(),
        headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let secure = state.tls_enabled || resolved.secure;
    if refuse_insecure_remote(resolved.local, secure, state.allow_insecure_remote) {
        tracing::warn!(client = %resolved.ip, "refused /mcp: insecure transport to a remote client");
        return Err((
            StatusCode::UPGRADE_REQUIRED,
            "TLS required for remote MCP clients",
        )
            .into_response());
    }
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !state.origin_policy.is_allowed(origin, host) {
        tracing::warn!(client = %resolved.ip, "refused /mcp: disallowed origin");
        return Err((StatusCode::FORBIDDEN, "origin not allowed").into_response());
    }
    authorize(
        headers,
        resolved,
        &state.device_tokens,
        &state.security_store,
        &*state.validate_shared,
    )
    .map_err(|refusal| {
        tracing::info!(client = %resolved.ip, ?refusal, "refused /mcp: unauthorized");
        let why = match refusal {
            AuthRefusal::NoCredential => {
                "Authorization: Bearer <device token | gateway token> required"
            }
            AuthRefusal::BadCredential => "invalid bearer token",
            AuthRefusal::Walled => "credential is valid but its user is not active",
        };
        (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"aleph\""),
            )],
            why,
        )
            .into_response()
    })
    .map(|caller| Admitted {
        caller,
        client_ip: resolved.ip,
    })
}

/// The remote-only bucket (R6.4). `Ok(())` for loopback without touching the
/// limiter — the exemption is structural, not a zero-cost check.
#[allow(clippy::result_large_err)] // house shape for Result<_, Response> gates
fn charge_remote_post(state: &McpRouteState, admitted: &Admitted) -> Result<(), Response> {
    if admitted.caller.is_local {
        return Ok(());
    }
    let key = RateLimitKey::new(&admitted.client_ip.to_string(), RateLimitScope::RpcHeavy);
    state.rate_limiter.check_and_record(&key).map_err(|e| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, e.retry_after_secs().to_string())],
            "MCP request rate limit exceeded",
        )
            .into_response()
    })
}

/// The session named by the header, or the refusal to send.
#[allow(clippy::result_large_err)] // house shape for Result<_, Response> gates
fn session_from_header(
    state: &McpRouteState,
    headers: &HeaderMap,
) -> Result<SessionView, Response> {
    let Some(id) = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok()) else {
        return Err((StatusCode::BAD_REQUEST, "Mcp-Session-Id header required").into_response());
    };
    state.face.sessions().touch(id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "unknown or expired Mcp-Session-Id; send initialize again",
        )
            .into_response()
    })
}

fn with_session_header(mut response: Response, id: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(id) {
        response.headers_mut().insert(SESSION_HEADER, v);
    }
    response
}

fn bad_request(code: i32, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(JsonRpcResponse::error(None, code, message)),
    )
        .into_response()
}

async fn handle_post(
    State(state): State<Arc<McpRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admitted = match admit(&state, peer, &headers) {
        Ok(a) => a,
        Err(refused) => return refused,
    };
    if let Err(refused) = charge_remote_post(&state, &admitted) {
        return refused;
    }
    let caller = admitted.caller;
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return bad_request(PARSE_ERROR, &format!("Parse error: {e}")),
    };
    let (messages, batched) = match parsed {
        Value::Array(items) if items.is_empty() => {
            return bad_request(INVALID_REQUEST, "empty batch");
        }
        Value::Array(items) => (items, true),
        other => (vec![other], false),
    };

    // One session for the whole POST: the header names it, or `initialize`
    // mints it. A batch is served in order under that one session.
    let mut session: Option<SessionView> = None;
    let mut replies: Vec<JsonRpcResponse> = Vec::new();
    for item in messages {
        let msg: JsonRpcRequest = match serde_json::from_value(item) {
            Ok(m) => m,
            Err(_) if batched => {
                replies.push(JsonRpcResponse::error(
                    None,
                    INVALID_REQUEST,
                    "not a JSON-RPC message",
                ));
                continue;
            }
            Err(_) => return bad_request(INVALID_REQUEST, "not a JSON-RPC 2.0 message"),
        };
        if requires_session(&msg.method) && session.is_none() {
            match session_from_header(&state, &headers) {
                Ok(s) => session = Some(s),
                Err(refused) => return refused,
            }
        }
        match handle_message(&state.face, &caller, session.as_ref(), msg).await {
            Outcome::Reply {
                response,
                new_session,
            } => {
                if let Some(s) = new_session {
                    session = Some(s);
                }
                replies.push(response);
            }
            Outcome::Accepted => {}
        }
    }

    let response = if replies.is_empty() {
        StatusCode::ACCEPTED.into_response()
    } else if batched {
        (StatusCode::OK, Json(replies)).into_response()
    } else {
        (StatusCode::OK, Json(replies.remove(0))).into_response()
    };
    match session {
        Some(s) => with_session_header(response, &s.id),
        None => response,
    }
}

async fn handle_get(
    State(state): State<Arc<McpRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = admit(&state, peer, &headers) {
        return refused;
    }
    let session = match session_from_header(&state, &headers) {
        Ok(s) => s,
        Err(refused) => return refused,
    };
    let Some(rx) = state.face.sessions().attach_stream(&session.id) else {
        return (StatusCode::NOT_FOUND, "session vanished").into_response();
    };
    let stream =
        ReceiverStream::new(rx).map(|note| Event::default().event("message").json_data(note));
    let response = Sse::new(stream)
        .keep_alive(KeepAlive::new())
        .into_response();
    with_session_header(response, &session.id)
}

async fn handle_delete(
    State(state): State<Arc<McpRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = admit(&state, peer, &headers) {
        return refused;
    }
    let Some(id) = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok()) else {
        return (StatusCode::BAD_REQUEST, "Mcp-Session-Id header required").into_response();
    };
    if state.face.sessions().remove(id) {
        StatusCode::OK.into_response()
    } else {
        (StatusCode::NOT_FOUND, "unknown Mcp-Session-Id").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::gateway::origin_policy::OriginPolicy;
    use crate::gateway::security::{DeviceTokenManager, SecurityStore};
    use axum::body::Body;
    use axum::http::{Method, Request};
    use futures::StreamExt;
    use serde_json::json;
    use tower::ServiceExt;

    const REMOTE: [u8; 4] = [203, 0, 113, 7];
    const LOCAL: [u8; 4] = [127, 0, 0, 1];

    struct Fixture {
        app: Router,
        face: Arc<McpFace>,
    }

    fn fixture_with(expose: &[&str], allow_insecure_remote: bool) -> Fixture {
        let face = Arc::new(face(expose, true));
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let state = Arc::new(McpRouteState::new(
            face.clone(),
            Arc::new(OriginPolicy::loopback_only()),
            false,
            Vec::new(),
            allow_insecure_remote,
            false,
            Arc::new(DeviceTokenManager::new(store.clone())),
            store,
            Arc::new(|t: &str| t == "aleph-good"),
        ));
        Fixture {
            app: mcp_routes(state),
            face,
        }
    }

    fn fixture() -> Fixture {
        fixture_with(&["echo"], true)
    }

    fn request(
        method: Method,
        ip: [u8; 4],
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(MCP_PATH);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(serde_json::to_vec(&v).unwrap())
            }
            None => Body::empty(),
        };
        let mut req = b.body(body).expect("request");
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((ip, 40000))));
        req
    }

    fn init_body() -> Value {
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "pi-mcp-aleph", "version": "1.0.0"}}})
    }

    async fn json_of(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn header_of(response: &Response, name: &str) -> Option<String> {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    async fn initialize(
        fx: &Fixture,
        ip: [u8; 4],
        headers: &[(&str, &str)],
    ) -> (StatusCode, Option<String>, Value) {
        let r = fx
            .app
            .clone()
            .oneshot(request(Method::POST, ip, headers, Some(init_body())))
            .await
            .unwrap();
        let status = r.status();
        let sid = header_of(&r, SESSION_HEADER);
        let body = if status.is_success() {
            json_of(r).await
        } else {
            Value::Null
        };
        (status, sid, body)
    }

    #[tokio::test]
    async fn loopback_initialize_needs_no_bearer_and_assigns_a_session() {
        let fx = fixture();
        let (status, sid, body) = initialize(&fx, LOCAL, &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(sid.is_some());
        assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
        assert!(fx.face.sessions().touch(sid.as_deref().unwrap()).is_some());
    }

    #[tokio::test]
    async fn remote_without_a_bearer_is_401_with_www_authenticate() {
        let fx = fixture();
        let r = fx
            .app
            .clone()
            .oneshot(request(Method::POST, REMOTE, &[], Some(init_body())))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            header_of(&r, "www-authenticate").as_deref(),
            Some("Bearer realm=\"aleph\"")
        );
        assert_eq!(
            fx.face.sessions().len(),
            0,
            "a refused request mints no session"
        );
    }

    #[tokio::test]
    async fn remote_with_a_bad_bearer_is_401_and_with_the_shared_token_is_admitted() {
        let fx = fixture();
        let (status, _, _) =
            initialize(&fx, REMOTE, &[("authorization", "Bearer aleph-nope")]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, sid, _) =
            initialize(&fx, REMOTE, &[("authorization", "Bearer aleph-good")]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(sid.is_some());
    }

    #[tokio::test]
    async fn a_plaintext_remote_is_426_when_not_allowed() {
        let fx = fixture_with(&["echo"], false);
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                REMOTE,
                &[("authorization", "Bearer aleph-good")],
                Some(init_body()),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UPGRADE_REQUIRED);
    }

    #[tokio::test]
    async fn remote_posts_are_rate_limited_and_loopback_is_exempt() {
        let fx = fixture();
        let auth = [("authorization", "Bearer aleph-good")];
        // N admitted, the N+1th refused with Retry-After.
        let mut limited = None;
        for _ in 0..=MCP_REMOTE_POSTS_PER_MINUTE {
            let r = fx
                .app
                .clone()
                .oneshot(request(Method::POST, REMOTE, &auth, Some(init_body())))
                .await
                .unwrap();
            if r.status() == StatusCode::TOO_MANY_REQUESTS {
                limited = Some(r);
                break;
            }
            assert_eq!(r.status(), StatusCode::OK);
        }
        let r = limited.expect("the bucket must close on the N+1th remote request");
        assert!(header_of(&r, "retry-after").is_some());
        // Loopback never pays: the same count and one more, all 200.
        for _ in 0..=MCP_REMOTE_POSTS_PER_MINUTE {
            let r = fx
                .app
                .clone()
                .oneshot(request(Method::POST, LOCAL, &[], Some(init_body())))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn a_cross_origin_request_is_403() {
        let fx = fixture();
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[
                    ("origin", "https://evil.example"),
                    ("host", "127.0.0.1:18790"),
                ],
                Some(init_body()),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_missing_session_header_is_400_and_an_unknown_one_is_404() {
        let fx = fixture();
        let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
        let r = fx
            .app
            .clone()
            .oneshot(request(Method::POST, LOCAL, &[], Some(list.clone())))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[(SESSION_HEADER, "not-a-session")],
                Some(list),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_request_in_a_live_session_is_200_json_and_echoes_the_session() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let call = json!({"jsonrpc": "2.0", "id": "c-1", "method": "tools/call", "params": {"name": "echo", "arguments": {}}});
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[(SESSION_HEADER, &sid)],
                Some(call),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(header_of(&r, "content-type")
            .unwrap()
            .starts_with("application/json"));
        assert_eq!(header_of(&r, SESSION_HEADER).as_deref(), Some(sid.as_str()));
        let body = json_of(r).await;
        assert_eq!(body["id"], "c-1");
        assert_eq!(body["result"]["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn a_notification_is_202_with_an_empty_body() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[(SESSION_HEADER, &sid)],
                Some(note),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
        assert!(bytes.is_empty());
        assert!(fx.face.sessions().touch(&sid).unwrap().initialized);
    }

    #[tokio::test]
    async fn a_malformed_body_is_400_with_a_jsonrpc_error() {
        let fx = fixture();
        let mut req = request(Method::POST, LOCAL, &[], None);
        *req.body_mut() = Body::from("{not json");
        let r = fx.app.clone().oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let body = json_of(r).await;
        assert_eq!(body["error"]["code"], crate::gateway::protocol::PARSE_ERROR);
        assert_eq!(body["id"], Value::Null);

        // Valid JSON that is not a JSON-RPC message.
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[],
                Some(json!({"hello": "world"})),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json_of(r).await["error"]["code"],
            crate::gateway::protocol::INVALID_REQUEST
        );
    }

    #[tokio::test]
    async fn a_batch_from_a_2025_03_26_client_is_answered_as_an_array() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let batch = json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}
        ]);
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[(SESSION_HEADER, &sid)],
                Some(batch),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = json_of(r).await;
        let arr = body.as_array().expect("array");
        assert_eq!(arr.len(), 2, "the notification produces no entry");
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[1]["id"], 2);
        // An all-notification batch is 202.
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[(SESSION_HEADER, &sid)],
                Some(json!([{"jsonrpc":"2.0","method":"notifications/initialized"}])),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
        // An empty batch is a bad request.
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                LOCAL,
                &[(SESSION_HEADER, &sid)],
                Some(json!([])),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_ends_the_session_and_a_second_delete_is_404() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::DELETE,
                LOCAL,
                &[(SESSION_HEADER, &sid)],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::DELETE,
                LOCAL,
                &[(SESSION_HEADER, &sid)],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = fx
            .app
            .clone()
            .oneshot(request(Method::DELETE, LOCAL, &[], None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_opens_an_sse_stream_that_carries_list_changed() {
        let fx = fixture();
        let (_, sid, _) = initialize(&fx, LOCAL, &[]).await;
        let sid = sid.unwrap();
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::GET,
                LOCAL,
                &[(SESSION_HEADER, &sid), ("accept", "text/event-stream")],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(header_of(&r, "content-type")
            .unwrap()
            .starts_with("text/event-stream"));
        assert_eq!(header_of(&r, SESSION_HEADER).as_deref(), Some(sid.as_str()));

        let mut frames = r.into_body().into_data_stream();
        fx.face.notify_tools_list_changed();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
            .await
            .expect("a frame arrives within 2s")
            .expect("stream open")
            .expect("bytes");
        let text = String::from_utf8(first.to_vec()).unwrap();
        assert!(text.contains("event: message"), "{text}");
        assert!(text.contains("notifications/tools/list_changed"), "{text}");

        // An unknown session cannot open a stream.
        let r = fx
            .app
            .clone()
            .oneshot(request(
                Method::GET,
                LOCAL,
                &[(SESSION_HEADER, "nope")],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
}
