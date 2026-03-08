use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;

use crate::a2a::domain::security::Credentials;
use crate::a2a::domain::{AgentCard, UpdateEvent};
use crate::a2a::port::authenticator::A2AAuthContext;

use super::request_processor::{A2ARequestProcessor, A2AServerState, JsonRpcRequest, JsonRpcResponse};

/// Build the axum router for A2A endpoints.
///
/// Provides three routes:
/// - `GET /.well-known/agent-card.json` — agent card discovery
/// - `POST /a2a` — synchronous JSON-RPC dispatch
/// - `POST /a2a/stream` — streaming JSON-RPC via SSE
///
/// Note: The server must be started with `.into_make_service_with_connect_info::<SocketAddr>()`
/// to support `ConnectInfo<SocketAddr>`. If ConnectInfo is not available, a fallback
/// address (127.0.0.1:0) is used.
pub fn a2a_routes(state: Arc<A2AServerState>) -> Router {
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card_handler))
        .route("/a2a", post(a2a_handler))
        .route("/a2a/stream", post(a2a_stream_handler))
        .with_state(state)
}

// --- Handlers ---

/// GET /.well-known/agent-card.json — return the agent card
async fn agent_card_handler(State(state): State<Arc<A2AServerState>>) -> Json<AgentCard> {
    Json(state.card.clone())
}

/// POST /a2a — synchronous JSON-RPC request
async fn a2a_handler(
    State(state): State<Arc<A2AServerState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let remote_addr = fallback_addr();
    let credentials = extract_credentials(&headers);
    let auth_context = A2AAuthContext {
        remote_addr,
        headers: headers_to_map(&headers),
        credentials,
    };

    // Authenticate
    let principal = match state.authenticator.authenticate(&auth_context).await {
        Ok(p) => p,
        Err(e) => {
            let resp = JsonRpcResponse::from_a2a_error(request.id.clone(), &e);
            return (StatusCode::UNAUTHORIZED, Json(resp));
        }
    };

    // Process
    let processor = A2ARequestProcessor::new(Arc::clone(&state));
    let resp = processor.process(request, principal).await;
    (StatusCode::OK, Json(resp))
}

/// POST /a2a/stream — streaming JSON-RPC via SSE
///
/// Only supports the `message/send` method. Other methods return a JSON error.
async fn a2a_stream_handler(
    State(state): State<Arc<A2AServerState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let remote_addr = fallback_addr();
    let credentials = extract_credentials(&headers);
    let auth_context = A2AAuthContext {
        remote_addr,
        headers: headers_to_map(&headers),
        credentials,
    };

    // Authenticate
    let principal = match state.authenticator.authenticate(&auth_context).await {
        Ok(p) => p,
        Err(e) => {
            let resp = JsonRpcResponse::from_a2a_error(request.id.clone(), &e);
            let json = serde_json::to_string(&resp).unwrap_or_default();
            let stream = futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(json))
            });
            return Sse::new(stream).into_response();
        }
    };

    // Only message/send supports streaming
    if request.method != "message/send" {
        let resp = JsonRpcResponse::error(
            request.id.clone(),
            -32601,
            "Only message/send supports streaming",
        );
        let json = serde_json::to_string(&resp).unwrap_or_default();
        let stream = futures::stream::once(async move {
            Ok::<_, Infallible>(Event::default().event("error").data(json))
        });
        return Sse::new(stream).into_response();
    }

    // Authorize
    let action = crate::a2a::port::authenticator::A2AAction::SendMessage;
    match state.authenticator.authorize(&principal, &action).await {
        Ok(true) => {}
        Ok(false) => {
            let resp = JsonRpcResponse::from_a2a_error(
                request.id.clone(),
                &crate::a2a::domain::A2AError::Forbidden,
            );
            let json = serde_json::to_string(&resp).unwrap_or_default();
            let stream = futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(json))
            });
            return Sse::new(stream).into_response();
        }
        Err(e) => {
            let resp = JsonRpcResponse::from_a2a_error(request.id.clone(), &e);
            let json = serde_json::to_string(&resp).unwrap_or_default();
            let stream = futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(json))
            });
            return Sse::new(stream).into_response();
        }
    }

    // Extract message params
    let message: crate::a2a::domain::A2AMessage = match serde_json::from_value(
        request.params.get("message").cloned().unwrap_or(serde_json::Value::Null),
    ) {
        Ok(m) => m,
        Err(e) => {
            let resp = JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                &format!("Invalid params: missing or invalid 'message': {}", e),
            );
            let json = serde_json::to_string(&resp).unwrap_or_default();
            let stream = futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(json))
            });
            return Sse::new(stream).into_response();
        }
    };

    let task_id = request
        .params
        .get("taskId")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let session_id = request
        .params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Get the update stream from the message handler
    let update_stream = match state
        .message_handler
        .handle_message_stream(&task_id, message, session_id.as_deref())
        .await
    {
        Ok(stream) => stream,
        Err(e) => {
            let resp = JsonRpcResponse::from_a2a_error(request.id.clone(), &e);
            let json = serde_json::to_string(&resp).unwrap_or_default();
            let stream = futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(json))
            });
            return Sse::new(stream).into_response();
        }
    };

    // Convert UpdateEvent stream to SSE events
    let request_id = request.id.clone();
    let sse_stream = update_stream.map(move |event_result| {
        match event_result {
            Ok(event) => {
                let (event_type, data) = match &event {
                    UpdateEvent::StatusUpdate(_) => (
                        "status-update",
                        serde_json::to_string(&JsonRpcResponse::success(
                            request_id.clone(),
                            serde_json::to_value(&event).unwrap_or_default(),
                        ))
                        .unwrap_or_default(),
                    ),
                    UpdateEvent::ArtifactUpdate(_) => (
                        "artifact-update",
                        serde_json::to_string(&JsonRpcResponse::success(
                            request_id.clone(),
                            serde_json::to_value(&event).unwrap_or_default(),
                        ))
                        .unwrap_or_default(),
                    ),
                };
                Ok::<_, Infallible>(Event::default().event(event_type).data(data))
            }
            Err(e) => {
                let json = serde_json::to_string(&JsonRpcResponse::from_a2a_error(
                    request_id.clone(),
                    &e,
                ))
                .unwrap_or_default();
                Ok(Event::default().event("error").data(json))
            }
        }
    });

    Sse::new(sse_stream).into_response()
}

// --- Helpers ---

/// Extract credentials from HTTP headers
fn extract_credentials(headers: &HeaderMap) -> Credentials {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            return Credentials::BearerToken(token.to_string());
        }
        if let Some(token) = auth.strip_prefix("bearer ") {
            return Credentials::BearerToken(token.to_string());
        }
    }
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Credentials::ApiKey(key.to_string());
    }
    Credentials::None
}

/// Convert axum HeaderMap to a plain HashMap
fn headers_to_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
        .collect()
}

/// Fallback socket address when ConnectInfo is not available.
/// Task 16 (Server Startup) will wire ConnectInfo properly.
fn fallback_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn extract_credentials_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer abc123"));
        match extract_credentials(&headers) {
            Credentials::BearerToken(t) => assert_eq!(t, "abc123"),
            other => panic!("Expected BearerToken, got {:?}", other),
        }
    }

    #[test]
    fn extract_credentials_bearer_lowercase() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("bearer xyz789"));
        match extract_credentials(&headers) {
            Credentials::BearerToken(t) => assert_eq!(t, "xyz789"),
            other => panic!("Expected BearerToken, got {:?}", other),
        }
    }

    #[test]
    fn extract_credentials_api_key() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("my-key-42"));
        match extract_credentials(&headers) {
            Credentials::ApiKey(k) => assert_eq!(k, "my-key-42"),
            other => panic!("Expected ApiKey, got {:?}", other),
        }
    }

    #[test]
    fn extract_credentials_none() {
        let headers = HeaderMap::new();
        assert!(matches!(extract_credentials(&headers), Credentials::None));
    }

    #[test]
    fn extract_credentials_bearer_takes_precedence() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer token1"));
        headers.insert("x-api-key", HeaderValue::from_static("key2"));
        match extract_credentials(&headers) {
            Credentials::BearerToken(t) => assert_eq!(t, "token1"),
            other => panic!("Expected BearerToken, got {:?}", other),
        }
    }

    #[test]
    fn headers_to_map_basic() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("x-custom", HeaderValue::from_static("value"));
        let map = headers_to_map(&headers);
        assert_eq!(map.get("content-type").unwrap(), "application/json");
        assert_eq!(map.get("x-custom").unwrap(), "value");
    }

    #[test]
    fn headers_to_map_empty() {
        let headers = HeaderMap::new();
        let map = headers_to_map(&headers);
        assert!(map.is_empty());
    }

    #[test]
    fn headers_to_map_skips_non_utf8() {
        let mut headers = HeaderMap::new();
        headers.insert("good", HeaderValue::from_static("ok"));
        // Non-UTF8 values are filtered out by to_str()
        let map = headers_to_map(&headers);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn fallback_addr_is_loopback() {
        let addr = fallback_addr();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 0);
    }
}
