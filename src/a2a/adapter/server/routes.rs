use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;

use crate::a2a::domain::security::Credentials;
use crate::a2a::domain::{AgentCard, UpdateEvent};
use crate::a2a::port::authenticator::{A2AAuthContext, A2AAuthPrincipal};

use super::request_processor::{
    A2ARequestProcessor, A2AServerState, JsonRpcRequest, JsonRpcResponse,
};

/// Build the axum router for A2A endpoints.
///
/// Provides three routes:
/// - `GET /.well-known/agent-card.json` — agent card discovery
/// - `POST /a2a` — synchronous JSON-RPC dispatch
/// - `POST /a2a/stream` — streaming JSON-RPC via SSE
///
/// Note: The server must be started with `.into_make_service_with_connect_info::<SocketAddr>()`
/// to support `ConnectInfo<SocketAddr>`. If `ConnectInfo` is not available, a fallback
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
    PeerAddr(remote_addr): PeerAddr,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
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

/// POST /a2a/stream — streaming JSON-RPC via Server-Sent Events.
///
/// Dispatches the A2A methods that produce an event stream:
/// - `message/stream` — run a new turn and stream its task updates (canonical
///   A2A method name; `message/send` is accepted as a back-compat alias)
/// - `tasks/resubscribe` — re-attach to the live event stream of a task that
///   is already running (e.g. after a dropped SSE connection)
///
/// Any other method is rejected with a JSON-RPC `MethodNotFound` error, since the
/// synchronous `/a2a` endpoint is the correct target for it.
async fn a2a_stream_handler(
    State(state): State<Arc<A2AServerState>>,
    headers: HeaderMap,
    PeerAddr(remote_addr): PeerAddr,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let credentials = extract_credentials(&headers);
    let auth_context = A2AAuthContext {
        remote_addr,
        headers: headers_to_map(&headers),
        credentials,
    };

    // Authenticate once; per-method authorization happens in each branch.
    let principal = match state.authenticator.authenticate(&auth_context).await {
        Ok(p) => p,
        Err(e) => {
            return sse_error(JsonRpcResponse::from_a2a_error(request.id.clone(), &e));
        }
    };

    match request.method.as_str() {
        // `message/stream` is the canonical A2A streaming method name
        // (SendStreamingMessage); `message/send` is a back-compat alias.
        "message/stream" | "message/send" => stream_message_send(state, principal, request).await,
        "tasks/resubscribe" => stream_resubscribe(state, principal, request).await,
        other => sse_error(JsonRpcResponse::error(
            request.id.clone(),
            -32601,
            &format!("Method does not support streaming: {other}"),
        )),
    }
}

/// `message/send` over SSE — execute a new turn and stream its task updates.
async fn stream_message_send(
    state: Arc<A2AServerState>,
    principal: A2AAuthPrincipal,
    request: JsonRpcRequest,
) -> axum::response::Response {
    let action = crate::a2a::port::authenticator::A2AAction::SendMessage;
    if let Err(resp) = authorize_stream(&state, &principal, &action, &request).await {
        return resp;
    }

    // Extract message params
    let message: crate::a2a::domain::A2AMessage = match serde_json::from_value(
        request
            .params
            .get("message")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        Ok(m) => m,
        Err(e) => {
            return sse_error(JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                &format!("Invalid params: missing or invalid 'message': {e}"),
            ));
        }
    };

    // Enforce the same File-part invariant the sync path checks: a peer must
    // not send a File part carrying both `bytes`/`uri` or neither. Without this
    // the streaming endpoint accepted malformed parts the sync path rejects.
    for part in &message.parts {
        if let crate::a2a::domain::Part::File { file, .. } = part {
            if let Err(e) = file.validate() {
                return sse_error(JsonRpcResponse::error(
                    request.id.clone(),
                    -32602,
                    &format!("Invalid params: {e}"),
                ));
            }
        }
    }

    let task_id = request
        .params
        .get("taskId")
        .and_then(|v| v.as_str())
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), String::from);

    if let Some(push_params) = request.params.get("pushNotificationConfig").cloned() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct InlinePushConfig {
            url: String,
            #[serde(default)]
            token: Option<String>,
            #[serde(default)]
            events: Vec<String>,
        }
        match serde_json::from_value::<InlinePushConfig>(push_params) {
            Ok(inline) => {
                let push_config =
                    crate::a2a::service::notification::PushNotificationConfig {
                        task_id: task_id.clone(),
                        url: inline.url,
                        token: inline.token,
                        events: inline.events,
                    };
                if let Err(e) = state.notification.set_config(push_config).await {
                    return sse_error(JsonRpcResponse::from_a2a_error(
                        request.id.clone(),
                        &e,
                    ));
                }
            }
            Err(e) => {
                return sse_error(JsonRpcResponse::error(
                    request.id.clone(),
                    -32602,
                    &format!("Invalid params: invalid 'pushNotificationConfig': {e}"),
                ));
            }
        }
    }

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
            return sse_error(JsonRpcResponse::from_a2a_error(request.id.clone(), &e));
        }
    };

    sse_from_update_stream(request.id, update_stream)
}

/// `tasks/resubscribe` over SSE — re-attach to a running task's event stream.
///
/// Connects the caller to the shared broadcast hub for the task so it can
/// resume receiving status/artifact updates after a dropped connection. The
/// hub channel exists only while the task is in flight; resubscribing to an
/// already-finished (and cleaned-up) task yields an idle stream that the client
/// closes itself. Mirrors the A2A `tasks/resubscribe` method (params `{ id }`).
async fn stream_resubscribe(
    state: Arc<A2AServerState>,
    principal: A2AAuthPrincipal,
    request: JsonRpcRequest,
) -> axum::response::Response {
    let action = crate::a2a::port::authenticator::A2AAction::Subscribe;
    if let Err(resp) = authorize_stream(&state, &principal, &action, &request).await {
        return resp;
    }

    let task_id = match request.params.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return sse_error(JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                "Invalid params: missing 'id'",
            ));
        }
    };

    let update_stream = match state.streaming.subscribe_all(&task_id).await {
        Ok(stream) => stream,
        Err(e) => {
            return sse_error(JsonRpcResponse::from_a2a_error(request.id.clone(), &e));
        }
    };

    sse_from_update_stream(request.id, update_stream)
}

/// Authorize a streaming request, returning an SSE error response on denial.
async fn authorize_stream(
    state: &Arc<A2AServerState>,
    principal: &A2AAuthPrincipal,
    action: &crate::a2a::port::authenticator::A2AAction,
    request: &JsonRpcRequest,
) -> Result<(), axum::response::Response> {
    match state.authenticator.authorize(principal, action).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(sse_error(JsonRpcResponse::from_a2a_error(
            request.id.clone(),
            &crate::a2a::domain::A2AError::Forbidden,
        ))),
        Err(e) => Err(sse_error(JsonRpcResponse::from_a2a_error(
            request.id.clone(),
            &e,
        ))),
    }
}

/// Wrap an `UpdateEvent` stream as an SSE response with the standard framing.
fn sse_from_update_stream(
    request_id: Option<serde_json::Value>,
    update_stream: std::pin::Pin<
        Box<
            dyn futures::Stream<Item = crate::a2a::port::task_manager::A2AResult<UpdateEvent>>
                + Send,
        >,
    >,
) -> axum::response::Response {
    let sse_stream = update_stream.map(move |event_result| {
        Ok::<_, Infallible>(update_event_to_sse(&request_id, event_result))
    });
    Sse::new(sse_stream)
        .keep_alive(KeepAlive::new())
        .into_response()
}

/// Build the `(sse-event-name, json-rpc-payload)` frame for one update event.
///
/// Extracted so every streaming method emits identical wire framing for
/// `status-update` / `artifact-update` / `error` events.
fn update_event_frame(
    request_id: &Option<serde_json::Value>,
    event_result: crate::a2a::port::task_manager::A2AResult<UpdateEvent>,
) -> (&'static str, String) {
    match event_result {
        Ok(event) => {
            let event_type = match &event {
                UpdateEvent::StatusUpdate(_) => "status-update",
                UpdateEvent::ArtifactUpdate(_) => "artifact-update",
            };
            let data = serde_json::to_string(&JsonRpcResponse::success(
                request_id.clone(),
                serde_json::to_value(&event).unwrap_or_default(),
            ))
            .unwrap_or_default();
            (event_type, data)
        }
        Err(e) => {
            let data =
                serde_json::to_string(&JsonRpcResponse::from_a2a_error(request_id.clone(), &e))
                    .unwrap_or_default();
            ("error", data)
        }
    }
}

/// Map a single update event (or stream error) onto a JSON-RPC-wrapped SSE event.
fn update_event_to_sse(
    request_id: &Option<serde_json::Value>,
    event_result: crate::a2a::port::task_manager::A2AResult<UpdateEvent>,
) -> Event {
    let (event_type, data) = update_event_frame(request_id, event_result);
    Event::default().event(event_type).data(data)
}

/// Build an SSE error response from a `JsonRpcResponse`
fn sse_error(resp: JsonRpcResponse) -> axum::response::Response {
    let json = serde_json::to_string(&resp).unwrap_or_default();
    let stream = futures::stream::once(async move {
        Ok::<_, Infallible>(Event::default().event("error").data(json))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new())
        .into_response()
}

// --- Helpers ---

/// Extract credentials from HTTP headers
fn extract_credentials(headers: &HeaderMap) -> Credentials {
if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
            // RFC 7235: auth-scheme is case-insensitive
            if let Some(prefix) = auth.get(..7) {
                if prefix.eq_ignore_ascii_case("bearer ") {
                    if let Some(token) = auth.get(7..) {
                        return Credentials::BearerToken(token.to_string());
                    }
                }
            }
        }
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Credentials::ApiKey(key.to_string());
    }
    Credentials::None
}

/// Convert axum `HeaderMap` to a plain `HashMap`
fn headers_to_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
        .collect()
}

/// Fallback socket address when `ConnectInfo` is not available.
///
/// Returns a non-loopback address so that missing `ConnectInfo` does NOT
/// trigger the localhost authentication bypass. `ConnectInfo` must be wired
/// via `.into_make_service_with_connect_info::<SocketAddr>()` in production;
/// the fallback is only safe for local tests and oneshot requests.
fn fallback_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 0))
}

/// Peer socket address extractor that never rejects.
///
/// Reads the `ConnectInfo<SocketAddr>` injected by
/// `.into_make_service_with_connect_info::<SocketAddr>()` (always present when
/// the server is wired that way) and falls back to a non-loopback address when
/// it is absent — e.g. a `oneshot` request in tests. This keeps the handlers
/// infallible without granting localhost trust to remote callers.
struct PeerAddr(SocketAddr);

impl<S> FromRequestParts<S> for PeerAddr
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let addr = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or_else(fallback_addr, |ci| ci.0);
        Ok(Self(addr))
    }
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
    fn extract_credentials_bearer_mixed_case() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("BEARER upper123"));
        match extract_credentials(&headers) {
            Credentials::BearerToken(t) => assert_eq!(t, "upper123"),
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
    fn fallback_addr_is_not_loopback() {
        let addr = fallback_addr();
        assert!(!addr.ip().is_loopback());
        assert_eq!(addr.port(), 0);
    }

    // --- SSE framing (shared by message/send and tasks/resubscribe) ---

    use crate::a2a::domain::message::Artifact;
    use crate::a2a::domain::task::{TaskState, TaskStatus};
    use crate::a2a::domain::{A2AError, Part, TaskArtifactUpdateEvent, TaskStatusUpdateEvent};

    fn status_update(task_id: &str, state: TaskState) -> UpdateEvent {
        UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx-1".to_string(),
            status: TaskStatus {
                state,
                message: None,
                timestamp: chrono::Utc::now(),
            },
            is_final: false,
            metadata: None,
        })
    }

    fn artifact_update(task_id: &str) -> UpdateEvent {
        UpdateEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx-1".to_string(),
            artifact: Artifact {
                artifact_id: "art-1".to_string(),
                kind: "text".to_string(),
                parts: vec![Part::Text {
                    text: "hi".to_string(),
                    metadata: None,
                }],
                metadata: None,
            },
            append: false,
            last_chunk: true,
            metadata: None,
        })
    }

    #[test]
    fn frame_status_update_is_status_event_with_result() {
        let id = Some(serde_json::Value::Number(1.into()));
        let (event_type, data) =
            update_event_frame(&id, Ok(status_update("t1", TaskState::Working)));
        assert_eq!(event_type, "status-update");
        let json: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        // success frames carry `result`, never `error`
        assert!(json.get("result").is_some());
        assert!(json.get("error").is_none());
        assert_eq!(json["result"]["taskId"], "t1");
    }

    #[test]
    fn frame_artifact_update_is_artifact_event() {
        let id = Some(serde_json::Value::String("req-7".to_string()));
        let (event_type, data) = update_event_frame(&id, Ok(artifact_update("t2")));
        assert_eq!(event_type, "artifact-update");
        let json: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert!(json.get("result").is_some());
        assert_eq!(json["result"]["artifact"]["artifactId"], "art-1");
    }

    #[test]
    fn frame_error_is_error_event_with_code() {
        let id = Some(serde_json::Value::Number(9.into()));
        let (event_type, data) =
            update_event_frame(&id, Err(A2AError::TaskNotFound("missing".to_string())));
        assert_eq!(event_type, "error");
        let json: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert!(json.get("result").is_none());
        assert_eq!(json["error"]["code"], -32001);
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing"));
    }

    use std::pin::Pin;

    use futures::Stream;
    use tower::ServiceExt;

    use crate::a2a::adapter::server::request_processor::A2AServerState;
    use crate::a2a::domain::security::TrustLevel;
    use crate::a2a::domain::{
        A2AMessage, ListTasksParams, ListTasksResult, SecurityScheme,
    };
    use crate::a2a::port::authenticator::{
        A2AAction, A2AAuthContext, A2AAuthPrincipal, A2AAuthenticator,
    };
    use crate::a2a::port::message_handler::A2AMessageHandler;
    use crate::a2a::port::streaming::A2AStreamingHandler;
    use crate::a2a::port::task_manager::{A2AResult, A2ATaskManager};
    use crate::a2a::service::notification::NotificationService;

    struct AllowAllAuth;

    #[async_trait::async_trait]
    impl A2AAuthenticator for AllowAllAuth {
        async fn authenticate(
            &self,
            _context: &A2AAuthContext,
        ) -> A2AResult<A2AAuthPrincipal> {
            Ok(A2AAuthPrincipal {
                agent_id: None,
                trust_level: TrustLevel::Local,
                permissions: vec!["*".to_string()],
            })
        }

        async fn authorize(
            &self,
            _principal: &A2AAuthPrincipal,
            _action: &A2AAction,
        ) -> A2AResult<bool> {
            Ok(true)
        }

        fn supported_schemes(&self) -> Vec<SecurityScheme> {
            vec![]
        }
    }

    struct StubTaskManager;

    #[async_trait::async_trait]
    impl A2ATaskManager for StubTaskManager {
        async fn create_task(
            &self,
            task_id: &str,
            context_id: &str,
        ) -> A2AResult<crate::a2a::domain::A2ATask> {
            Ok(crate::a2a::domain::A2ATask::new(task_id, context_id))
        }

        async fn get_task(
            &self,
            task_id: &str,
            _history_length: Option<usize>,
        ) -> A2AResult<crate::a2a::domain::A2ATask> {
            Ok(crate::a2a::domain::A2ATask::new(task_id, "ctx-default"))
        }

        async fn update_status(
            &self,
            task_id: &str,
            _state: TaskState,
            _message: Option<A2AMessage>,
        ) -> A2AResult<crate::a2a::domain::A2ATask> {
            Ok(crate::a2a::domain::A2ATask::new(task_id, "ctx-default"))
        }

        async fn cancel_task(
            &self,
            task_id: &str,
        ) -> A2AResult<crate::a2a::domain::A2ATask> {
            Ok(crate::a2a::domain::A2ATask::new(task_id, "ctx-default"))
        }

        async fn list_tasks(
            &self,
            _params: ListTasksParams,
        ) -> A2AResult<ListTasksResult> {
            Ok(ListTasksResult {
                tasks: vec![],
                next_cursor: None,
            })
        }

        async fn add_artifact(&self, _task_id: &str, _artifact: Artifact) -> A2AResult<()> {
            Ok(())
        }
    }

    struct StubMessageHandler;

    #[async_trait::async_trait]
    impl A2AMessageHandler for StubMessageHandler {
        async fn handle_message(
            &self,
            task_id: &str,
            _message: A2AMessage,
            _session_id: Option<&str>,
        ) -> A2AResult<crate::a2a::domain::A2ATask> {
            Ok(crate::a2a::domain::A2ATask::new(task_id, "ctx-msg"))
        }

        async fn handle_message_stream(
            &self,
            _task_id: &str,
            _message: A2AMessage,
            _session_id: Option<&str>,
        ) -> A2AResult<
            Pin<Box<dyn Stream<Item = A2AResult<crate::a2a::domain::UpdateEvent>> + Send>>,
        > {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    struct StubStreamingHandler;

    #[async_trait::async_trait]
    impl A2AStreamingHandler for StubStreamingHandler {
        async fn subscribe_all(
            &self,
            _task_id: &str,
        ) -> A2AResult<
            Pin<Box<dyn Stream<Item = A2AResult<crate::a2a::domain::UpdateEvent>> + Send>>,
        > {
            Ok(Box::pin(futures::stream::empty()))
        }

        async fn broadcast_status(
            &self,
            _task_id: &str,
            _update: TaskStatusUpdateEvent,
        ) -> A2AResult<()> {
            Ok(())
        }
    }

    fn build_test_state() -> std::sync::Arc<A2AServerState> {
        std::sync::Arc::new(A2AServerState {
            task_manager: std::sync::Arc::new(StubTaskManager),
            message_handler: std::sync::Arc::new(StubMessageHandler),
            streaming: std::sync::Arc::new(StubStreamingHandler),
            authenticator: std::sync::Arc::new(AllowAllAuth),
            notification: std::sync::Arc::new(NotificationService::new()),
            card: crate::a2a::domain::AgentCard {
                id: "test".to_string(),
                name: "Test".to_string(),
                version: "0.1.0".to_string(),
                description: None,
                provider: None,
                documentation_url: None,
                interfaces: vec![],
                skills: vec![],
                security: vec![],
                extensions: vec![],
                default_input_modes: vec![],
                default_output_modes: vec![],
            },
        })
    }

    fn stream_request_body(
        method: &str,
        task_id: Option<&str>,
        push_url: Option<&str>,
    ) -> serde_json::Value {
        let mut params = serde_json::json!({
            "message": {
                "messageId": "m-stream-push",
                "role": "user",
                "parts": [{"type": "text", "text": "hi"}]
            }
        });
        if let Some(tid) = task_id {
            params
                .as_object_mut()
                .unwrap()
                .insert("taskId".to_string(), serde_json::json!(tid));
        }
        if let Some(url) = push_url {
            params.as_object_mut().unwrap().insert(
                "pushNotificationConfig".to_string(),
                serde_json::json!({"url": url, "events": ["status-update"]}),
            );
        }
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        })
    }

    #[tokio::test]
    async fn stream_message_send_registers_push_notification_config() {
        use axum::body::Body;
        use axum::http::Request;

        let state = build_test_state();
        let app = a2a_routes(std::sync::Arc::clone(&state));

        let rpc_body = stream_request_body(
            "message/stream",
            Some("task-stream-push-1"),
            Some("https://8.8.8.8/notify"),
        );

        let request = Request::builder()
            .method("POST")
            .uri("/a2a/stream")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&rpc_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let stored = state
            .notification
            .get_config("task-stream-push-1")
            .await
            .expect("get_config should not error")
            .expect("push config should be registered for streamed task");
        assert_eq!(stored.url, "https://8.8.8.8/notify");
        assert_eq!(stored.events, vec!["status-update".to_string()]);
    }

    #[tokio::test]
    async fn stream_message_send_without_push_config_does_not_register() {
        use axum::body::Body;
        use axum::http::Request;

        let state = build_test_state();
        let app = a2a_routes(std::sync::Arc::clone(&state));

        let rpc_body = stream_request_body("message/send", Some("task-stream-nopush"), None);

        let request = Request::builder()
            .method("POST")
            .uri("/a2a/stream")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&rpc_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let stored = state
            .notification
            .get_config("task-stream-nopush")
            .await
            .expect("get_config should not error");
        assert!(stored.is_none(), "no push config must be stored");
    }
}
