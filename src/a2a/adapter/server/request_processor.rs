use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::a2a::domain::{A2AError, A2AMessage, AgentCard, ListTasksParams, Part};
use crate::a2a::port::authenticator::{A2AAction, A2AAuthPrincipal, A2AAuthenticator};
use crate::a2a::port::message_handler::A2AMessageHandler;
use crate::a2a::port::streaming::A2AStreamingHandler;
use crate::a2a::port::task_manager::A2ATaskManager;
use crate::a2a::service::notification::NotificationService;

/// Shared state for the A2A HTTP server.
///
/// Holds references to all port implementations and the agent card.
/// Passed as axum state to all route handlers.
pub struct A2AServerState {
    pub task_manager: Arc<dyn A2ATaskManager>,
    pub message_handler: Arc<dyn A2AMessageHandler>,
    pub streaming: Arc<dyn A2AStreamingHandler>,
    pub authenticator: Arc<dyn A2AAuthenticator>,
    pub notification: Arc<NotificationService>,
    pub card: AgentCard,
}

// --- JSON-RPC 2.0 structures ---

/// JSON-RPC 2.0 request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    pub id: Option<Value>,
}

/// JSON-RPC 2.0 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    pub id: Option<Value>,
}

impl JsonRpcResponse {
    /// Build a success response
    #[must_use]
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Build an error response from code and message
    #[must_use]
    pub fn error(id: Option<Value>, code: i64, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(serde_json::json!({
                "code": code,
                "message": message,
            })),
            id,
        }
    }

    /// Build an error response from an `A2AError`
    #[must_use]
    pub fn from_a2a_error(id: Option<Value>, err: &A2AError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(err.to_jsonrpc_error()),
            id,
        }
    }

    /// Serialize a value into a JSON-RPC success response, or return an
    /// internal-error response if serialization fails. Centralizes the
    /// `to_value` / `success` / `error(-32603, ...)` dance used by every
    /// handler that returns domain types.
    #[must_use]
    pub fn serialize_ok<T: Serialize>(id: Option<Value>, value: &T) -> Self {
        match serde_json::to_value(value) {
            Ok(v) => Self::success(id, v),
            Err(e) => Self::error(id, -32603, &format!("Internal error: {e}")),
        }
    }
}

// --- Request Processor ---

/// Dispatches JSON-RPC requests to the appropriate A2A handler.
///
/// Each method is authorized before execution. Unknown methods
/// return a standard JSON-RPC `MethodNotFound` error.
pub struct A2ARequestProcessor {
    state: Arc<A2AServerState>,
}

impl A2ARequestProcessor {
    #[must_use]
    pub const fn new(state: Arc<A2AServerState>) -> Self {
        Self { state }
    }

    /// Dispatch a JSON-RPC request to the matching handler
    pub async fn process(
        &self,
        request: JsonRpcRequest,
        auth: A2AAuthPrincipal,
    ) -> JsonRpcResponse {
        if request.jsonrpc != "2.0" {
            return JsonRpcResponse::error(
                request.id,
                -32600,
                "Invalid request: jsonrpc field must be \"2.0\"",
            );
        }

        match request.method.as_str() {
            "message/send" => self.handle_message_send(request, auth).await,
            "tasks/get" => self.handle_tasks_get(request, auth).await,
            "tasks/cancel" => self.handle_tasks_cancel(request, auth).await,
            "tasks/list" => self.handle_tasks_list(request, auth).await,
            "tasks/pushNotificationConfig/set"
            | "tasks/pushNotificationConfig/get"
            | "tasks/pushNotificationConfig/list"
            | "tasks/pushNotificationConfig/delete" => self.handle_push_config(request, auth).await,
            "agent/getAuthenticatedExtendedCard" => self.handle_extended_card(request, auth).await,
            _ => JsonRpcResponse::error(
                request.id,
                -32601,
                &format!("Method not found: {}", request.method),
            ),
        }
    }

    // --- Individual handlers ---

    async fn handle_message_send(
        &self,
        request: JsonRpcRequest,
        auth: A2AAuthPrincipal,
    ) -> JsonRpcResponse {
        if let Err(resp) = self
            .authorize(&request, &auth, &A2AAction::SendMessage)
            .await
        {
            return resp;
        }

        // Extract params
        let message: A2AMessage = match serde_json::from_value(
            request
                .params
                .get("message")
                .cloned()
                .unwrap_or(Value::Null),
        ) {
            Ok(m) => m,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    -32602,
                    &format!("Invalid params: missing or invalid 'message': {e}"),
                );
            }
        };

        // Enforce the A2A spec invariant that a File part carries exactly one
        // of `bytes`/`uri` at the deserialization boundary — a peer can
        // otherwise send a part with both or neither and have it flow through
        // unchecked (the `validate()` method was previously never called).
        for part in &message.parts {
            if let Part::File { file, .. } = part {
                if let Err(e) = file.validate() {
                    return JsonRpcResponse::error(
                        request.id,
                        -32602,
                        &format!("Invalid params: {e}"),
                    );
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
                    let push_config = crate::a2a::service::notification::PushNotificationConfig {
                        task_id: task_id.clone(),
                        url: inline.url,
                        token: inline.token,
                        events: inline.events,
                    };
                    if let Err(e) = self.state.notification.set_config(push_config).await {
                        return JsonRpcResponse::from_a2a_error(request.id, &e);
                    }
                }
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        -32602,
                        &format!("Invalid params: invalid 'pushNotificationConfig': {e}"),
                    );
                }
            }
        }

        let session_id = request
            .params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(String::from);

        match self
            .state
            .message_handler
            .handle_message(&task_id, message, session_id.as_deref())
            .await
        {
            Ok(task) => JsonRpcResponse::serialize_ok(request.id, &task),
            Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
        }
    }

    async fn handle_tasks_get(
        &self,
        request: JsonRpcRequest,
        auth: A2AAuthPrincipal,
    ) -> JsonRpcResponse {
        if let Err(resp) = self.authorize(&request, &auth, &A2AAction::GetTask).await {
            return resp;
        }

        let id = match request.params.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return JsonRpcResponse::error(request.id, -32602, "Invalid params: missing 'id'");
            }
        };

        let history_length = request
            .params
            .get("historyLength")
            .and_then(|v| v.as_u64())
            .map(|v| usize::try_from(v).unwrap_or(usize::MAX));

        match self.state.task_manager.get_task(id, history_length).await {
            Ok(task) => JsonRpcResponse::serialize_ok(request.id, &task),
            Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
        }
    }

    async fn handle_tasks_cancel(
        &self,
        request: JsonRpcRequest,
        auth: A2AAuthPrincipal,
    ) -> JsonRpcResponse {
        if let Err(resp) = self
            .authorize(&request, &auth, &A2AAction::CancelTask)
            .await
        {
            return resp;
        }

        let id = match request.params.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return JsonRpcResponse::error(request.id, -32602, "Invalid params: missing 'id'");
            }
        };

        match self.state.task_manager.cancel_task(id).await {
            Ok(task) => {
                // Broadcast a Canceled status update so any SSE subscriber
                // attached via `tasks/resubscribe` (or the original
                // `message/stream`) sees the cancellation. Without this the
                // subscriber sees the stream end silently when the bridge's
                // spawned task eventually completes. A2A-R4-07.
                let cancel_event = crate::a2a::domain::events::TaskStatusUpdateEvent {
                    task_id: task.id.clone(),
                    context_id: task.context_id.clone(),
                    status: crate::a2a::domain::task::TaskStatus {
                        state: crate::a2a::domain::task::TaskState::Canceled,
                        message: None,
                        timestamp: chrono::Utc::now(),
                    },
                    is_final: true,
                    metadata: None,
                };
                let _ = self
                    .state
                    .streaming
                    .broadcast_status(&task.id, cancel_event)
                    .await;
                JsonRpcResponse::serialize_ok(request.id, &task)
            }
            Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
        }
    }

    async fn handle_tasks_list(
        &self,
        request: JsonRpcRequest,
        auth: A2AAuthPrincipal,
    ) -> JsonRpcResponse {
        if let Err(resp) = self.authorize(&request, &auth, &A2AAction::ListTasks).await {
            return resp;
        }

        let params: ListTasksParams = if request.params.is_null() {
            ListTasksParams::default()
        } else if request.params.is_object() {
            match serde_json::from_value(request.params.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        -32602,
                        &format!("Invalid params: {e}"),
                    );
                }
            }
        } else {
            ListTasksParams::default()
        };

        match self.state.task_manager.list_tasks(params).await {
            Ok(result) => JsonRpcResponse::serialize_ok(request.id, &result),
            Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
        }
    }

    /// Dispatch push notification config operations — all require `ManagePushConfig` permission
    async fn handle_push_config(
        &self,
        request: JsonRpcRequest,
        auth: A2AAuthPrincipal,
    ) -> JsonRpcResponse {
        if let Err(resp) = self
            .authorize(&request, &auth, &A2AAction::ManagePushConfig)
            .await
        {
            return resp;
        }

        match request.method.as_str() {
            "tasks/pushNotificationConfig/set" => {
                let config: crate::a2a::service::notification::PushNotificationConfig =
                    match serde_json::from_value(request.params.clone()) {
                        Ok(c) => c,
                        Err(e) => {
                            return JsonRpcResponse::error(
                                request.id,
                                -32602,
                                &format!("Invalid params: {e}"),
                            );
                        }
                    };
                match self.state.notification.set_config(config).await {
                    Ok(c) => JsonRpcResponse::serialize_ok(request.id, &c),
                    Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
                }
            }
            "tasks/pushNotificationConfig/get" => {
                let task_id = match request.params.get("taskId").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        return JsonRpcResponse::error(
                            request.id,
                            -32602,
                            "Invalid params: missing 'taskId'",
                        );
                    }
                };
                match self.state.notification.get_config(task_id).await {
                    Ok(config) => JsonRpcResponse::serialize_ok(request.id, &config),
                    Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
                }
            }
            "tasks/pushNotificationConfig/list" => {
                match self.state.notification.list_configs().await {
                    Ok(configs) => JsonRpcResponse::serialize_ok(request.id, &configs),
                    Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
                }
            }
            "tasks/pushNotificationConfig/delete" => {
                let task_id = match request.params.get("taskId").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        return JsonRpcResponse::error(
                            request.id,
                            -32602,
                            "Invalid params: missing 'taskId'",
                        );
                    }
                };
                match self.state.notification.delete_config(task_id).await {
                    Ok(()) => JsonRpcResponse::success(request.id, Value::Null),
                    Err(e) => JsonRpcResponse::from_a2a_error(request.id, &e),
                }
            }
            _ => JsonRpcResponse::error(request.id, -32601, "Unknown push config method"),
        }
    }

    /// `agent/getAuthenticatedExtendedCard` — return the agent card to an
    /// authenticated caller.
    ///
    /// The public card is served unauthenticated at
    /// `/.well-known/agent-card.json`; this method exposes the same card (which
    /// may carry richer, authenticated-only detail) behind the standard
    /// authorization layer, matching the A2A reference SDKs. Requires `read`
    /// access — anonymous/`Public` callers without it are rejected.
    async fn handle_extended_card(
        &self,
        request: JsonRpcRequest,
        auth: A2AAuthPrincipal,
    ) -> JsonRpcResponse {
        if let Err(resp) = self
            .authorize(&request, &auth, &A2AAction::GetExtendedCard)
            .await
        {
            return resp;
        }

        JsonRpcResponse::serialize_ok(request.id, &self.state.card)
    }

    // --- Authorization helper ---

    async fn authorize(
        &self,
        request: &JsonRpcRequest,
        auth: &A2AAuthPrincipal,
        action: &A2AAction,
    ) -> Result<(), JsonRpcResponse> {
        match self.state.authenticator.authorize(auth, action).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(JsonRpcResponse::from_a2a_error(
                request.id.clone(),
                &A2AError::Forbidden,
            )),
            Err(e) => Err(JsonRpcResponse::from_a2a_error(request.id.clone(), &e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::{
        A2ATask, Artifact, ListTasksResult, SecurityScheme, TaskState, TaskStatusUpdateEvent,
        UpdateEvent,
    };

    #[test]
    fn jsonrpc_response_success() {
        let resp = JsonRpcResponse::success(
            Some(Value::Number(1.into())),
            serde_json::json!({"status": "ok"}),
        );
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        assert_eq!(resp.id, Some(Value::Number(1.into())));
    }

    #[test]
    fn jsonrpc_response_error() {
        let resp =
            JsonRpcResponse::error(Some(Value::Number(2.into())), -32601, "Method not found");
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err["code"], -32601);
        assert_eq!(err["message"], "Method not found");
    }

    #[test]
    fn jsonrpc_response_from_a2a_error() {
        let a2a_err = A2AError::TaskNotFound("task-42".to_string());
        let resp = JsonRpcResponse::from_a2a_error(Some(Value::String("req-1".into())), &a2a_err);
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err["code"], -32001);
        assert!(err["message"].as_str().unwrap().contains("task-42"));
    }

    #[test]
    fn jsonrpc_response_null_id() {
        let resp = JsonRpcResponse::success(None, Value::Bool(true));
        assert!(resp.id.is_none());
    }

    #[test]
    fn jsonrpc_response_serde_roundtrip() {
        let resp = JsonRpcResponse::success(
            Some(Value::Number(1.into())),
            serde_json::json!({"data": "hello"}),
        );
        let json = serde_json::to_string(&resp).unwrap();
        let back: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.jsonrpc, "2.0");
        assert!(back.result.is_some());
        assert!(back.error.is_none());
    }

    #[test]
    fn jsonrpc_response_error_omits_result() {
        let resp = JsonRpcResponse::error(None, -32700, "Parse error");
        let json = serde_json::to_value(&resp).unwrap();
        // result should be omitted (skip_serializing_if = "Option::is_none")
        assert!(json.get("result").is_none());
        assert!(json.get("error").is_some());
    }

    #[test]
    fn jsonrpc_response_success_omits_error() {
        let resp = JsonRpcResponse::success(None, Value::Null);
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("error").is_none());
        assert!(json.get("result").is_some());
    }

    #[test]
    fn jsonrpc_request_deserialize() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "message/send",
            "params": {"message": {"messageId": "m1", "role": "user", "parts": []}},
            "id": 1
        }"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "message/send");
        assert!(req.params.is_object());
        assert_eq!(req.id, Some(Value::Number(1.into())));
    }

    #[test]
    fn jsonrpc_request_no_params() {
        let json = r#"{"jsonrpc": "2.0", "method": "tasks/list", "id": "abc"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tasks/list");
        // params defaults to null when missing
        assert!(req.params.is_null());
    }

    // Integration-style test using mock implementations
    mod dispatch {
        use super::*;
        use std::pin::Pin;

        use futures::Stream;

        use crate::a2a::domain::security::TrustLevel;
        use crate::a2a::port::authenticator::A2AAuthContext;
        use crate::a2a::port::task_manager::A2AResult;

        // Minimal mock authenticator that always allows
        struct AllowAllAuth;

        #[async_trait::async_trait]
        impl A2AAuthenticator for AllowAllAuth {
            async fn authenticate(&self, _context: &A2AAuthContext) -> A2AResult<A2AAuthPrincipal> {
                Ok(A2AAuthPrincipal {
                    agent_id: None,
                    trust_level: TrustLevel::Local,
                    permissions: vec![],
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

        // Minimal mock task manager
        struct MockTaskManager;

        #[async_trait::async_trait]
        impl A2ATaskManager for MockTaskManager {
            async fn create_task(&self, task_id: &str, context_id: &str) -> A2AResult<A2ATask> {
                Ok(A2ATask::new(task_id, context_id))
            }

            async fn get_task(
                &self,
                task_id: &str,
                _history_length: Option<usize>,
            ) -> A2AResult<A2ATask> {
                Ok(A2ATask::new(task_id, "ctx-default"))
            }

            async fn update_status(
                &self,
                task_id: &str,
                _state: TaskState,
                _message: Option<A2AMessage>,
            ) -> A2AResult<A2ATask> {
                Ok(A2ATask::new(task_id, "ctx-default"))
            }

            async fn cancel_task(&self, task_id: &str) -> A2AResult<A2ATask> {
                Ok(A2ATask::new(task_id, "ctx-default"))
            }

            async fn list_tasks(&self, _params: ListTasksParams) -> A2AResult<ListTasksResult> {
                Ok(ListTasksResult {
                    tasks: vec![],
                    next_cursor: None,
                })
            }

            async fn add_artifact(&self, _task_id: &str, _artifact: Artifact) -> A2AResult<()> {
                Ok(())
            }
        }

        // Minimal mock message handler
        struct MockMessageHandler;

        #[async_trait::async_trait]
        impl A2AMessageHandler for MockMessageHandler {
            async fn handle_message(
                &self,
                task_id: &str,
                _message: A2AMessage,
                _session_id: Option<&str>,
            ) -> A2AResult<A2ATask> {
                Ok(A2ATask::new(task_id, "ctx-msg"))
            }

            async fn handle_message_stream(
                &self,
                _task_id: &str,
                _message: A2AMessage,
                _session_id: Option<&str>,
            ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>> {
                Ok(Box::pin(futures::stream::empty()))
            }
        }

        // Minimal mock streaming handler
        struct MockStreamingHandler;

        #[async_trait::async_trait]
        impl A2AStreamingHandler for MockStreamingHandler {
            async fn subscribe_all(
                &self,
                _task_id: &str,
            ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>> {
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

        fn make_state() -> Arc<A2AServerState> {
            Arc::new(A2AServerState {
                task_manager: Arc::new(MockTaskManager),
                message_handler: Arc::new(MockMessageHandler),
                streaming: Arc::new(MockStreamingHandler),
                authenticator: Arc::new(AllowAllAuth),
                notification: Arc::new(NotificationService::new()),
                card: AgentCard {
                    id: "test".to_string(),
                    name: "Test Agent".to_string(),
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

        fn make_auth() -> A2AAuthPrincipal {
            A2AAuthPrincipal {
                agent_id: None,
                trust_level: TrustLevel::Local,
                permissions: vec![],
            }
        }

        #[tokio::test]
        async fn unknown_method_returns_method_not_found() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "nonexistent/method".to_string(),
                params: Value::Null,
                id: Some(Value::Number(1.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_some());
            let err = resp.error.unwrap();
            assert_eq!(err["code"], -32601);
            assert!(err["message"]
                .as_str()
                .unwrap()
                .contains("nonexistent/method"));
        }

        #[tokio::test]
        async fn tasks_get_dispatches_correctly() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tasks/get".to_string(),
                params: serde_json::json!({"id": "task-1"}),
                id: Some(Value::Number(2.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());
            let result = resp.result.unwrap();
            assert_eq!(result["id"], "task-1");
        }

        #[tokio::test]
        async fn extended_card_dispatches_returns_card() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "agent/getAuthenticatedExtendedCard".to_string(),
                params: Value::Null,
                id: Some(Value::Number(42.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());
            let result = resp.result.unwrap();
            assert_eq!(result["id"], "test");
            assert_eq!(result["name"], "Test Agent");
        }

        #[tokio::test]
        async fn tasks_get_missing_id_returns_error() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tasks/get".to_string(),
                params: serde_json::json!({}),
                id: Some(Value::Number(3.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_some());
            assert_eq!(resp.error.unwrap()["code"], -32602);
        }

        #[tokio::test]
        async fn tasks_cancel_dispatches_correctly() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tasks/cancel".to_string(),
                params: serde_json::json!({"id": "task-cancel-1"}),
                id: Some(Value::Number(4.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());
            assert_eq!(resp.result.unwrap()["id"], "task-cancel-1");
        }

        #[tokio::test]
        async fn tasks_list_empty() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tasks/list".to_string(),
                params: Value::Null,
                id: Some(Value::Number(5.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());
            let result = resp.result.unwrap();
            assert_eq!(result["tasks"], serde_json::json!([]));
        }

        #[tokio::test]
        async fn message_send_dispatches_correctly() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "message/send".to_string(),
                params: serde_json::json!({
                    "message": {
                        "messageId": "m1",
                        "role": "user",
                        "parts": [{"type": "text", "text": "hello"}]
                    },
                    "taskId": "task-msg-1"
                }),
                id: Some(Value::Number(6.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());
            assert_eq!(resp.result.unwrap()["id"], "task-msg-1");
        }

        #[tokio::test]
        async fn message_send_generates_task_id_if_missing() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "message/send".to_string(),
                params: serde_json::json!({
                    "message": {
                        "messageId": "m2",
                        "role": "user",
                        "parts": [{"type": "text", "text": "hello"}]
                    }
                }),
                id: Some(Value::Number(7.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());
            // Task ID should be a valid UUID
            let task_id = resp.result.unwrap()["id"].as_str().unwrap().to_string();
            assert!(!task_id.is_empty());
            assert!(uuid::Uuid::parse_str(&task_id).is_ok());
        }

        #[tokio::test]
        async fn message_send_missing_message_returns_error() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "message/send".to_string(),
                params: serde_json::json!({}),
                id: Some(Value::Number(8.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_some());
            assert_eq!(resp.error.unwrap()["code"], -32602);
        }

        #[tokio::test]
        async fn forbidden_when_auth_denies() {
            // Build state with a denying authenticator
            struct DenyAllAuth;

            #[async_trait::async_trait]
            impl A2AAuthenticator for DenyAllAuth {
                async fn authenticate(
                    &self,
                    _context: &A2AAuthContext,
                ) -> A2AResult<A2AAuthPrincipal> {
                    Ok(A2AAuthPrincipal {
                        agent_id: None,
                        trust_level: TrustLevel::Public,
                        permissions: vec![],
                    })
                }

                async fn authorize(
                    &self,
                    _principal: &A2AAuthPrincipal,
                    _action: &A2AAction,
                ) -> A2AResult<bool> {
                    Ok(false)
                }

                fn supported_schemes(&self) -> Vec<SecurityScheme> {
                    vec![]
                }
            }

            let state = Arc::new(A2AServerState {
                task_manager: Arc::new(MockTaskManager),
                message_handler: Arc::new(MockMessageHandler),
                streaming: Arc::new(MockStreamingHandler),
                authenticator: Arc::new(DenyAllAuth),
                notification: Arc::new(NotificationService::new()),
                card: AgentCard {
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
            });

            let processor = A2ARequestProcessor::new(state);
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tasks/get".to_string(),
                params: serde_json::json!({"id": "task-1"}),
                id: Some(Value::Number(1.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_some());
            let err = resp.error.unwrap();
            assert_eq!(err["code"], -32005); // Forbidden
        }

        #[tokio::test]
        async fn invalid_jsonrpc_version_returns_invalid_request() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: "1.0".to_string(),
                method: "message/send".to_string(),
                params: serde_json::json!({
                    "message": {
                        "messageId": "m1",
                        "role": "user",
                        "parts": [{"type": "text", "text": "hi"}]
                    }
                }),
                id: Some(Value::Number(1.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.result.is_none());
            let err = resp.error.expect("invalid jsonrpc version must error");
            assert_eq!(err["code"], -32600);
        }

        #[tokio::test]
        async fn missing_jsonrpc_version_returns_invalid_request() {
            let processor = A2ARequestProcessor::new(make_state());
            let request = JsonRpcRequest {
                jsonrpc: String::new(),
                method: "tasks/get".to_string(),
                params: serde_json::json!({"id": "task-1"}),
                id: Some(Value::Number(2.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_some());
            assert_eq!(resp.error.unwrap()["code"], -32600);
        }

        #[tokio::test]
        async fn message_send_with_push_notification_config_registers_config() {
            let state = make_state();
            let processor = A2ARequestProcessor::new(Arc::clone(&state));
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "message/send".to_string(),
                params: serde_json::json!({
                    "message": {
                        "messageId": "m-push",
                        "role": "user",
                        "parts": [{"type": "text", "text": "hi"}]
                    },
                    "taskId": "task-push-1",
                    "pushNotificationConfig": {
                        "url": "https://8.8.8.8/notify",
                        "token": "secret-token",
                        "events": ["status-update"]
                    }
                }),
                id: Some(Value::Number(11.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none(), "message/send should succeed");

            let stored = state
                .notification
                .get_config("task-push-1")
                .await
                .expect("get_config should not error")
                .expect("push config should be registered under task-push-1");
            assert_eq!(stored.url, "https://8.8.8.8/notify");
            assert_eq!(stored.token.as_deref(), Some("secret-token"));
            assert_eq!(stored.events, vec!["status-update".to_string()]);
        }

        #[tokio::test]
        async fn message_send_generates_task_id_registers_push_config_under_generated_id() {
            let state = make_state();
            let processor = A2ARequestProcessor::new(Arc::clone(&state));
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "message/send".to_string(),
                params: serde_json::json!({
                    "message": {
                        "messageId": "m-push-2",
                        "role": "user",
                        "parts": [{"type": "text", "text": "hi"}]
                    },
                    "pushNotificationConfig": {
                        "url": "https://8.8.8.8/notify"
                    }
                }),
                id: Some(Value::Number(12.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());
            let task_id = resp.result.unwrap()["id"].as_str().unwrap().to_string();
            assert!(uuid::Uuid::parse_str(&task_id).is_ok());

            let stored = state
                .notification
                .get_config(&task_id)
                .await
                .expect("get_config should not error")
                .expect("push config should be registered under generated task id");
            assert_eq!(stored.url, "https://8.8.8.8/notify");
            assert!(stored.token.is_none());
        }

        #[tokio::test]
        async fn message_send_without_push_config_does_not_register() {
            let state = make_state();
            let processor = A2ARequestProcessor::new(Arc::clone(&state));
            let request = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "message/send".to_string(),
                params: serde_json::json!({
                    "message": {
                        "messageId": "m-nopush",
                        "role": "user",
                        "parts": [{"type": "text", "text": "hi"}]
                    },
                    "taskId": "task-nopush"
                }),
                id: Some(Value::Number(13.into())),
            };
            let resp = processor.process(request, make_auth()).await;
            assert!(resp.error.is_none());

            let stored = state
                .notification
                .get_config("task-nopush")
                .await
                .expect("get_config should not error");
            assert!(stored.is_none(), "no push config must be stored");
        }
    }
}
