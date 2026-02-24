//! Tools context for BDD tests
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use tempfile::TempDir;
use tokio::sync::RwLock;

use alephcore::agents::sub_agents::{
    DelegateResult, ExecutionContextInfo, MergedResult, SubAgentRequest,
};
use alephcore::builtin_tools::meta_tools::{
    GetToolSchemaOutput, ListToolsOutput,
};
use alephcore::builtin_tools::sessions::{
    SessionsListOutput, SessionsSendOutput,
};
use alephcore::dispatcher::{
    ToolIndex, ToolIndexEntry, ToolRegistry, ToolDefinition, UnifiedTool,
};
use alephcore::gateway::a2a_policy::AgentToAgentPolicy;
use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use alephcore::gateway::context::GatewayContext as AlephGatewayContext;
use alephcore::gateway::event_emitter::{EventEmitter, EventEmitError, StreamEvent};
use alephcore::gateway::execution_adapter::ExecutionAdapter;
use alephcore::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};
use alephcore::gateway::router::SessionKey;
use alephcore::tools::{AlephToolServer, AlephToolServerHandle, ToolUpdateInfo};

/// Context for tool server tests
#[derive(Default)]
pub struct ToolsContext {
    /// Tool server instance
    pub server: Option<AlephToolServer>,
    /// Tool server handle for handle-based operations
    pub handle: Option<AlephToolServerHandle>,
    /// Tool definition captured from a tool
    pub tool_definition: Option<ToolDefinition>,
    /// LLM context string from a tool definition
    pub llm_context: Option<String>,
    /// Update info from replace_tool operations
    pub update_info: Option<ToolUpdateInfo>,
    /// Result from calling a tool
    pub call_result: Option<serde_json::Value>,
    /// Replacement counter for tracking multiple replacements
    pub replacement_count: usize,

    // Smart Tool Discovery fields
    /// Current unified tool being tested
    pub unified_tool: Option<UnifiedTool>,
    /// Current tool index entry
    pub index_entry: Option<ToolIndexEntry>,
    /// Current tool index
    pub tool_index: Option<ToolIndex>,
    /// Tool registry for meta tools tests
    pub tool_registry: Option<Arc<RwLock<ToolRegistry>>>,
    /// List tools result
    pub list_result: Option<ListToolsOutput>,
    /// Get tool schema result
    pub schema_result: Option<GetToolSchemaOutput>,
    /// Generated prompt
    pub prompt: Option<String>,
    /// Full schema text size (chars)
    pub full_schema_size: Option<usize>,
    /// Index text size (chars)
    pub index_size: Option<usize>,
    /// Latency measurements (microseconds)
    pub latencies: LatencyMeasurements,

    // Sub-agent fields
    /// Delegate result JSON for parsing tests
    pub delegate_json: Option<JsonValue>,
    /// Parsed delegate result
    pub delegate_result: Option<DelegateResult>,
    /// Merged result from delegate
    pub merged_result: Option<MergedResult>,
    /// Execution context info
    pub execution_context: Option<ExecutionContextInfo>,
    /// Sub-agent request
    pub sub_agent_request: Option<SubAgentRequest>,

    // Sessions tools fields
    /// Gateway context for sessions tools
    pub gateway_context: Option<Arc<AlephGatewayContext>>,
    /// Tracking execution adapter
    pub tracking_adapter: Option<Arc<TestExecutionAdapter>>,
    /// Session manager
    pub session_manager: Option<Arc<SessionManager>>,
    /// Agent registry
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Current session key for multi-step tests
    pub current_session_key: Option<SessionKey>,
    /// Found session key (from list search)
    pub found_session_key: Option<String>,
    /// Sessions list result
    pub sessions_list_result: Option<SessionsListOutput>,
    /// Sessions send result
    pub sessions_send_result: Option<SessionsSendOutput>,
    /// Temp directory
    pub temp_dir: Option<TempDir>,
    /// Current caller agent ID
    pub caller_agent_id: Option<String>,
}

/// Latency measurements
#[derive(Debug, Default)]
pub struct LatencyMeasurements {
    pub list_tools_us: Option<u128>,
    pub get_tool_schema_us: Option<u128>,
    pub generate_index_us: Option<u128>,
}


impl std::fmt::Debug for ToolsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolsContext")
            .field("server", &self.server.as_ref().map(|_| "AlephToolServer"))
            .field("handle", &self.handle.as_ref().map(|_| "AlephToolServerHandle"))
            .field("tool_definition", &self.tool_definition)
            .field("llm_context", &self.llm_context)
            .field("update_info", &self.update_info)
            .field("call_result", &self.call_result)
            .field("replacement_count", &self.replacement_count)
            .field("unified_tool", &self.unified_tool.as_ref().map(|t| &t.name))
            .field("index_entry", &self.index_entry)
            .field("tool_index", &self.tool_index.as_ref().map(|i| i.total_count()))
            .field("tool_registry", &self.tool_registry.as_ref().map(|_| "ToolRegistry"))
            .field("list_result", &self.list_result.as_ref().map(|_| "ListToolsOutput"))
            .field("schema_result", &self.schema_result.as_ref().map(|r| &r.name))
            .field("prompt", &self.prompt.as_ref().map(|p| p.len()))
            .field("full_schema_size", &self.full_schema_size)
            .field("index_size", &self.index_size)
            .field("latencies", &self.latencies)
            .field("delegate_result", &self.delegate_result.as_ref().map(|d| d.success))
            .field("merged_result", &self.merged_result.as_ref().map(|m| m.success))
            .field("gateway_context", &self.gateway_context.as_ref().map(|_| "GatewayContext"))
            .field("tracking_adapter", &self.tracking_adapter.as_ref().map(|_| "TestExecutionAdapter"))
            .field("sessions_list_result", &self.sessions_list_result.as_ref().map(|r| r.count))
            .field("sessions_send_result", &self.sessions_send_result.as_ref().map(|r| &r.status))
            .finish()
    }
}

// =============================================================================
// Test Execution Adapter for Sessions Tests
// =============================================================================

/// Mock execution adapter that tracks invocations and can simulate various behaviors
pub struct TestExecutionAdapter {
    /// Number of times execute was called
    execute_count: AtomicUsize,
    /// Whether to fail execution
    should_fail: AtomicBool,
    /// Response to return (for simulating replies)
    pub mock_response: Option<String>,
}

impl TestExecutionAdapter {
    pub fn new() -> Self {
        Self {
            execute_count: AtomicUsize::new(0),
            should_fail: AtomicBool::new(false),
            mock_response: None,
        }
    }

    pub fn failing() -> Self {
        let adapter = Self::new();
        adapter.should_fail.store(true, Ordering::SeqCst);
        adapter
    }

    pub fn call_count(&self) -> usize {
        self.execute_count.load(Ordering::SeqCst)
    }
}

impl Default for TestExecutionAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TestExecutionAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestExecutionAdapter")
            .field("execute_count", &self.execute_count.load(Ordering::SeqCst))
            .field("should_fail", &self.should_fail.load(Ordering::SeqCst))
            .finish()
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
            tokio::time::sleep(Duration::from_millis(10)).await;
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

// =============================================================================
// Helper Functions
// =============================================================================

impl ToolsContext {
    /// Initialize a permissive gateway context
    pub fn init_permissive_gateway(&mut self) {
        self.init_gateway_with_policy(AgentToAgentPolicy::permissive(), false);
    }

    /// Initialize gateway with tracking adapter
    pub fn init_permissive_gateway_with_tracking(&mut self) {
        self.init_gateway_with_policy(AgentToAgentPolicy::permissive(), true);
    }

    /// Initialize gateway with failing adapter
    pub fn init_permissive_gateway_with_failing(&mut self) {
        let temp = tempfile::tempdir().unwrap();
        let session_config = SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        };
        let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
        let agent_registry = Arc::new(AgentRegistry::new());
        let adapter = Arc::new(TestExecutionAdapter::failing());
        let a2a_policy = Arc::new(AgentToAgentPolicy::permissive());

        self.session_manager = Some(session_manager.clone());
        self.agent_registry = Some(agent_registry.clone());
        self.tracking_adapter = Some(adapter.clone());
        self.gateway_context = Some(Arc::new(AlephGatewayContext::new(
            session_manager,
            agent_registry,
            adapter,
            a2a_policy,
        )));
        self.temp_dir = Some(temp);
    }

    /// Initialize gateway with custom policy
    pub fn init_gateway_with_policy(&mut self, policy: AgentToAgentPolicy, with_tracking: bool) {
        let temp = tempfile::tempdir().unwrap();
        let session_config = SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        };
        let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
        let agent_registry = Arc::new(AgentRegistry::new());
        let adapter = Arc::new(TestExecutionAdapter::new());
        let a2a_policy = Arc::new(policy);

        self.session_manager = Some(session_manager.clone());
        self.agent_registry = Some(agent_registry.clone());
        if with_tracking {
            self.tracking_adapter = Some(adapter.clone());
        }
        self.gateway_context = Some(Arc::new(AlephGatewayContext::new(
            session_manager,
            agent_registry,
            adapter,
            a2a_policy,
        )));
        self.temp_dir = Some(temp);
    }

    /// Register a test agent
    pub async fn register_agent(&self, agent_id: &str) {
        if let (Some(ctx), Some(temp)) = (&self.gateway_context, &self.temp_dir) {
            let config = AgentInstanceConfig {
                agent_id: agent_id.to_string(),
                workspace: temp.path().join(format!("{}_workspace", agent_id)),
                ..Default::default()
            };
            let agent = AgentInstance::new(config).unwrap();
            ctx.agent_registry().register(agent).await;
        }
    }
}
