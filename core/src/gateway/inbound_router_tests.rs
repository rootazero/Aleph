//! Tests for InboundMessageRouter

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use chrono::Utc;
use async_trait::async_trait;
use tempfile::tempdir;
use crate::gateway::agent_instance::AgentInstanceConfig;
use crate::gateway::channel::{ChannelId, ConversationId, MessageId, UserId};
use crate::gateway::event_emitter::{EventEmitter, EventEmitError, StreamEvent};
use crate::gateway::execution_engine::{ExecutionError, RunStatus, RunState, SimpleExecutionEngine, ExecutionEngineConfig};
use crate::gateway::pairing_store::SqlitePairingStore;

fn make_test_message(is_group: bool) -> InboundMessage {
    InboundMessage {
        id: MessageId::new("msg-1"),
        channel_id: ChannelId::new("imessage"),
        conversation_id: ConversationId::new(if is_group { "chat_id:42" } else { "+15551234567" }),
        sender_id: UserId::new("+15551234567"),
        sender_name: None,
        text: "Hello".to_string(),
        attachments: vec![],
        timestamp: Utc::now(),
        reply_to: None,
        is_group,
        raw: None,
    }
}

// =========================================================================
// Mock ExecutionAdapter for testing execution integration
// =========================================================================

/// Tracks whether execute() was called
struct TrackingExecutionAdapter {
    execute_called: AtomicBool,
    execute_count: AtomicUsize,
    should_fail: bool,
}

impl TrackingExecutionAdapter {
    fn new() -> Self {
        Self {
            execute_called: AtomicBool::new(false),
            execute_count: AtomicUsize::new(0),
            should_fail: false,
        }
    }

    #[allow(dead_code)]
    fn failing() -> Self {
        Self {
            execute_called: AtomicBool::new(false),
            execute_count: AtomicUsize::new(0),
            should_fail: true,
        }
    }

    fn was_called(&self) -> bool {
        self.execute_called.load(Ordering::SeqCst)
    }

    fn call_count(&self) -> usize {
        self.execute_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ExecutionAdapter for TrackingExecutionAdapter {
    async fn execute(
        &self,
        _request: super::super::execution_engine::RunRequest,
        _agent: Arc<super::super::agent_instance::AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        self.execute_called.store(true, Ordering::SeqCst);
        self.execute_count.fetch_add(1, Ordering::SeqCst);
        if self.should_fail {
            Err(ExecutionError::Failed("Mock failure".to_string()))
        } else {
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
            current_tool: None,
        })
    }
}

// =========================================================================
// Helper to build InboundContext for tests
// =========================================================================

fn make_test_context(msg: &InboundMessage) -> InboundContext {
    let reply_route = super::super::inbound_context::ReplyRoute::new(
        msg.channel_id.clone(),
        msg.conversation_id.clone(),
    );
    let session_key = SessionKey::main("main");
    InboundContext::new(msg.clone(), reply_route, session_key).authorize()
}

#[test]
fn test_resolve_session_key_dm_per_peer() {
    let registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default().with_dm_scope(DmScope::PerPeer);

    let router = InboundMessageRouter::new(registry, store, config);

    let msg = make_test_message(false);
    let key = router.resolve_session_key(&msg);

    assert_eq!(key.to_key_string(), "agent:main:peer:dm:+15551234567");
}

#[test]
fn test_resolve_session_key_dm_main() {
    let registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default().with_dm_scope(DmScope::Main);

    let router = InboundMessageRouter::new(registry, store, config);

    let msg = make_test_message(false);
    let key = router.resolve_session_key(&msg);

    assert_eq!(key.to_key_string(), "agent:main:main");
}

#[test]
fn test_resolve_session_key_group() {
    let registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let router = InboundMessageRouter::new(registry, store, config);

    let msg = make_test_message(true);
    let key = router.resolve_session_key(&msg);

    assert_eq!(key.to_key_string(), "agent:main:peer:imessage:group:chat_id:42");
}

#[test]
fn test_is_in_allowlist() {
    let registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let router = InboundMessageRouter::new(registry, store, config);

    let allowlist = vec!["+15551234567".to_string(), "user@example.com".to_string()];

    assert!(router.is_in_allowlist("+15551234567", &allowlist));
    assert!(router.is_in_allowlist("5551234567", &allowlist)); // Normalized
    assert!(router.is_in_allowlist("user@example.com", &allowlist));
    assert!(!router.is_in_allowlist("+19999999999", &allowlist));
}

#[test]
fn test_is_in_allowlist_wildcard() {
    let registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let router = InboundMessageRouter::new(registry, store, config);

    let allowlist = vec!["*".to_string()];
    assert!(router.is_in_allowlist("+19999999999", &allowlist));
}

#[test]
fn test_check_mention() {
    let registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let router = InboundMessageRouter::new(registry, store, config);

    let channel_config = ChannelConfig {
        bot_name: Some("MyBot".to_string()),
        ..Default::default()
    };

    assert!(router.check_mention("Hey @aether, help me", &channel_config));
    assert!(router.check_mention("MyBot can you help?", &channel_config));
    assert!(router.check_mention("Hello AETHER", &channel_config));
    assert!(!router.check_mention("Hello world", &channel_config));
}

// =========================================================================
// Execution Integration Tests
// =========================================================================

/// Test: execute_for_context with no execution support configured (graceful degradation)
///
/// When router is created with `new()` (no execution support), execute_for_context
/// should log what would happen and return Ok(()) rather than failing.
#[tokio::test]
async fn test_execute_for_context_no_execution_support() {
    let registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    // Create router WITHOUT execution support
    let router = InboundMessageRouter::new(registry, store, config);

    let msg = make_test_message(false);
    let ctx = make_test_context(&msg);

    // Should return Ok(()) with graceful degradation (just logs)
    let result = router.execute_for_context(&ctx).await;
    assert!(result.is_ok(), "Should gracefully degrade when execution not configured");
}

/// Test: execute_for_context with agent not found
///
/// When router has execution support but the agent registry is empty,
/// execute_for_context should return AgentNotFound error.
#[tokio::test]
async fn test_execute_for_context_agent_not_found() {
    let channel_registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    // Create EMPTY agent registry (no agents registered)
    let agent_registry = Arc::new(AgentRegistry::new());

    // Create mock execution adapter
    let adapter: Arc<dyn ExecutionAdapter> = Arc::new(TrackingExecutionAdapter::new());

    // Create router WITH execution support but empty registry
    let router = InboundMessageRouter::with_execution(
        channel_registry,
        store,
        config,
        agent_registry,
        adapter,
    );

    let msg = make_test_message(false);
    let ctx = make_test_context(&msg);

    // Should return AgentNotFound error
    let result = router.execute_for_context(&ctx).await;
    assert!(result.is_err(), "Should fail when agent not found");

    match result {
        Err(RoutingError::AgentNotFound(agent_id)) => {
            assert_eq!(agent_id, "main", "Agent ID should be 'main'");
        }
        other => panic!("Expected AgentNotFound error, got: {:?}", other),
    }
}

/// Test: execute_for_context calls ExecutionAdapter
///
/// When router has execution support and agent is registered,
/// execute_for_context should call the execution adapter's execute() method.
#[tokio::test]
async fn test_execute_for_context_calls_adapter() {
    let channel_registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    // Create agent registry and register an agent
    let agent_registry = Arc::new(AgentRegistry::new());
    let temp = tempdir().unwrap();
    let agent_config = AgentInstanceConfig {
        agent_id: "main".to_string(),
        workspace: temp.path().join("workspace"),
        ..Default::default()
    };
    let agent = super::super::agent_instance::AgentInstance::new(agent_config).unwrap();
    agent_registry.register(agent).await;

    // Create tracking adapter to verify execute() is called
    let tracking_adapter = Arc::new(TrackingExecutionAdapter::new());
    let adapter: Arc<dyn ExecutionAdapter> = tracking_adapter.clone();

    // Create router with execution support
    let router = InboundMessageRouter::with_execution(
        channel_registry,
        store,
        config,
        agent_registry,
        adapter,
    );

    let msg = make_test_message(false);
    let ctx = make_test_context(&msg);

    // Execute
    let result = router.execute_for_context(&ctx).await;
    assert!(result.is_ok(), "Execute should succeed: {:?}", result);

    // Give tokio::spawn a moment to run (execution is spawned)
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify adapter's execute() was called
    assert!(
        tracking_adapter.was_called(),
        "ExecutionAdapter.execute() should have been called"
    );
    assert_eq!(
        tracking_adapter.call_count(),
        1,
        "ExecutionAdapter.execute() should have been called exactly once"
    );
}

/// Test: SimpleExecutionEngine as Arc<dyn ExecutionAdapter>
///
/// Verify that SimpleExecutionEngine can be used as a trait object
/// and its methods can be called through the trait interface.
#[tokio::test]
async fn test_simple_execution_engine_as_trait_object() {
    // Create SimpleExecutionEngine and wrap as trait object
    let engine = SimpleExecutionEngine::new(ExecutionEngineConfig::default());
    let adapter: Arc<dyn ExecutionAdapter> = Arc::new(engine);

    // Test get_status through trait object
    let status = adapter.get_status("nonexistent-run").await;
    assert!(status.is_none(), "Should return None for nonexistent run");

    // Test cancel through trait object
    let cancel_result = adapter.cancel("nonexistent-run").await;
    assert!(
        matches!(cancel_result, Err(ExecutionError::RunNotFound(_))),
        "Should return RunNotFound for nonexistent run"
    );

    // Test execute through trait object requires agent setup
    let temp = tempdir().unwrap();
    let agent_config = AgentInstanceConfig {
        agent_id: "test".to_string(),
        workspace: temp.path().join("workspace"),
        ..Default::default()
    };
    let agent = Arc::new(
        super::super::agent_instance::AgentInstance::new(agent_config).unwrap()
    );

    // Create a simple no-op emitter
    struct TestEmitter;
    #[async_trait]
    impl EventEmitter for TestEmitter {
        async fn emit(&self, _event: StreamEvent) -> Result<(), EventEmitError> {
            Ok(())
        }
        fn next_seq(&self) -> u64 {
            0
        }
    }

    let emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(TestEmitter);
    let request = super::super::execution_engine::RunRequest {
        run_id: "test-run".to_string(),
        input: "Hello".to_string(),
        session_key: SessionKey::main("test"),
        timeout_secs: Some(5),
        metadata: HashMap::new(),
    };

    // Execute through trait object
    let result = adapter.execute(request, agent, emitter).await;
    assert!(result.is_ok(), "Execute through trait object should succeed");
}

/// Test: Router created with with_execution has both registries set
#[tokio::test]
async fn test_router_with_execution_has_registries() {
    let channel_registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let agent_registry = Arc::new(AgentRegistry::new());
    let adapter: Arc<dyn ExecutionAdapter> = Arc::new(TrackingExecutionAdapter::new());

    let router = InboundMessageRouter::with_execution(
        channel_registry,
        store,
        config,
        agent_registry,
        adapter,
    );

    // Verify execution support is configured by checking behavior
    // (internal fields are private, so we test behavior)
    let msg = make_test_message(false);
    let ctx = make_test_context(&msg);

    // Should return AgentNotFound (not graceful degradation) because execution IS configured
    let result = router.execute_for_context(&ctx).await;
    assert!(
        matches!(result, Err(RoutingError::AgentNotFound(_))),
        "Should return AgentNotFound when execution configured but agent missing"
    );
}

/// Test: Router backward compatibility with new()
#[test]
fn test_router_backward_compatible_new() {
    let channel_registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    // This should compile and work exactly as before
    let router = InboundMessageRouter::new(channel_registry, store, config);

    // Can still register channel configs
    let mut router = router;
    router.register_channel_config("test", ChannelConfig::default());

    // Basic routing operations should still work
    let msg = make_test_message(false);
    let key = router.resolve_session_key(&msg);
    assert!(key.to_key_string().contains("main"));
}

// =========================================================================
// Unified Routing Tests (AgentRouter integration)
// =========================================================================

/// Test: Unified routing respects AgentRouter bindings
///
/// When a channel pattern is bound to a specific agent in AgentRouter,
/// inbound messages from that channel should route to that agent.
#[tokio::test]
async fn test_unified_routing_respects_bindings() {
    use super::super::router::AgentRouter;

    let channel_registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    // Create AgentRouter with a binding: imessage:* -> work
    let agent_router = Arc::new(AgentRouter::new());
    agent_router.register_agent("work").await;
    agent_router.add_binding("imessage:*", "work").await;

    // Create router with unified routing
    let router = InboundMessageRouter::new(channel_registry, store, config)
        .with_agent_router(agent_router);

    // Resolve agent for imessage channel
    let agent_id = router.resolve_agent_id_async("imessage").await;
    assert_eq!(agent_id, "work", "Should route imessage to 'work' agent");

    // Resolve agent for other channel (should use default)
    let agent_id = router.resolve_agent_id_async("telegram").await;
    assert_eq!(agent_id, "main", "Should route telegram to default 'main' agent");
}

/// Test: Unified routing falls back to default when no router
#[tokio::test]
async fn test_unified_routing_fallback_no_router() {
    let channel_registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::new("assistant");

    // Create router WITHOUT AgentRouter
    let router = InboundMessageRouter::new(channel_registry, store, config);

    // Should fall back to config.default_agent
    let agent_id = router.resolve_agent_id_async("imessage").await;
    assert_eq!(agent_id, "assistant", "Should use config default_agent when no router");
}

/// Test: with_unified_routing constructor sets all fields
#[tokio::test]
async fn test_with_unified_routing_constructor() {
    use super::super::router::AgentRouter;

    let channel_registry = Arc::new(ChannelRegistry::new());
    let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();
    let agent_registry = Arc::new(AgentRegistry::new());
    let adapter: Arc<dyn ExecutionAdapter> = Arc::new(TrackingExecutionAdapter::new());
    let agent_router = Arc::new(AgentRouter::new());

    let router = InboundMessageRouter::with_unified_routing(
        channel_registry,
        store,
        config,
        agent_registry,
        adapter,
        agent_router.clone(),
    );

    // Verify routing works through the agent_router
    agent_router.add_binding("test:*", "custom").await;
    let agent_id = router.resolve_agent_id_async("test:channel").await;
    assert_eq!(agent_id, "custom", "Should use AgentRouter for resolution");
}
