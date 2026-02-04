//! Integration tests for sessions tools (sessions_list, sessions_send)
//!
//! These tests verify the full workflow of session tools working together
//! with the gateway infrastructure.
//!
//! Tests cover:
//! 1. sessions_list with various filters
//! 2. sessions_send fire-and-forget mode
//! 3. sessions_send with wait mode (using mocks)
//! 4. A2A policy enforcement through the full stack

#![cfg(feature = "gateway")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use tempfile::tempdir;

use alephcore::builtin_tools::sessions::{
    SessionsListArgs, SessionsListTool, SessionsSendArgs, SessionsSendStatus, SessionsSendTool,
};
use alephcore::gateway::a2a_policy::AgentToAgentPolicy;
use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use alephcore::gateway::context::GatewayContext;
use alephcore::gateway::event_emitter::EventEmitter;
use alephcore::gateway::execution_adapter::ExecutionAdapter;
use alephcore::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
use alephcore::gateway::router::SessionKey;
use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};
use alephcore::tools::AlephTool;

// ============================================================================
// Test Infrastructure
// ============================================================================

/// Mock execution adapter that tracks invocations and can simulate various behaviors
struct TestExecutionAdapter {
    /// Number of times execute was called
    execute_count: AtomicUsize,
    /// Whether to fail execution
    should_fail: AtomicBool,
    /// Response to return (for simulating replies)
    mock_response: Option<String>,
}

impl TestExecutionAdapter {
    fn new() -> Self {
        Self {
            execute_count: AtomicUsize::new(0),
            should_fail: AtomicBool::new(false),
            mock_response: None,
        }
    }

    fn failing() -> Self {
        let adapter = Self::new();
        adapter.should_fail.store(true, Ordering::SeqCst);
        adapter
    }

    fn call_count(&self) -> usize {
        self.execute_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ExecutionAdapter for TestExecutionAdapter {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        self.execute_count.fetch_add(1, Ordering::SeqCst);

        if self.should_fail.load(Ordering::SeqCst) {
            Err(ExecutionError::Failed("Test failure".to_string()))
        } else {
            // Simulate a brief execution
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            Ok(())
        }
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        Err(ExecutionError::RunNotFound(run_id.to_string()))
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        Some(RunStatus {
            run_id: run_id.to_string(),
            state: RunState::Completed,
            started_at: Some(chrono::Utc::now()),
            completed_at: Some(chrono::Utc::now()),
            steps_completed: 1,
            current_tool: self.mock_response.clone(),
        })
    }
}

/// Create a test context with customizable A2A policy
fn create_test_context(
    temp_path: std::path::PathBuf,
    a2a_policy: AgentToAgentPolicy,
) -> (Arc<GatewayContext>, Arc<TestExecutionAdapter>) {
    let session_config = SessionManagerConfig {
        db_path: temp_path.join("sessions.db"),
        ..Default::default()
    };
    let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
    let agent_registry = Arc::new(AgentRegistry::new());
    let execution_adapter = Arc::new(TestExecutionAdapter::new());
    let a2a_policy = Arc::new(a2a_policy);

    let context = Arc::new(GatewayContext::new(
        session_manager,
        agent_registry,
        execution_adapter.clone(),
        a2a_policy,
    ));

    (context, execution_adapter)
}

/// Create a test context with permissive A2A policy
fn create_permissive_context(
    temp_path: std::path::PathBuf,
) -> (Arc<GatewayContext>, Arc<TestExecutionAdapter>) {
    create_test_context(temp_path, AgentToAgentPolicy::permissive())
}

/// Create a test context with a custom execution adapter
fn create_context_with_adapter(
    temp_path: std::path::PathBuf,
    a2a_policy: AgentToAgentPolicy,
    adapter: Arc<TestExecutionAdapter>,
) -> Arc<GatewayContext> {
    let session_config = SessionManagerConfig {
        db_path: temp_path.join("sessions.db"),
        ..Default::default()
    };
    let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
    let agent_registry = Arc::new(AgentRegistry::new());

    Arc::new(GatewayContext::new(
        session_manager,
        agent_registry,
        adapter,
        Arc::new(a2a_policy),
    ))
}

/// Register a test agent in the registry
async fn register_test_agent(
    context: &GatewayContext,
    agent_id: &str,
    temp_path: &std::path::Path,
) {
    let config = AgentInstanceConfig {
        agent_id: agent_id.to_string(),
        workspace: temp_path.join(format!("{}_workspace", agent_id)),
        ..Default::default()
    };
    let agent = AgentInstance::new(config).unwrap();
    context.agent_registry().register(agent).await;
}

// ============================================================================
// Sessions List Integration Tests
// ============================================================================

/// Test: List sessions returns empty when no sessions exist
#[tokio::test]
async fn test_list_sessions_empty() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    let tool = SessionsListTool::new(context, "main");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 0);
    assert!(result.sessions.is_empty());
}

/// Test: List sessions returns all created sessions
#[tokio::test]
async fn test_list_sessions_with_multiple_types() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    // Create sessions of different types
    let session_manager = context.session_manager();
    let key_main = SessionKey::main("main");
    let key_task = SessionKey::task("main", "cron", "daily-summary");
    let key_peer = SessionKey::peer("main", "user123");

    session_manager.get_or_create(&key_main).await.unwrap();
    session_manager.get_or_create(&key_task).await.unwrap();
    session_manager.get_or_create(&key_peer).await.unwrap();

    let tool = SessionsListTool::new(context, "main");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 3);
}

/// Test: Filter sessions by kind (only task sessions)
#[tokio::test]
async fn test_list_sessions_filter_by_kind_task() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("main"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::task("main", "cron", "task-1"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::task("main", "webhook", "task-2"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::peer("main", "user"))
        .await
        .unwrap();

    let tool = SessionsListTool::new(context, "main");
    let args = SessionsListArgs {
        kinds: Some(vec!["task".to_string()]),
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 2);
    for session in &result.sessions {
        assert_eq!(session.kind, "task");
    }
}

/// Test: Filter sessions by multiple kinds
#[tokio::test]
async fn test_list_sessions_filter_by_multiple_kinds() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("main"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::task("main", "cron", "task-1"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::ephemeral("main"))
        .await
        .unwrap();

    let tool = SessionsListTool::new(context, "main");
    let args = SessionsListArgs {
        kinds: Some(vec!["main".to_string(), "ephemeral".to_string()]),
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 2);

    let kinds: Vec<&str> = result.sessions.iter().map(|s| s.kind.as_str()).collect();
    assert!(kinds.contains(&"main"));
    assert!(kinds.contains(&"ephemeral"));
    assert!(!kinds.contains(&"task"));
}

/// Test: Limit the number of returned sessions
#[tokio::test]
async fn test_list_sessions_with_limit() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    let session_manager = context.session_manager();
    for i in 0..10 {
        let key = SessionKey::task("main", "cron", &format!("task-{}", i));
        session_manager.get_or_create(&key).await.unwrap();
    }

    let tool = SessionsListTool::new(context, "main");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(5),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 5);
}

/// Test: List sessions with message history
#[tokio::test]
async fn test_list_sessions_with_messages() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    let session_manager = context.session_manager();
    let key = SessionKey::main("main");
    session_manager.get_or_create(&key).await.unwrap();

    // Add some messages
    session_manager
        .add_message(&key, "user", "Hello!")
        .await
        .unwrap();
    session_manager
        .add_message(&key, "assistant", "Hi there!")
        .await
        .unwrap();
    session_manager
        .add_message(&key, "user", "How are you?")
        .await
        .unwrap();

    let tool = SessionsListTool::new(context, "main");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: Some(5),
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 1);

    let session = &result.sessions[0];
    assert!(session.messages.is_some());
    let messages = session.messages.as_ref().unwrap();
    assert_eq!(messages.len(), 3);
}

// ============================================================================
// A2A Policy Integration Tests with sessions_list
// ============================================================================

/// Test: Permissive policy allows listing all agent sessions
#[tokio::test]
async fn test_list_with_permissive_policy() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("main"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("work"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("personal"))
        .await
        .unwrap();

    // Agent "tester" should see all sessions with permissive policy
    let tool = SessionsListTool::new(context, "tester");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 3);
}

/// Test: Restrictive policy filters out unauthorized agent sessions
#[tokio::test]
async fn test_list_with_restrictive_policy() {
    let temp = tempdir().unwrap();
    // Policy: only allow access to "main" agent
    let policy = AgentToAgentPolicy::new(true, vec!["main".to_string()]);
    let (context, _adapter) = create_test_context(temp.path().to_path_buf(), policy);

    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("main"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("work"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("personal"))
        .await
        .unwrap();

    // Agent "tester" can only see "main" sessions
    let tool = SessionsListTool::new(context, "tester");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 1);
    assert!(result.sessions[0].key.contains("main"));
}

/// Test: Prefix pattern policy filters correctly
#[tokio::test]
async fn test_list_with_prefix_pattern_policy() {
    let temp = tempdir().unwrap();
    // Policy: allow access to "work-*" agents
    let policy = AgentToAgentPolicy::new(true, vec!["work-*".to_string()]);
    let (context, _adapter) = create_test_context(temp.path().to_path_buf(), policy);

    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("work-project1"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("work-project2"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("personal"))
        .await
        .unwrap();

    let tool = SessionsListTool::new(context, "tester");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 2);

    for session in &result.sessions {
        assert!(session.key.contains("work-"));
    }
}

/// Test: Disabled policy only allows same-agent communication
#[tokio::test]
async fn test_list_with_disabled_policy() {
    let temp = tempdir().unwrap();
    let policy = AgentToAgentPolicy::disabled();
    let (context, _adapter) = create_test_context(temp.path().to_path_buf(), policy);

    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("main"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("work"))
        .await
        .unwrap();

    // Agent "main" should only see its own sessions
    let tool = SessionsListTool::new(context, "main");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.count, 1);
    assert!(result.sessions[0].key.contains("agent:main:"));
}

// ============================================================================
// Sessions Send Integration Tests
// ============================================================================

/// Test: sessions_send without context returns error
#[tokio::test]
async fn test_send_without_context_returns_error() {
    let tool = SessionsSendTool::new();
    let args = SessionsSendArgs {
        session_key: Some("agent:main:main".to_string()),
        message: "Hello".to_string(),
        timeout_seconds: 0,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.status, SessionsSendStatus::Error);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("GatewayContext not configured"));
}

/// Test: sessions_send with invalid session key returns error
#[tokio::test]
async fn test_send_invalid_session_key_returns_error() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    let tool = SessionsSendTool::with_context((*context).clone(), "main");
    let args = SessionsSendArgs {
        session_key: Some("invalid:key:format".to_string()),
        message: "Hello".to_string(),
        timeout_seconds: 0,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.status, SessionsSendStatus::Error);
    assert!(result.error.as_ref().unwrap().contains("Invalid session key"));
}

/// Test: sessions_send with A2A policy denial returns forbidden
#[tokio::test]
async fn test_send_a2a_policy_denial() {
    let temp = tempdir().unwrap();
    // Policy: disabled, only same-agent allowed
    let policy = AgentToAgentPolicy::disabled();
    let (context, _adapter) = create_test_context(temp.path().to_path_buf(), policy);

    // Register target agent
    register_test_agent(&context, "translator", temp.path()).await;

    let tool = SessionsSendTool::with_context((*context).clone(), "main");
    let args = SessionsSendArgs {
        session_key: Some("agent:translator:main".to_string()),
        message: "Translate this".to_string(),
        timeout_seconds: 30,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.status, SessionsSendStatus::Forbidden);
    assert!(result.error.as_ref().unwrap().contains("A2A policy denies"));
}

/// Test: sessions_send fire-and-forget mode with permissive policy
#[tokio::test]
async fn test_send_fire_and_forget() {
    let temp = tempdir().unwrap();
    let adapter = Arc::new(TestExecutionAdapter::new());
    let context =
        create_context_with_adapter(temp.path().to_path_buf(), AgentToAgentPolicy::permissive(), adapter.clone());

    // Register target agent
    register_test_agent(&context, "main", temp.path()).await;

    let tool = SessionsSendTool::with_context((*context).clone(), "caller");
    let args = SessionsSendArgs {
        session_key: Some("agent:main:main".to_string()),
        message: "Fire and forget message".to_string(),
        timeout_seconds: 0, // Fire-and-forget
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.status, SessionsSendStatus::Accepted);
    assert!(result.session_key.is_some());
    assert!(result.reply.is_none()); // No reply in fire-and-forget mode

    // Give background task time to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify execution was triggered
    assert!(adapter.call_count() >= 1);
}

/// Test: sessions_send to non-existent agent returns error
#[tokio::test]
async fn test_send_agent_not_found() {
    let temp = tempdir().unwrap();
    let (context, _adapter) = create_permissive_context(temp.path().to_path_buf());

    // Don't register the target agent
    let tool = SessionsSendTool::with_context((*context).clone(), "main");
    let args = SessionsSendArgs {
        session_key: Some("agent:nonexistent:main".to_string()),
        message: "Hello".to_string(),
        timeout_seconds: 30,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.status, SessionsSendStatus::Error);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("not found in registry"));
}

/// Test: sessions_send wait mode with execution failure
#[tokio::test]
async fn test_send_wait_mode_execution_failure() {
    let temp = tempdir().unwrap();
    let adapter = Arc::new(TestExecutionAdapter::failing());
    let context =
        create_context_with_adapter(temp.path().to_path_buf(), AgentToAgentPolicy::permissive(), adapter.clone());

    // Register target agent
    register_test_agent(&context, "main", temp.path()).await;

    let tool = SessionsSendTool::with_context((*context).clone(), "caller");
    let args = SessionsSendArgs {
        session_key: Some("agent:main:main".to_string()),
        message: "This will fail".to_string(),
        timeout_seconds: 5, // Wait mode
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.status, SessionsSendStatus::Error);
    assert!(result.error.as_ref().unwrap().contains("Execution failed"));
}

/// Test: sessions_send defaults to main session when no key provided
#[tokio::test]
async fn test_send_default_to_main_session() {
    let temp = tempdir().unwrap();
    let adapter = Arc::new(TestExecutionAdapter::new());
    let context =
        create_context_with_adapter(temp.path().to_path_buf(), AgentToAgentPolicy::permissive(), adapter.clone());

    // Register the "main" agent (default target)
    register_test_agent(&context, "main", temp.path()).await;

    let tool = SessionsSendTool::with_context((*context).clone(), "caller");
    let args = SessionsSendArgs {
        session_key: None, // Should default to agent:main:main
        message: "Hello default".to_string(),
        timeout_seconds: 0,
    };

    let result = AlephTool::call(&tool, args).await.unwrap();
    assert_eq!(result.status, SessionsSendStatus::Accepted);
}

// ============================================================================
// Combined Workflow Tests
// ============================================================================

/// Test: Full workflow - list sessions, then send to discovered session
#[tokio::test]
async fn test_list_then_send_workflow() {
    let temp = tempdir().unwrap();
    let adapter = Arc::new(TestExecutionAdapter::new());
    let context =
        create_context_with_adapter(temp.path().to_path_buf(), AgentToAgentPolicy::permissive(), adapter.clone());

    // Create some sessions
    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("translator"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("coder"))
        .await
        .unwrap();

    // Register agents
    register_test_agent(&context, "translator", temp.path()).await;
    register_test_agent(&context, "coder", temp.path()).await;

    // Step 1: List sessions to discover available agents
    let list_tool = SessionsListTool::new(context.clone(), "main");
    let list_args = SessionsListArgs {
        kinds: Some(vec!["main".to_string()]),
        limit: Some(10),
        active_minutes: None,
        message_limit: None,
    };

    let list_result = AlephTool::call(&list_tool, list_args).await.unwrap();
    assert!(list_result.count >= 2);

    // Find the translator session
    let translator_session = list_result
        .sessions
        .iter()
        .find(|s| s.key.contains("translator"))
        .expect("Should find translator session");

    // Step 2: Send message to discovered session
    let send_tool = SessionsSendTool::with_context((*context).clone(), "main");
    let send_args = SessionsSendArgs {
        session_key: Some(translator_session.key.clone()),
        message: "Translate 'Hello' to French".to_string(),
        timeout_seconds: 0, // Fire-and-forget
    };

    let send_result = AlephTool::call(&send_tool, send_args).await.unwrap();
    assert_eq!(send_result.status, SessionsSendStatus::Accepted);

    // Give background task time to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    assert!(adapter.call_count() >= 1);
}

/// Test: Policy enforcement across list and send operations
#[tokio::test]
async fn test_policy_consistency_across_tools() {
    let temp = tempdir().unwrap();
    // Policy: only allow "work-*" agents
    let policy = AgentToAgentPolicy::new(true, vec!["work-*".to_string()]);
    let adapter = Arc::new(TestExecutionAdapter::new());
    let context = create_context_with_adapter(temp.path().to_path_buf(), policy, adapter);

    // Create sessions for different agents
    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("work-agent"))
        .await
        .unwrap();
    session_manager
        .get_or_create(&SessionKey::main("personal-agent"))
        .await
        .unwrap();

    // Register agents
    register_test_agent(&context, "work-agent", temp.path()).await;
    register_test_agent(&context, "personal-agent", temp.path()).await;

    // List: should only see work-* sessions
    let list_tool = SessionsListTool::new(context.clone(), "main");
    let list_args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let list_result = AlephTool::call(&list_tool, list_args).await.unwrap();
    assert_eq!(list_result.count, 1);
    assert!(list_result.sessions[0].key.contains("work-agent"));

    // Send to work-agent: should succeed
    let send_tool = SessionsSendTool::with_context((*context).clone(), "main");
    let send_args = SessionsSendArgs {
        session_key: Some("agent:work-agent:main".to_string()),
        message: "Hello work agent".to_string(),
        timeout_seconds: 0,
    };

    let send_result = AlephTool::call(&send_tool, send_args).await.unwrap();
    assert_eq!(send_result.status, SessionsSendStatus::Accepted);

    // Send to personal-agent: should be forbidden
    let send_args_personal = SessionsSendArgs {
        session_key: Some("agent:personal-agent:main".to_string()),
        message: "Hello personal agent".to_string(),
        timeout_seconds: 0,
    };

    let send_result_personal = AlephTool::call(&send_tool, send_args_personal).await.unwrap();
    assert_eq!(send_result_personal.status, SessionsSendStatus::Forbidden);
}

/// Test: Same-agent communication always allowed even with restrictive policy
#[tokio::test]
async fn test_same_agent_always_allowed() {
    let temp = tempdir().unwrap();
    let policy = AgentToAgentPolicy::disabled();
    let adapter = Arc::new(TestExecutionAdapter::new());
    let context = create_context_with_adapter(temp.path().to_path_buf(), policy, adapter);

    // Create session for "main" agent
    let session_manager = context.session_manager();
    session_manager
        .get_or_create(&SessionKey::main("main"))
        .await
        .unwrap();

    // Register the agent
    register_test_agent(&context, "main", temp.path()).await;

    // List: main agent should see its own sessions
    let list_tool = SessionsListTool::new(context.clone(), "main");
    let list_args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: None,
    };

    let list_result = AlephTool::call(&list_tool, list_args).await.unwrap();
    assert_eq!(list_result.count, 1);

    // Send: main agent should be able to send to itself
    let send_tool = SessionsSendTool::with_context((*context).clone(), "main");
    let send_args = SessionsSendArgs {
        session_key: Some("agent:main:main".to_string()),
        message: "Self message".to_string(),
        timeout_seconds: 0,
    };

    let send_result = AlephTool::call(&send_tool, send_args).await.unwrap();
    assert_eq!(send_result.status, SessionsSendStatus::Accepted);
}
