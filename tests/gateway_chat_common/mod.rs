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
    build_sandbox_factory, AgentHarnessRunner, BrainRef, DenyAllSandbox, FlowInput, FlowOutcome,
    FlowOverrides, FlowRegistry, FlowRequest, FlowSet, FlowSpec, FlowStreamEvent, HarnessRunner,
    Orchestrator, RoutingOverrides, SandboxKind, SessionStrategy,
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
            sandbox_kind: SandboxKind::None,
            session_strategy: SessionStrategy::Fresh,
            priority: 128,
            overrides: FlowOverrides::default(),
        }),
    );
    let flow_registry = Arc::new(FlowRegistry::new(specs));
    let mut defaults: HashMap<String, String> = HashMap::new();
    defaults.insert("main".into(), "gateway-chat".into());

    let session_service = fresh_session_service();
    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(DenyAllSandbox::new()) as Arc<dyn Sandbox>)
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
        default_provider: Arc::new(NeverProvider) as Arc<dyn AiProvider>,
        named_providers: HashMap::new(),
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
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
        parent_session: None,
        depth: 0,
        tool_service: None,
        trace_sink: None,
    }
}
