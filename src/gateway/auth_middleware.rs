//! HTTP Authentication Middleware and Login Routes
//!
//! Provides session-cookie-based auth for Panel UI and
//! Bearer token auth for /v1/* API routes.

use std::net::SocketAddr;

use crate::gateway::config::AuthMode;
use crate::gateway::protocol::JsonRpcRequest;
use crate::gateway::security::SharedTokenManager;
use crate::gateway::session::HttpSessionManager;
use crate::sync_primitives::Arc;
use axum::{
    body::Body,
    extract::{ConnectInfo, Query, State},
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

/// Shared state for auth middleware
pub struct AuthState {
    pub shared_token_mgr: Arc<SharedTokenManager>,
    pub session_mgr: Arc<HttpSessionManager>,
    pub auth_mode: AuthMode,
    /// Issues + consumes one-shot bootstrap nonces for the loopback
    /// cookie-handoff endpoint. Shared with [`super::handlers::auth::AuthContext`]
    /// so the JSON-RPC issuer and the HTTP consumer see the same map.
    pub bootstrap_mgr: Arc<crate::gateway::bootstrap::BootstrapNonceManager>,
    /// Browser-pairing manager. Shared with [`super::handlers::auth::AuthContext`]
    /// so `pairing.approve` (Browser arm) can stash a session_id keyed by
    /// the pairing code that `/auth/bootstrap/from_pairing` then trades
    /// for a session cookie.
    pub pairing_mgr: Arc<crate::gateway::security::PairingManager>,
    /// Shared AuthContext so the anonymous `/rpc` HTTP route (scoped to
    /// pairing.start_browser + pairing.poll only) can reach the same
    /// handler functions the WebSocket dispatcher uses. `None` when
    /// `auth_routes` is built in isolation (e.g. legacy bootstrap-only
    /// tests); the production wiring always plumbs it.
    pub auth_ctx: Option<Arc<crate::gateway::handlers::auth::AuthContext>>,
}

/// Build auth routes (no auth required for these).
///
/// Mount with `into_make_service_with_connect_info::<SocketAddr>()` so the
/// loopback bootstrap consumer sees the real peer address.
///
/// Phase 4 deletion: the legacy `/login` + `/auth/login` token-paste form
/// has been removed. `/pair` (Phase 3 browser-pairing) is the only inbound
/// auth surface for cold browsers; `/auth/bootstrap` (Phase 2) handles
/// loopback handoff from the desktop shell.
pub fn auth_routes(state: Arc<AuthState>) -> Router {
    Router::new()
        .route("/auth/logout", post(handle_logout))
        .route("/auth/bootstrap", get(handle_bootstrap_consume))
        .route("/pair", get(show_pair_page))
        .route(
            "/auth/bootstrap/from_pairing",
            get(handle_bootstrap_from_pairing),
        )
        // Tiny anonymous JSON-RPC HTTP endpoint scoped to the two browser-
        // pairing methods. The gateway's primary RPC surface is WebSocket
        // (/ws), but the cold `/pair` page can't open a WS before it has
        // a token; this is the smallest possible bridge.
        .route("/rpc", post(handle_anonymous_rpc))
        .with_state(state)
}

/// True when the peer is on a loopback interface (127.0.0.0/8 or ::1).
/// Used by the bootstrap-consume endpoint to refuse non-local peers
/// regardless of the `Origin` header.
pub fn is_loopback_peer(addr: &SocketAddr) -> bool {
    match addr.ip() {
        std::net::IpAddr::V4(v4) => v4.is_loopback(),
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

#[derive(Deserialize)]
struct BootstrapQuery {
    nonce: String,
}

/// Consume a one-shot bootstrap nonce and 303-redirect to `/` with a
/// fresh `aleph_session` cookie. Refuses any non-loopback peer with
/// 403; refuses invalid / expired / replayed nonces with 401.
///
/// Pairs with the JSON-RPC issuer `gateway.bootstrap.issue`: same
/// [`crate::gateway::bootstrap::BootstrapNonceManager`] sits behind
/// both.
async fn handle_bootstrap_consume(
    State(state): State<Arc<AuthState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(q): Query<BootstrapQuery>,
) -> Response {
    if !is_loopback_peer(&peer) {
        tracing::warn!(
            peer = %peer,
            "rejected non-loopback /auth/bootstrap request"
        );
        return (StatusCode::FORBIDDEN, "loopback only").into_response();
    }
    if let Err(e) = state.bootstrap_mgr.consume(&q.nonce) {
        tracing::info!(error = %e, "bootstrap nonce rejected");
        return (StatusCode::UNAUTHORIZED, "invalid or expired nonce").into_response();
    }
    // Fabricate a fresh session keyed by the shared-token HMAC.
    let token = match state.shared_token_mgr.get_current_token() {
        Some(t) => t,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "no shared token available",
            )
                .into_response();
        }
    };
    let hash = crate::gateway::security::hmac_sign(state.shared_token_mgr.secret(), &token);
    let session_id = match state.session_mgr.create_session(&hash) {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "session creation failed").into_response();
        }
    };
    let max_age = state.session_mgr.expiry_hours() * 3600;
    let cookie = format!(
        "aleph_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        session_id, max_age,
    );
    tracing::info!("bootstrap nonce consumed; session cookie issued");
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/".to_string()),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}

async fn handle_logout(State(state): State<Arc<AuthState>>) -> Response {
    // Clear the cookie; session will expire naturally. Redirect to the
    // browser-pairing page so an unauthenticated user sees the new UX
    // (the legacy `/login` token-paste form was removed in Phase 4).
    let _ = &state;
    let cookie = "aleph_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/pair".to_string()),
            (header::SET_COOKIE, cookie.to_string()),
        ],
    )
        .into_response()
}

#[derive(Deserialize)]
struct PairQuery {
    code: Option<String>,
}

async fn show_pair_page(Query(q): Query<PairQuery>) -> Html<String> {
    Html(pair_page_html(q.code.as_deref()))
}

#[derive(Deserialize)]
struct BootstrapFromPairingQuery {
    code: String,
}

/// Trade an approved Browser pairing code for a session cookie + 303 to /.
///
/// The corresponding `pairing.approve` call deposited a session_id into
/// `PairingManager.approved_browser_sessions[code]`. This single-use
/// fetch removes it; a second request for the same code receives 401.
///
/// Unlike `/auth/bootstrap`, this is NOT loopback-gated — the whole
/// browser-pairing flow exists precisely so a remote LAN browser (mobile,
/// other laptop) can pair. The security boundary is the in-Panel
/// operator's 1-click approve.
async fn handle_bootstrap_from_pairing(
    State(state): State<Arc<AuthState>>,
    Query(q): Query<BootstrapFromPairingQuery>,
) -> Response {
    let session_id = match state.pairing_mgr.fetch_browser_session(&q.code) {
        Some(id) => id,
        None => {
            tracing::info!(code = %q.code, "pairing not approved or expired");
            return (StatusCode::UNAUTHORIZED, "pairing not approved").into_response();
        }
    };
    let max_age = state.session_mgr.expiry_hours() * 3600;
    let cookie = format!(
        "aleph_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        session_id, max_age,
    );
    tracing::info!(code = %q.code, "browser pairing cookie issued");
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/".to_string()),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}

/// Tiny anonymous JSON-RPC HTTP endpoint scoped exclusively to the two
/// browser-pairing methods (`pairing.start_browser`, `pairing.poll`). The
/// gateway's main RPC surface is WebSocket-only; this endpoint exists so
/// the cold `/pair` HTML page can do `fetch('/rpc', ...)` before it has a
/// token to open a WS with.
///
/// Other methods receive `-32601 method not allowed via /rpc` — the WS
/// endpoint enforces the full auth gate; we don't even try to mirror it
/// here.
async fn handle_anonymous_rpc(
    State(state): State<Arc<AuthState>>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    let ctx = match &state.auth_ctx {
        Some(ctx) => ctx.clone(),
        None => {
            tracing::error!("/rpc reached without auth_ctx wired into AuthState");
            return (StatusCode::INTERNAL_SERVER_ERROR, "rpc not available").into_response();
        }
    };
    let response = match req.method.as_str() {
        "pairing.start_browser" => {
            crate::gateway::handlers::auth::handle_pairing_start_browser(req, ctx).await
        }
        "pairing.poll" => crate::gateway::handlers::auth::handle_pairing_poll(req, ctx).await,
        other => crate::gateway::protocol::JsonRpcResponse::error(
            req.id.clone(),
            -32601,
            format!("method '{}' not allowed via /rpc; use /ws", other),
        ),
    };
    Json(response).into_response()
}

/// Generate the cold-browser pairing HTML page.
///
/// Renders a 6-digit code container, polls `pairing.poll` every 2s via
/// the `/rpc` HTTP endpoint, and redirects to
/// `/auth/bootstrap/from_pairing?code=…` once approved. No build step,
/// no framework — single-file vanilla HTML/JS so the page works even
/// from a fresh browser with no extensions.
pub fn pair_page_html(prefilled_code: Option<&str>) -> String {
    // Strip single quotes from the prefill to keep the inline JS literal
    // safe even if a malicious query string slips through.
    let prefill_js = match prefilled_code {
        Some(c) => format!("var prefilled = '{}';", c.replace('\'', "")),
        None => "var prefilled = null;".to_string(),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Aleph — Pair this browser</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#0a0a0f;color:#e0e0e0;display:flex;justify-content:center;align-items:center;min-height:100vh;padding:16px}}
.c{{background:#14141f;border:1px solid #2a2a3a;border-radius:16px;padding:40px;max-width:480px;width:100%;text-align:center}}
h1{{font-size:22px;margin-bottom:8px}}
.s{{color:#888;font-size:14px;margin-bottom:16px;line-height:1.5}}
.code{{font-family:'SF Mono',Menlo,monospace;font-size:48px;letter-spacing:8px;color:#a5b4fc;background:#0a0a0f;border-radius:12px;padding:24px;margin:24px 0}}
.err{{color:#fca5a5;margin-top:16px;font-size:14px}}
</style></head><body>
<div class="c">
<h1>Pair this browser</h1>
<p class="s">Open the Aleph desktop app and approve this code from the notification bell.</p>
<div class="code" id="code">…</div>
<div class="s" id="status">Waiting for approval…</div>
<div class="err" id="err"></div>
<script>
{prefill_js}
function rpc(method, params) {{
    return fetch('/rpc', {{
        method: 'POST',
        headers: {{'Content-Type': 'application/json'}},
        body: JSON.stringify({{jsonrpc: '2.0', id: Date.now(), method: method, params: params}})
    }}).then(function(r) {{ return r.json(); }});
}}
function showErr(msg) {{ document.getElementById('err').textContent = msg; }}
function start() {{
    return rpc('pairing.start_browser', {{
        origin_label: 'Browser on this network',
        user_agent: navigator.userAgent,
        peer_ip: 'self'
    }}).then(function(r) {{
        if (r.error) {{ showErr(r.error.message); return null; }}
        var code = r.result.code;
        document.getElementById('code').textContent = code;
        return code;
    }});
}}
function poll(code) {{
    setTimeout(function() {{
        rpc('pairing.poll', {{code: code}}).then(function(r) {{
            if (r.error) {{ showErr(r.error.message); return; }}
            var s = r.result.status;
            if (s === 'approved') {{
                window.location.href = '/auth/bootstrap/from_pairing?code=' + encodeURIComponent(code);
            }} else if (s === 'rejected') {{
                document.getElementById('status').textContent = 'Pairing rejected';
            }} else if (s === 'expired') {{
                document.getElementById('status').textContent = 'Pairing expired — refresh to try again';
            }} else {{
                poll(code);
            }}
        }}).catch(function(e) {{ showErr('Network error: ' + e); }});
    }}, 2000);
}}
if (prefilled) {{
    document.getElementById('code').textContent = prefilled;
    poll(prefilled);
}} else {{
    start().then(function(code) {{ if (code) poll(code); }});
}}
</script>
</div></body></html>"#
    )
}

/// Extract session ID from Cookie header
fn extract_session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
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

/// Session cookie middleware for Panel UI routes
pub async fn session_auth_middleware(
    State(state): State<Arc<AuthState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if matches!(state.auth_mode, AuthMode::None) {
        return next.run(request).await;
    }

    match extract_session_cookie(request.headers()) {
        Some(id) if state.session_mgr.validate_session(&id).unwrap_or(false) => {
            next.run(request).await
        }
        // Cold visit → browser-pairing flow (Phase 3). The legacy `/login`
        // token-paste form was removed in Phase 4; `/pair` is the only
        // entry point for an unauthenticated browser.
        _ => Redirect::to("/pair").into_response(),
    }
}

/// Bearer token middleware for API routes (/v1/*, /a2a/*)
pub async fn bearer_auth_middleware(
    State(state): State<Arc<AuthState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if matches!(state.auth_mode, AuthMode::None) {
        return next.run(request).await;
    }

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header_val) => {
            if let Some(token) = crate::gateway::openai_api::auth::extract_bearer_token(header_val)
            {
                if state.shared_token_mgr.validate(token).unwrap_or(false) {
                    return next.run(request).await;
                }
            }
            (StatusCode::UNAUTHORIZED, "Invalid token").into_response()
        }
        None => (StatusCode::UNAUTHORIZED, "Authorization header required").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_session_cookie_found() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "aleph_session=abc123; other=val".parse().unwrap(),
        );
        assert_eq!(extract_session_cookie(&headers), Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_session_cookie_missing() {
        let headers = axum::http::HeaderMap::new();
        assert_eq!(extract_session_cookie(&headers), None);
    }

    #[test]
    fn test_extract_session_cookie_no_match() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::COOKIE, "other=val; foo=bar".parse().unwrap());
        assert_eq!(extract_session_cookie(&headers), None);
    }

    #[test]
    fn pair_page_contains_friendly_copy_and_rpc_calls() {
        let html = pair_page_html(None);
        // User-visible copy
        assert!(html.contains("Pair this browser"));
        // The two anonymous RPC methods the page issues
        assert!(html.contains("pairing.start_browser"));
        assert!(html.contains("pairing.poll"));
        // Redirect target after approval
        assert!(html.contains("/auth/bootstrap/from_pairing"));
    }

    #[test]
    fn pair_page_prefills_code_when_provided() {
        let html = pair_page_html(Some("654321"));
        assert!(html.contains("654321"));
        assert!(html.contains("var prefilled = '654321';"));
    }

    #[test]
    fn pair_page_sanitises_single_quotes_in_prefill() {
        // Even a hostile query string can't inject JS via the prefill.
        let html = pair_page_html(Some("abc'); alert('xss"));
        assert!(!html.contains("alert('xss"));
    }

    /// Phase 4 deletion: `/login` and `/auth/login` no longer exist; the
    /// `/pair` page (Phase 3) is the only inbound auth surface for
    /// cold browsers.
    #[tokio::test]
    async fn login_route_returns_404_after_phase4_deletion() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state();
        let app = auth_routes(state);

        let resp_get = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp_get.status(),
            axum::http::StatusCode::NOT_FOUND,
            "GET /login must be removed after Phase 4"
        );

        let resp_post = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("token=anything"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp_post.status(),
            axum::http::StatusCode::NOT_FOUND,
            "POST /auth/login must be removed after Phase 4"
        );
    }

    /// Build a minimal AuthState backed by in-memory stores for route tests.
    fn test_state() -> Arc<AuthState> {
        use crate::gateway::bootstrap::BootstrapNonceManager;
        use crate::gateway::security::{PairingManager, SecurityStore, SharedTokenManager};
        use crate::gateway::session::HttpSessionManager;

        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let shared = Arc::new(SharedTokenManager::new(
            store.clone(),
            "/tmp/aleph_phase4.vault",
        ));
        let session_mgr = Arc::new(HttpSessionManager::new(store.clone(), 24));
        let bootstrap_mgr = Arc::new(BootstrapNonceManager::default());
        let pairing_mgr = Arc::new(PairingManager::new(store));
        Arc::new(AuthState {
            shared_token_mgr: shared,
            session_mgr,
            auth_mode: AuthMode::Token,
            bootstrap_mgr,
            pairing_mgr,
            auth_ctx: None,
        })
    }
}
