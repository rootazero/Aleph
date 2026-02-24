//! Gateway Context for BDD tests
//!
//! Provides shared state for testing InboundMessageRouter and related Gateway components.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tempfile::TempDir;

use alephcore::gateway::{
    AgentInstance, AgentRegistry, AgentRouter, ChannelId, ChannelRegistry, ConversationId, ExecutionAdapter, InboundContext, InboundMessage, InboundMessageRouter,
    MessageId, ReplyRoute, RoutingConfig, RunRequest, RunStatus,
    SqlitePairingStore, UserId,
};
use alephcore::gateway::router::SessionKey;
use alephcore::gateway::execution_engine::{ExecutionError, RunState};
use alephcore::gateway::event_emitter::{EventEmitter, EventEmitError, StreamEvent};

/// Gateway test context
#[derive(Default)]
pub struct GatewayContext {
    /// Inbound message router under test
    pub router: Option<InboundMessageRouter>,
    /// Routing configuration
    pub config: Option<RoutingConfig>,
    /// Channel registry
    pub channel_registry: Option<Arc<ChannelRegistry>>,
    /// Pairing store
    pub pairing_store: Option<Arc<SqlitePairingStore>>,
    /// Agent registry (for execution tests)
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Agent router (for unified routing tests)
    pub agent_router: Option<Arc<AgentRouter>>,
    /// Execution adapter (for execution tests)
    pub execution_adapter: Option<Arc<dyn ExecutionAdapter>>,
    /// Tracking adapter (to verify execute was called)
    pub tracking_adapter: Option<Arc<TrackingExecutionAdapter>>,

    // Test message state
    /// Current test message
    pub test_message: Option<InboundMessage>,
    /// Current test context
    pub test_context: Option<InboundContext>,

    // Results
    /// Resolved session key
    pub session_key: Option<String>,
    /// Allowlist check result
    pub is_allowed: Option<bool>,
    /// Mention detection result
    pub mention_detected: Option<bool>,
    /// Execution result
    pub execution_result: Option<Result<(), String>>,
    /// Resolved agent ID (for unified routing)
    pub resolved_agent_id: Option<String>,
    /// Current allowlist for testing
    pub allowlist: Option<Vec<String>>,

    // Temp resources
    pub temp_dir: Option<TempDir>,

    // iMessage routing test state
    /// Last handle_message result
    pub handle_message_result: Option<Result<(), String>>,
    /// Whether the message was filtered (not an error, just filtered)
    pub message_filtered: Option<bool>,
}

impl std::fmt::Debug for GatewayContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayContext")
            .field("router", &self.router.as_ref().map(|_| "InboundMessageRouter"))
            .field("config", &self.config)
            .field("channel_registry", &self.channel_registry.as_ref().map(|_| "ChannelRegistry"))
            .field("pairing_store", &self.pairing_store.as_ref().map(|_| "SqlitePairingStore"))
            .field("agent_registry", &self.agent_registry.as_ref().map(|_| "AgentRegistry"))
            .field("agent_router", &self.agent_router.as_ref().map(|_| "AgentRouter"))
            .field("execution_adapter", &self.execution_adapter.as_ref().map(|_| "dyn ExecutionAdapter"))
            .field("tracking_adapter", &self.tracking_adapter)
            .field("test_message", &self.test_message)
            .field("test_context", &self.test_context.as_ref().map(|_| "InboundContext"))
            .field("session_key", &self.session_key)
            .field("is_allowed", &self.is_allowed)
            .field("mention_detected", &self.mention_detected)
            .field("execution_result", &self.execution_result)
            .field("resolved_agent_id", &self.resolved_agent_id)
            .field("allowlist", &self.allowlist)
            .field("temp_dir", &self.temp_dir.as_ref().map(|_| "TempDir"))
            .field("handle_message_result", &self.handle_message_result)
            .field("message_filtered", &self.message_filtered)
            .finish()
    }
}

impl GatewayContext {
    /// Initialize basic router without execution support
    pub fn init_basic_router(&mut self) {
        self.init_basic_router_with_config(RoutingConfig::default());
    }

    /// Initialize basic router with custom config
    pub fn init_basic_router_with_config(&mut self, config: RoutingConfig) {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());

        self.channel_registry = Some(registry.clone());
        self.pairing_store = Some(store.clone());
        self.config = Some(config.clone());
        self.router = Some(InboundMessageRouter::new(registry, store, config));
    }

    /// Initialize router with execution support
    pub fn init_router_with_execution(&mut self) {
        let channel_registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();
        let agent_registry = Arc::new(AgentRegistry::new());
        let tracking_adapter = Arc::new(TrackingExecutionAdapter::new());
        let adapter: Arc<dyn ExecutionAdapter> = tracking_adapter.clone();

        self.channel_registry = Some(channel_registry.clone());
        self.pairing_store = Some(store.clone());
        self.config = Some(config.clone());
        self.agent_registry = Some(agent_registry.clone());
        self.tracking_adapter = Some(tracking_adapter);
        self.execution_adapter = Some(adapter.clone());

        self.router = Some(InboundMessageRouter::with_execution(
            channel_registry,
            store,
            config,
            agent_registry,
            adapter,
        ));
    }

    /// Initialize router with unified routing
    pub fn init_router_with_unified_routing(&mut self) {
        let channel_registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();
        let agent_registry = Arc::new(AgentRegistry::new());
        let tracking_adapter = Arc::new(TrackingExecutionAdapter::new());
        let adapter: Arc<dyn ExecutionAdapter> = tracking_adapter.clone();
        let agent_router = Arc::new(AgentRouter::new());

        self.channel_registry = Some(channel_registry.clone());
        self.pairing_store = Some(store.clone());
        self.config = Some(config.clone());
        self.agent_registry = Some(agent_registry.clone());
        self.tracking_adapter = Some(tracking_adapter);
        self.execution_adapter = Some(adapter.clone());
        self.agent_router = Some(agent_router.clone());

        self.router = Some(InboundMessageRouter::with_unified_routing(
            channel_registry,
            store,
            config,
            agent_registry,
            adapter,
            agent_router,
        ));
    }

    /// Create a test message
    pub fn create_test_message(&mut self, is_group: bool) {
        self.test_message = Some(make_test_message(is_group));
    }

    /// Create a test message with custom conversation ID
    pub fn create_test_message_with_conv(&mut self, is_group: bool, conv_id: &str) {
        let mut msg = make_test_message(is_group);
        msg.conversation_id = ConversationId::new(conv_id);
        self.test_message = Some(msg);
    }

    /// Create test context from current message
    pub fn create_test_context(&mut self) {
        if let Some(msg) = &self.test_message {
            let reply_route = ReplyRoute::new(
                msg.channel_id.clone(),
                msg.conversation_id.clone(),
            );
            let session_key = SessionKey::main("main");
            self.test_context = Some(InboundContext::new(msg.clone(), reply_route, session_key).authorize());
        }
    }

    /// Get the router (panics if not initialized)
    pub fn router(&self) -> &InboundMessageRouter {
        self.router.as_ref().expect("Router not initialized")
    }
}

/// Create a test inbound message
pub fn make_test_message(is_group: bool) -> InboundMessage {
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

/// Tracking execution adapter that records whether execute() was called
pub struct TrackingExecutionAdapter {
    execute_called: AtomicBool,
    execute_count: AtomicUsize,
    should_fail: bool,
}

impl std::fmt::Debug for TrackingExecutionAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackingExecutionAdapter")
            .field("execute_called", &self.execute_called.load(Ordering::SeqCst))
            .field("execute_count", &self.execute_count.load(Ordering::SeqCst))
            .field("should_fail", &self.should_fail)
            .finish()
    }
}

impl TrackingExecutionAdapter {
    pub fn new() -> Self {
        Self {
            execute_called: AtomicBool::new(false),
            execute_count: AtomicUsize::new(0),
            should_fail: false,
        }
    }

    #[allow(dead_code)]
    pub fn failing() -> Self {
        Self {
            execute_called: AtomicBool::new(false),
            execute_count: AtomicUsize::new(0),
            should_fail: true,
        }
    }

    pub fn was_called(&self) -> bool {
        self.execute_called.load(Ordering::SeqCst)
    }

    pub fn call_count(&self) -> usize {
        self.execute_count.load(Ordering::SeqCst)
    }
}

impl Default for TrackingExecutionAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExecutionAdapter for TrackingExecutionAdapter {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
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

/// Simple test emitter for execution tests
pub struct TestEmitter;

impl std::fmt::Debug for TestEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestEmitter").finish()
    }
}

#[async_trait]
impl EventEmitter for TestEmitter {
    async fn emit(&self, _event: StreamEvent) -> Result<(), EventEmitError> {
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        0
    }
}
