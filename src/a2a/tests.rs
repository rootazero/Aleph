//! Integration tests for the A2A protocol subsystem.
//!
//! These tests verify end-to-end behavior across multiple A2A components
//! working together, complementing the unit tests in each module.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use futures::Stream;
use tokio_stream::StreamExt;
use tower::ServiceExt;

use crate::a2a::adapter::auth::TieredAuthenticator;
use crate::a2a::adapter::server::request_processor::{
    A2ARequestProcessor, A2AServerState, JsonRpcRequest,
};
use crate::a2a::adapter::server::routes::a2a_routes;
use crate::a2a::adapter::server::{StreamHub, TaskStore};
use crate::a2a::config::{A2ASecurityConfig, A2AServerConfig, A2ASkillConfig};
use crate::a2a::domain::*;
use crate::a2a::port::authenticator::{
    A2AAction, A2AAuthContext, A2AAuthPrincipal, A2AAuthenticator,
};
use crate::a2a::port::message_handler::A2AMessageHandler;
use crate::a2a::port::streaming::A2AStreamingHandler;
use crate::a2a::port::task_manager::{A2AResult, A2ATaskManager};
use crate::a2a::service::card_builder::CardBuilder;
use crate::a2a::service::card_registry::CardRegistry;
use crate::a2a::service::notification::NotificationService;
use crate::a2a::service::smart_router::{RoutingMethod, SmartRouter};

// ============================================================
// Test helpers
// ============================================================

/// Build a default AgentCard for testing
fn test_agent_card() -> AgentCard {
    AgentCard {
        id: "test-agent".to_string(),
        name: "Test Agent".to_string(),
        version: "0.1.0".to_string(),
        description: Some("Integration test agent".to_string()),
        provider: Some(AgentProvider {
            name: "Aleph".to_string(),
            url: None,
        }),
        documentation_url: None,
        interfaces: vec![AgentInterface {
            url: "http://127.0.0.1:8080/a2a".to_string(),
            protocol: TransportProtocol::JsonRpc,
        }],
        skills: vec![AgentSkill {
            id: "general".to_string(),
            name: "General".to_string(),
            description: Some("General purpose".to_string()),
            aliases: None,
            input_types: Some(vec!["text".to_string()]),
            output_types: Some(vec!["text".to_string()]),
        }],
        security: vec![],
        extensions: vec![],
        default_input_modes: vec!["text".to_string()],
        default_output_modes: vec!["text".to_string()],
    }
}

/// Stub message handler that immediately completes the task
struct StubMessageHandler {
    task_manager: Arc<dyn A2ATaskManager>,
}

#[async_trait::async_trait]
impl A2AMessageHandler for StubMessageHandler {
    async fn handle_message(
        &self,
        task_id: &str,
        _message: A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<A2ATask> {
        let context_id = session_id.unwrap_or(task_id);
        let _task = self.task_manager.create_task(task_id, context_id).await?;
        let response = A2AMessage::text(A2ARole::Agent, "Stub response");
        self.task_manager
            .update_status(task_id, TaskState::Completed, Some(response))
            .await
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

/// AllowAll authenticator for tests that don't care about auth
struct AllowAllAuth;

#[async_trait::async_trait]
impl A2AAuthenticator for AllowAllAuth {
    async fn authenticate(&self, _context: &A2AAuthContext) -> A2AResult<A2AAuthPrincipal> {
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

/// Build a fully wired A2AServerState for integration tests
fn test_server_state() -> Arc<A2AServerState> {
    let task_store: Arc<TaskStore> = Arc::new(TaskStore::new());
    let stream_hub = Arc::new(StreamHub::new());
    let message_handler = Arc::new(StubMessageHandler {
        task_manager: Arc::clone(&task_store) as Arc<dyn A2ATaskManager>,
    });

    Arc::new(A2AServerState {
        task_manager: task_store,
        message_handler,
        streaming: stream_hub,
        authenticator: Arc::new(AllowAllAuth),
        notification: Arc::new(NotificationService::new()),
        card: test_agent_card(),
    })
}

fn localhost_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
}

fn remote_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 8080)
}

fn make_auth_context(addr: SocketAddr, creds: Credentials) -> A2AAuthContext {
    A2AAuthContext {
        remote_addr: addr,
        headers: HashMap::new(),
        credentials: creds,
    }
}

fn local_auth_principal() -> A2AAuthPrincipal {
    A2AAuthPrincipal {
        agent_id: None,
        trust_level: TrustLevel::Local,
        permissions: vec!["*".to_string()],
    }
}

// ============================================================
// Test 1: Agent Card Discovery (CardBuilder)
// ============================================================

#[tokio::test]
async fn test_agent_card_discovery() {
    let config = A2AServerConfig {
        card_name: Some("Integration Test Agent".to_string()),
        card_description: Some("An agent for testing".to_string()),
        card_version: Some("0.5.0".to_string()),
        security: A2ASecurityConfig {
            local_bypass: true,
            tokens: vec!["secret-token".to_string()],
        },
        skills: vec![A2ASkillConfig {
            id: "test-skill".to_string(),
            name: "Test Skill".to_string(),
            description: Some("A test skill".to_string()),
        }],
    };

    let card = CardBuilder::build(&config, "127.0.0.1:8080");

    // Verify card fields
    assert_eq!(card.name, "Integration Test Agent");
    assert_eq!(card.description.as_deref(), Some("An agent for testing"));
    assert_eq!(card.version, "0.5.0");
    assert!(card.id.starts_with("aleph-"));

    // Verify interface
    assert_eq!(card.interfaces.len(), 1);
    assert_eq!(card.interfaces[0].url, "http://127.0.0.1:8080/a2a");

    // Verify skills
    assert_eq!(card.skills.len(), 1);
    assert_eq!(card.skills[0].id, "test-skill");
    assert_eq!(card.skills[0].name, "Test Skill");

    // Verify security scheme (tokens configured -> bearer scheme present)
    assert_eq!(card.security.len(), 1);
    assert!(matches!(
        &card.security[0],
        SecurityScheme::Http { scheme, .. } if scheme == "bearer"
    ));

    // Verify provider
    assert!(card.provider.is_some());
    assert_eq!(card.provider.unwrap().name, "Aleph");
}

// ============================================================
// Test 2: TaskStore CRUD lifecycle
// ============================================================

#[tokio::test]
async fn test_task_crud_lifecycle() {
    let store = TaskStore::new();

    // Create
    let task = store.create_task("task-1", "ctx-1").await.unwrap();
    assert_eq!(task.id, "task-1");
    assert_eq!(task.context_id, "ctx-1");
    assert_eq!(task.status.state, TaskState::Submitted);

    // Get
    let task = store.get_task("task-1", None).await.unwrap();
    assert_eq!(task.id, "task-1");

    // Update: Submitted -> Working
    let msg = A2AMessage::text(A2ARole::Agent, "Processing...");
    let task = store
        .update_status("task-1", TaskState::Working, Some(msg))
        .await
        .unwrap();
    assert_eq!(task.status.state, TaskState::Working);
    assert_eq!(task.history.len(), 1);

    // Update: Working -> Completed
    let msg = A2AMessage::text(A2ARole::Agent, "Done!");
    let task = store
        .update_status("task-1", TaskState::Completed, Some(msg))
        .await
        .unwrap();
    assert_eq!(task.status.state, TaskState::Completed);
    assert_eq!(task.history.len(), 2);

    // Cancel completed task -> should fail
    let err = store.cancel_task("task-1").await.unwrap_err();
    assert!(matches!(
        err,
        A2AError::TaskNotCancelable(TaskState::Completed)
    ));

    // List tasks
    let result = store.list_tasks(ListTasksParams::default()).await.unwrap();
    assert_eq!(result.tasks.len(), 1);
    assert_eq!(result.tasks[0].id, "task-1");
}

// ============================================================
// Test 3: StreamHub pub/sub
// ============================================================

#[tokio::test]
async fn test_stream_hub_pubsub() {
    let hub = StreamHub::new();

    // Subscribe first
    let mut stream = hub.subscribe_all("task-1").await.unwrap();

    // Broadcast a status update
    let event = TaskStatusUpdateEvent {
        task_id: "task-1".to_string(),
        context_id: "ctx-1".to_string(),
        status: TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: Utc::now(),
        },
        is_final: false,
        metadata: None,
    };
    hub.broadcast_status("task-1", event).await.unwrap();

    // Verify subscriber receives it
    let received = stream.next().await.unwrap().unwrap();
    match received {
        UpdateEvent::StatusUpdate(e) => {
            assert_eq!(e.task_id, "task-1");
            assert_eq!(e.status.state, TaskState::Working);
            assert!(!e.is_final);
        }
        _ => unreachable!("Expected StatusUpdate event"),
    }

    // Broadcast a final event
    let final_event = TaskStatusUpdateEvent {
        task_id: "task-1".to_string(),
        context_id: "ctx-1".to_string(),
        status: TaskStatus {
            state: TaskState::Completed,
            message: None,
            timestamp: Utc::now(),
        },
        is_final: true,
        metadata: None,
    };
    hub.broadcast_status("task-1", final_event).await.unwrap();

    let received = stream.next().await.unwrap().unwrap();
    match received {
        UpdateEvent::StatusUpdate(e) => {
            assert_eq!(e.status.state, TaskState::Completed);
            assert!(e.is_final);
        }
        _ => unreachable!("Expected final StatusUpdate"),
    }
}

// ============================================================
// Test 4: TieredAuth - Localhost bypass
// ============================================================

#[tokio::test]
async fn test_tiered_auth_localhost_bypass() {
    let auth = TieredAuthenticator::new(true, vec![]);
    let ctx = make_auth_context(localhost_addr(), Credentials::None);

    let principal = auth.authenticate(&ctx).await.unwrap();
    assert_eq!(principal.trust_level, TrustLevel::Local);
    assert!(principal.permissions.contains(&"*".to_string()));

    // Should be authorized for all actions
    assert!(auth
        .authorize(&principal, &A2AAction::SendMessage)
        .await
        .unwrap());
    assert!(auth
        .authorize(&principal, &A2AAction::GetTask)
        .await
        .unwrap());
    assert!(auth
        .authorize(&principal, &A2AAction::CancelTask)
        .await
        .unwrap());
    assert!(auth
        .authorize(&principal, &A2AAction::ListTasks)
        .await
        .unwrap());
    assert!(auth
        .authorize(&principal, &A2AAction::Subscribe)
        .await
        .unwrap());
}

// ============================================================
// Test 5: TieredAuth - Token validation
// ============================================================

#[tokio::test]
async fn test_tiered_auth_token_validation() {
    let auth = TieredAuthenticator::new(false, vec!["valid-token-42".to_string()]);

    // Valid bearer token from remote address
    let ctx = make_auth_context(
        remote_addr(),
        Credentials::BearerToken("valid-token-42".to_string()),
    );
    let principal = auth.authenticate(&ctx).await.unwrap();
    assert_eq!(principal.trust_level, TrustLevel::Trusted);
    assert!(principal.permissions.contains(&"*".to_string()));

    // Invalid bearer token should reject
    let ctx = make_auth_context(
        remote_addr(),
        Credentials::BearerToken("wrong-token".to_string()),
    );
    let err = auth.authenticate(&ctx).await.unwrap_err();
    assert!(matches!(err, A2AError::Unauthorized));
}

// ============================================================
// Test 6: TieredAuth - Rejection
// ============================================================

#[tokio::test]
async fn test_tiered_auth_rejection() {
    let auth = TieredAuthenticator::new(false, vec!["some-token".to_string()]);

    // No credentials from non-localhost -> reject
    let ctx = make_auth_context(remote_addr(), Credentials::None);
    let err = auth.authenticate(&ctx).await.unwrap_err();
    assert!(matches!(err, A2AError::Unauthorized));
}

// ============================================================
// Test 7: SmartRouter exact name match
// ============================================================

#[tokio::test]
async fn test_smart_router_exact_match() {
    let registry = CardRegistry::new();

    // Register an agent named "交易助手"
    let card = AgentCard {
        id: "trading-agent".to_string(),
        name: "交易助手".to_string(),
        version: "1.0.0".to_string(),
        description: Some("Trading assistant".to_string()),
        provider: None,
        documentation_url: None,
        interfaces: vec![],
        skills: vec![],
        security: vec![],
        extensions: vec![],
        default_input_modes: vec!["text".to_string()],
        default_output_modes: vec!["text".to_string()],
    };

    use crate::a2a::port::AgentResolver;
    registry
        .register(card, "http://localhost:9000", TrustLevel::Trusted)
        .await
        .unwrap();

    let registry: Arc<dyn AgentResolver> = Arc::new(registry);
    let router = SmartRouter::new(registry);

    // Route with Chinese quoted name -> should match with high confidence
    let decision = router
        .route("请使用「交易助手」分析黄金走势")
        .await
        .unwrap();

    assert!(decision.is_some());
    let decision = decision.unwrap();
    assert_eq!(decision.agent.card.name, "交易助手");
    assert_eq!(decision.confidence, 1.0);
    assert_eq!(decision.method, RoutingMethod::ExactName);
}

// ============================================================
// Test 8: RequestProcessor method dispatch
// ============================================================

#[tokio::test]
async fn test_request_processor_dispatch() {
    let state = test_server_state();
    let processor = A2ARequestProcessor::new(Arc::clone(&state));
    let auth = local_auth_principal();

    // Test message/send
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "message/send".to_string(),
        params: serde_json::json!({
            "message": {
                "messageId": "msg-1",
                "role": "user",
                "parts": [{"type": "text", "text": "Hello, agent!"}]
            },
            "taskId": "task-send-1"
        }),
        id: Some(serde_json::Value::Number(1.into())),
    };

    let resp = processor.process(request, auth.clone()).await;
    assert!(
        resp.error.is_none(),
        "Expected success, got error: {:?}",
        resp.error
    );
    let result = resp.result.unwrap();
    assert_eq!(result["id"], "task-send-1");
    assert_eq!(result["status"]["state"], "completed");

    // Test tasks/get (task created by message/send above)
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tasks/get".to_string(),
        params: serde_json::json!({"id": "task-send-1"}),
        id: Some(serde_json::Value::Number(2.into())),
    };

    let resp = processor.process(request, auth.clone()).await;
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["id"], "task-send-1");

    // Test tasks/list
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tasks/list".to_string(),
        params: serde_json::Value::Null,
        id: Some(serde_json::Value::Number(3.into())),
    };

    let resp = processor.process(request, auth.clone()).await;
    assert!(resp.error.is_none());
    let tasks = &resp.result.unwrap()["tasks"];
    assert!(!tasks.as_array().unwrap().is_empty());

    // Test unknown method
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "nonexistent/method".to_string(),
        params: serde_json::Value::Null,
        id: Some(serde_json::Value::Number(4.into())),
    };

    let resp = processor.process(request, auth).await;
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap()["code"], -32601);
}

// ============================================================
// Test 9: Routes - Agent Card endpoint
// ============================================================

#[tokio::test]
async fn test_routes_agent_card_endpoint() {
    let state = test_server_state();
    let app = a2a_routes(state);

    let request = Request::builder()
        .method("GET")
        .uri("/.well-known/agent-card.json")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let card: AgentCard = serde_json::from_slice(&body).unwrap();

    assert_eq!(card.id, "test-agent");
    assert_eq!(card.name, "Test Agent");
    assert_eq!(card.version, "0.1.0");
    assert_eq!(card.skills.len(), 1);
    assert_eq!(card.skills[0].id, "general");
    assert_eq!(card.interfaces.len(), 1);
    assert!(card.provider.is_some());
}

// ============================================================
// Test 10: Routes - JSON-RPC sync endpoint (via RequestProcessor)
// ============================================================

#[tokio::test]
async fn test_routes_jsonrpc_sync_endpoint() {
    // Since the /a2a POST route uses fallback_addr (127.0.0.1:0) and
    // AllowAllAuth, we can test through the actual router.
    let state = test_server_state();
    let app = a2a_routes(state);

    let rpc_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "message/send",
        "params": {
            "message": {
                "messageId": "m-route-1",
                "role": "user",
                "parts": [{"type": "text", "text": "Hello from route test"}]
            },
            "taskId": "task-route-1"
        },
        "id": 1
    });

    let request = Request::builder()
        .method("POST")
        .uri("/a2a")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&rpc_body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let rpc_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(rpc_resp["jsonrpc"], "2.0");
    assert!(rpc_resp.get("error").is_none() || rpc_resp["error"].is_null());
    assert_eq!(rpc_resp["result"]["id"], "task-route-1");
    assert_eq!(rpc_resp["result"]["status"]["state"], "completed");
}

// ============================================================
// Test 10b: Streaming endpoint accepts canonical `message/stream`
// (with `message/send` kept as a back-compat alias)
// ============================================================

async fn stream_endpoint_body(method: &str) -> (StatusCode, String) {
    let state = test_server_state();
    let app = a2a_routes(state);

    let rpc_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {
            "message": {
                "messageId": "m-stream-1",
                "role": "user",
                "parts": [{"type": "text", "text": "stream please"}]
            },
            "taskId": "task-stream-1"
        },
        "id": 1
    });

    let request = Request::builder()
        .method("POST")
        .uri("/a2a/stream")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&rpc_body).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).to_string())
}

#[tokio::test]
async fn test_stream_endpoint_accepts_message_stream() {
    // Canonical A2A streaming method name must be dispatched to the
    // stream handler, not rejected as unsupported.
    let (status, body) = stream_endpoint_body("message/stream").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("does not support streaming"),
        "message/stream must be accepted, got body: {body}"
    );
}

#[tokio::test]
async fn test_stream_endpoint_accepts_message_send_alias() {
    // Back-compat alias for older Aleph clients still streams.
    let (status, body) = stream_endpoint_body("message/send").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("does not support streaming"),
        "message/send alias must still be accepted, got body: {body}"
    );
}

#[tokio::test]
async fn test_stream_endpoint_rejects_unknown_method() {
    // A non-streaming method belongs on the synchronous /a2a endpoint.
    let (_status, body) = stream_endpoint_body("tasks/get").await;
    assert!(
        body.contains("does not support streaming"),
        "non-streaming method must be rejected on /a2a/stream, got body: {body}"
    );
}

// ============================================================
// End-to-end routing chain tests
// ============================================================

use crate::a2a::port::{AgentHealth, AgentResolver, RegisteredAgent};
use crate::a2a::service::llm_matcher::SemanticLlmMatcher;
use crate::a2a::service::smart_router::LlmMatcher;
use crate::providers::AiProvider;

/// Mock AiProvider that returns a configurable response string.
/// Used to simulate LLM routing responses in tests.
struct MockRoutingProvider {
    response: String,
}

impl MockRoutingProvider {
    fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }
}

impl AiProvider for MockRoutingProvider {
    fn process(
        &self,
        _payload: crate::providers::adapter::RequestPayload<'_>,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                > + Send
                + '_,
        >,
    > {
        let response = self.response.clone();
        Box::pin(async move {
            Ok(crate::providers::adapter::ProviderResponse::text_only(
                response,
            ))
        })
    }

    fn name(&self) -> &str {
        "mock-routing"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

/// Mock AgentResolver returning a configurable list of agents.
struct MockAgentResolver {
    agents: tokio::sync::Mutex<Vec<RegisteredAgent>>,
}

impl MockAgentResolver {
    fn new(agents: Vec<RegisteredAgent>) -> Self {
        Self {
            agents: tokio::sync::Mutex::new(agents),
        }
    }
}

#[async_trait::async_trait]
impl AgentResolver for MockAgentResolver {
    async fn register(
        &self,
        _card: crate::a2a::domain::AgentCard,
        _base_url: &str,
        _trust_level: TrustLevel,
    ) -> crate::a2a::port::task_manager::A2AResult<()> {
        Ok(())
    }

    async fn unregister(&self, _agent_id: &str) -> crate::a2a::port::task_manager::A2AResult<()> {
        Ok(())
    }

    async fn list_agents(&self) -> crate::a2a::port::task_manager::A2AResult<Vec<RegisteredAgent>> {
        let agents = self.agents.lock().await;
        Ok(agents.clone())
    }

    async fn resolve_by_id(
        &self,
        _agent_id: &str,
    ) -> crate::a2a::port::task_manager::A2AResult<Option<RegisteredAgent>> {
        Ok(None)
    }
}

/// Helper: build a RegisteredAgent for e2e tests
fn e2e_make_agent(
    id: &str,
    name: &str,
    description: Option<&str>,
    skills: Vec<AgentSkill>,
) -> RegisteredAgent {
    let card = AgentCard {
        id: id.to_string(),
        name: name.to_string(),
        version: "1.0.0".to_string(),
        description: description.map(|s| s.to_string()),
        provider: None,
        documentation_url: None,
        interfaces: vec![],
        skills,
        security: vec![],
        extensions: vec![],
        default_input_modes: vec!["text".to_string()],
        default_output_modes: vec!["text".to_string()],
    };
    RegisteredAgent::new(
        card,
        TrustLevel::Trusted,
        format!("http://localhost:9000/{}", id),
        Utc::now(),
        AgentHealth::Healthy,
        None,
    )
}

// ============================================================
// Test 11: SemanticLlmMatcher successful routing
// ============================================================

#[tokio::test]
async fn test_semantic_llm_matcher_successful_routing() {
    let mock_response = "\
AGENT_INDEX: 0
CONFIDENCE: 0.85
REASON: The trading assistant can analyze gold trends";

    let provider = Arc::new(MockRoutingProvider::new(mock_response));
    let matcher = SemanticLlmMatcher::new(Arc::new(crate::providers::StaticDefault::new(
        provider as Arc<dyn crate::providers::AiProvider>,
    )));

    let agents = vec![e2e_make_agent(
        "trading-assistant",
        "Trading Assistant",
        Some("Analyzes financial markets and trading trends"),
        vec![],
    )];

    let decision = matcher.match_intent("分析黄金走势", &agents).await;

    assert!(decision.is_some(), "Expected a routing decision");
    let decision = decision.unwrap();
    assert_eq!(decision.agent.card.name, "Trading Assistant");
    assert!(
        (decision.confidence - 0.85).abs() < f64::EPSILON,
        "Expected confidence 0.85, got {}",
        decision.confidence
    );
    assert_eq!(decision.method, RoutingMethod::LlmSemantic);
    assert!(decision
        .reason
        .as_ref()
        .unwrap()
        .contains("trading assistant"));
}

// ============================================================
// Test 12: SmartRouter with LLM fallback
// ============================================================

#[tokio::test]
async fn test_smart_router_with_llm_fallback() {
    let agent = e2e_make_agent(
        "financial-analyst",
        "Financial Analyst",
        Some("Analyzes market data and financial reports"),
        vec![],
    );

    let resolver: Arc<dyn AgentResolver> = Arc::new(MockAgentResolver::new(vec![agent]));

    // SemanticLlmMatcher backed by mock provider that returns index 0
    let mock_response = "\
AGENT_INDEX: 0
CONFIDENCE: 0.75
REASON: Financial analyst can help with market data analysis";

    let provider = Arc::new(MockRoutingProvider::new(mock_response));
    let llm_matcher: Arc<dyn LlmMatcher> = Arc::new(SemanticLlmMatcher::new(Arc::new(
        crate::providers::StaticDefault::new(provider as Arc<dyn crate::providers::AiProvider>),
    )));

    let router = SmartRouter::new(resolver).with_llm_matcher(llm_matcher);

    // "help me analyze market data" won't exact-match "Financial Analyst" name
    // (intent doesn't contain "financial analyst"), so it should fall through
    // to the LLM matcher.
    let decision = router.route("help me analyze market data").await.unwrap();

    assert!(
        decision.is_some(),
        "Expected LLM fallback to produce a decision"
    );
    let decision = decision.unwrap();
    assert_eq!(decision.agent.card.name, "Financial Analyst");
    assert_eq!(decision.method, RoutingMethod::LlmSemantic);
    assert!(
        (decision.confidence - 0.75).abs() < f64::EPSILON,
        "Expected confidence 0.75, got {}",
        decision.confidence
    );
}
