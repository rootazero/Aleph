//! HTTP Authentication Middleware and Login Routes
//!
//! Provides session-cookie-based auth for Panel UI and
//! Bearer token auth for /v1/* API routes.

use axum::{
    Router,
    routing::{get, post},
    response::{Html, IntoResponse, Redirect, Response},
    http::{Request, StatusCode, header},
    middleware::Next,
    extract::{State, Form},
    body::Body,
};
use serde::Deserialize;
use crate::sync_primitives::Arc;
use crate::gateway::security::SharedTokenManager;
use crate::gateway::session::HttpSessionManager;
use crate::gateway::config::AuthMode;

/// Shared state for auth middleware
pub struct AuthState {
    pub shared_token_mgr: Arc<SharedTokenManager>,
    pub session_mgr: Arc<HttpSessionManager>,
    pub auth_mode: AuthMode,
}

#[derive(Deserialize)]
struct LoginForm {
    token: String,
}

/// Build auth routes (no auth required for these)
pub fn auth_routes(state: Arc<AuthState>) -> Router {
    Router::new()
        .route("/login", get(show_login))
        .route("/auth/login", post(handle_login))
        .route("/auth/logout", post(handle_logout))
        .with_state(state)
}

async fn show_login() -> Html<String> {
    Html(login_page_html(""))
}

async fn handle_login(
    State(state): State<Arc<AuthState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    match state.shared_token_mgr.validate(&form.token) {
        Ok(true) => {
            // Create session using the HMAC hash of the token
            let hash = crate::gateway::security::hmac_sign(
                state.shared_token_mgr.secret(),
                &form.token,
            );
            match state.session_mgr.create_session(&hash) {
                Ok(session_id) => {
                    let max_age = state.session_mgr.expiry_hours() * 3600;
                    let cookie = format!(
                        "aleph_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
                        session_id, max_age,
                    );
                    (
                        StatusCode::SEE_OTHER,
                        [
                            (header::LOCATION, "/".to_string()),
                            (header::SET_COOKIE, cookie),
                        ],
                    ).into_response()
                }
                Err(_) => Html(login_page_html("Internal error")).into_response(),
            }
        }
        Ok(false) => Html(login_page_html("Invalid token")).into_response(),
        Err(_) => Html(login_page_html("Internal error")).into_response(),
    }
}

async fn handle_logout(State(state): State<Arc<AuthState>>) -> Response {
    // Clear the cookie; session will expire naturally
    let _ = &state;
    let cookie = "aleph_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/login".to_string()),
            (header::SET_COOKIE, cookie.to_string()),
        ],
    ).into_response()
}

/// Extract session ID from Cookie header
fn extract_session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';')
                .filter_map(|c| {
                    let (name, value) = c.trim().split_once('=')?;
                    if name == "aleph_session" { Some(value.to_string()) } else { None }
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
        _ => Redirect::to("/login").into_response(),
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
            if let Some(token) = crate::gateway::openai_api::auth::extract_bearer_token(header_val) {
                if state.shared_token_mgr.validate(token).unwrap_or(false) {
                    return next.run(request).await;
                }
            }
            (StatusCode::UNAUTHORIZED, "Invalid token").into_response()
        }
        None => (StatusCode::UNAUTHORIZED, "Authorization header required").into_response(),
    }
}

/// Generate the login page HTML
pub fn login_page_html(error: &str) -> String {
    let error_block = if error.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div style="background:#3b1419;border:1px solid #7f1d1d;color:#fca5a5;padding:12px;border-radius:8px;margin-bottom:16px;font-size:14px">{}</div>"#,
            error
        )
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Aleph — Login</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#0a0a0f;color:#e0e0e0;display:flex;justify-content:center;align-items:center;min-height:100vh}}
.c{{background:#14141f;border:1px solid #2a2a3a;border-radius:16px;padding:40px;max-width:400px;width:100%}}
h1{{font-size:24px;margin-bottom:8px}}
p{{color:#888;font-size:14px;margin-bottom:24px}}
input{{width:100%;padding:12px 16px;background:#0a0a0f;border:1px solid #2a2a3a;border-radius:8px;color:#e0e0e0;font-size:16px;margin-bottom:16px}}
input:focus{{outline:none;border-color:#6366f1}}
button{{width:100%;padding:12px;background:#6366f1;color:#fff;border:none;border-radius:8px;font-size:16px;cursor:pointer}}
button:hover{{background:#5558e6}}
</style>
</head>
<body>
<div class="c">
<h1>Aleph</h1>
<p>Enter your access token to continue</p>
{}
<form method="POST" action="/auth/login" id="lf">
<input type="password" name="token" placeholder="Access token" autofocus required>
<button type="submit">Sign in</button>
</form>
<script>document.getElementById('lf').addEventListener('submit',function(){{var t=this.querySelector('input[name=token]').value;try{{localStorage.setItem('aleph_shared_token',t)}}catch(e){{}}}});</script>
</div>
</body>
</html>"#,
        error_block
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_html_not_empty() {
        let html = login_page_html("");
        assert!(html.contains("<form"));
        assert!(html.contains("token"));
        assert!(html.contains("Aleph"));
    }

    #[test]
    fn test_login_html_shows_error() {
        let html = login_page_html("Invalid token");
        assert!(html.contains("Invalid token"));
    }

    #[test]
    fn test_login_html_no_error_when_empty() {
        let html = login_page_html("");
        assert!(!html.contains("3b1419")); // error bg color shouldn't appear
    }

    #[test]
    fn test_extract_session_cookie_found() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::COOKIE, "aleph_session=abc123; other=val".parse().unwrap());
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
    fn test_local_storage_script_present() {
        let html = login_page_html("");
        assert!(html.contains("localStorage.setItem"));
        assert!(html.contains("aleph_shared_token"));
    }
}
