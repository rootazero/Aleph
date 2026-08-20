//! Shared minimal fixtures for the `gateway_chat_*` integration tests.
//!
//! Builds an Orchestrator wired to a stub `HarnessRunner` so each test can
//! script the dispatch outcome without dragging the full Gateway boot.

#![allow(dead_code)] // shared across multiple integration test binaries

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use alephcore::agents::AgentRegistry;
use alephcore::orchestrator::FlowError;
use alephcore::orchestrator::{
    build_sandbox_factory, AgentHarnessRunner, BrainRef, FlowInput, FlowOutcome, FlowOverrides,
    FlowRegistry, FlowRequest, FlowSet, FlowSpec, FlowStreamEvent, HarnessRunner, Orchestrator,
    RoutingOverrides, SessionStrategy,
};
use alephcore::providers::adapter::{ProviderResponse, RequestPayload};
use alephcore::providers::AiProvider;
use alephcore::sandbox::Sandbox;
use alephcore::session::events::ToolOutput;
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::SessionService;
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService};

// -- Session service --------------------------------------------------------

/// Fresh `SessionService` on an in-memory SQLite backing store.
pub fn fresh_session_service() -> Arc<dyn SessionService> {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    migrate_add_session_events(&conn).expect("migrate session_events");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

// -- Tool service stub ------------------------------------------------------

pub struct NoopToolService;

#[async_trait]
impl ToolService for NoopToolService {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: serde_json::json!({}),
            metadata: Default::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }

    fn metadata_schema(&self) -> Arc<[alephcore::tool_metadata::ToolDefinition]> {
        Arc::from(Vec::<alephcore::tool_metadata::ToolDefinition>::new())
    }
}

// -- Dummy provider (not exercised but required by AgentHarnessRunner) ------

pub struct NeverProvider;

impl AiProvider for NeverProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = alephcore::Result<ProviderResponse>> + Send + 'a>,
    > {
        Box::pin(async move { Ok(ProviderResponse::text_only("unused".to_string())) })
    }

    fn name(&self) -> &str {
        "never-provider"
    }

    fn color(&self) -> &str {
        "#000"
    }
}

// -- Stub HarnessRunner -----------------------------------------------------

pub type StubRunFn = Arc<
    dyn Fn(
            StubContext,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<FlowOutcome, FlowError>> + Send>,
        > + Send
        + Sync,
>;

/// Context handed to a `StubHarnessRunner` callback.
pub struct StubContext {
    pub session_key: String,
    pub input: FlowInput,
    pub events: broadcast::Sender<FlowStreamEvent>,
    pub cancel: CancellationToken,
    pub tool_service_override: Option<Arc<dyn ToolService>>,
    pub trace_sink: Option<Arc<dyn alephcore::harness::TraceSink>>,
}

/// `HarnessRunner` impl that defers every `run` call to a user-supplied closure.
pub struct StubHarnessRunner {
    run_fn: StubRunFn,
}

impl StubHarnessRunner {
    pub fn new(run_fn: StubRunFn) -> Arc<Self> {
        Arc::new(Self { run_fn })
    }
}

#[async_trait]
impl HarnessRunner for StubHarnessRunner {
    async fn run(
        &self,
        session_key: String,
        _spec: Arc<FlowSpec>,
        input: FlowInput,
        _sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
        tool_service_override: Option<Arc<dyn ToolService>>,
        trace_sink: Option<Arc<dyn alephcore::harness::TraceSink>>,
        _interaction_manifest: Option<alephcore::thinker::InteractionManifest>,
        _workspace_override: Option<std::path::PathBuf>,
        _max_iterations_override: Option<u32>,
        _transient_context: Option<String>,
        _think_level: Option<alephcore::agents::thinking::ThinkLevel>,
        _envelope: alephcore::thinker::TurnEnvelope,
        _turn_model: Option<alephcore::providers::session_model_handle::SessionModelPref>,
    ) -> Result<FlowOutcome, FlowError> {
        let ctx = StubContext {
            session_key,
            input,
            events,
            cancel,
            tool_service_override,
            trace_sink,
        };
        (self.run_fn)(ctx).await
    }
}

// -- Orchestrator factory ---------------------------------------------------

pub fn orchestrator_with_stub(runner: Arc<StubHarnessRunner>) -> Arc<Orchestrator> {
    let mut specs = FlowSet::new();
    specs.insert(
        "gateway-chat".into(),
        Arc::new(FlowSpec {
            id: "gateway-chat".into(),
            description: "test flow".into(),
            agent: "main".into(),
            brain: BrainRef::Default,
            session_strategy: SessionStrategy::Fresh,
            overrides: FlowOverrides::default(),
        }),
    );
    let flow_registry = Arc::new(FlowRegistry::new(specs));
    let mut defaults: HashMap<String, String> = HashMap::new();
    defaults.insert("main".into(), "gateway-chat".into());

    let session_service = fresh_session_service();
    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(alephcore::sandbox::NoopSandbox) as Arc<dyn Sandbox>)
    }));

    // The Orchestrator requires an `AgentHarnessRunner` field for legacy
    // composition, but we only use the stub runner via `HarnessRunner` dyn
    // dispatch. `agent_registry` here must list the "main" agent so the
    // pre-dispatch lookup succeeds — but since the stub runner bypasses
    // `harness_bridge` entirely, we can hand Orchestrator::new any
    // `Arc<dyn HarnessRunner>` and it'll dispatch through that.
    let _ = AgentHarnessRunner {
        agent_registry: Arc::new(AgentRegistry::with_builtins()),
        session_service: session_service.clone(),
        tool_service: Arc::new(NoopToolService) as Arc<dyn ToolService>,
        default_provider: Arc::new(alephcore::providers::StaticDefault::new(
            Arc::new(NeverProvider) as Arc<dyn AiProvider>,
        )) as Arc<dyn alephcore::providers::DefaultProviderHandle>,
        named_providers: HashMap::new(),
        verifier_chain: None,
        context_budget_config: None,
        context_budget_refiner: None,
        skill_system: None,
        guardrails: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        default_max_iterations: 200,
        power: None,
        memory_context_provider: None,
        memory_backend: None,
        memory_project_scoped: false,
        tool_catalog: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        cheap_provider: None,
        default_prompt_mode: Default::default(),
        prompt_extra_files: None,
        mcp_handle: None,
        parallel_tool_concurrency: None,
        primary_context_window: None,
        routing_store: None,
        routing_recall: None,
        estimate_overhead_cache: std::sync::Arc::new(
            alephcore::orchestrator::harness_bridge::context_estimate::OverheadCache::default(),
        ),
        response_language: None,
    };

    Arc::new(Orchestrator::new(
        flow_registry,
        Arc::new(RoutingOverrides::default()),
        Arc::new(defaults),
        session_service,
        sandbox_factory,
        runner,
    ))
}

// -- FlowRequest builder ----------------------------------------------------

pub fn basic_request() -> FlowRequest {
    FlowRequest {
        flow_id: None,
        agent_id: "main".into(),
        input: FlowInput::Prompt("hello".into()),
        channel: Some("test".into()),
        session_hint: Some("test-session".into()),
        owner_user_id: None,
        scope_id: None,
        parent_session: None,
        depth: 0,
        tool_service: None,
        trace_sink: None,
        interaction_manifest: None,
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        transient_context: None,
        think_level: None,
        envelope: alephcore::thinker::TurnEnvelope::none(),
        model_directive: None,
    }
}
