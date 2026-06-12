//! Bearer-token gate. The token is minted per-spawn and handed to the
//! supervisor via the READY line — loopback-only defense in depth.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::AppState;

pub async fn require_bearer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == state.token);
    if !ok {
        return (StatusCode::UNAUTHORIZED, "invalid or missing bearer token").into_response();
    }
    state.last_activity_ms.store(crate::lifecycle::now_ms(), std::sync::atomic::Ordering::Relaxed);
    next.run(req).await
}
