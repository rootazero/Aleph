//! Shared fixtures for Phase 5 orchestrator end-to-end integration tests.
//!
//! Wires a real `InProcessActorSessionService` + in-memory SQLite event store
//! to a scripted `AiProvider` and a minimal no-op `ToolService`. Mirrors
//! `tests/harness_run_e2e.rs` fixture conventions so the style stays uniform.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use alephcore::agents::AgentRegistry;
use alephcore::orchestrator::{
    build_sandbox_factory, AgentHarnessRunner, BrainRef, DenyAllSandbox, FlowOverrides,
    FlowRegistry, FlowSet, FlowSpec, Orchestrator, RoutingOverrides, SandboxKind,
    SessionStrategy,
};
use alephcore::providers::adapter::{ProviderResponse, RequestPayload};
use alephcore::providers::AiProvider;
use alephcore::sandbox::Sandbox;
use alephcore::session::events::ToolOutput;
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::SessionService;
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService};
use alephcore::Result as AlephResult;

/// Fresh `SessionService` on an in-memory SQLite backing store.
fn fresh_session_service() -> Arc<dyn SessionService> {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    migrate_add_session_events(&conn).expect("migrate session_events");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

/// No-op `ToolService` — the scripted provider in these e2e tests never
/// requests a tool_call, so `execute` is never hit. Keeping a concrete impl
/// instead of leaning on an existing one avoids dragging the full tool
/// registry into integration tests.
struct NoopToolService;

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

/// Scripted `AiProvider` — pops pre-queued text-only responses FIFO. Once
/// the queue is drained, every further call returns the last response again
/// so the Think loop converges on `Done`.
pub struct ScriptedLlm {
    queue: Mutex<Vec<String>>,
    sticky: String,
}

impl ScriptedLlm {
    pub fn new<I: IntoIterator<Item = String>>(responses: I) -> Arc<Self> {
        let mut queued: Vec<String> = responses.into_iter().collect();
        let sticky = queued.last().cloned().unwrap_or_default();
        queued.reverse();
        Arc::new(Self {
            queue: Mutex::new(queued),
            sticky,
        })
    }
}

impl AiProvider for ScriptedLlm {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let mut q = self.queue.lock().await;
            let text = q.pop().unwrap_or_else(|| self.sticky.clone());
            Ok(ProviderResponse::text_only(text))
        })
    }

    fn name(&self) -> &str {
        "scripted-e2e"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

/// Orchestrator + shared session service, pre-wired with a "main" →
/// "default-agent" route and a single-response scripted LLM.
pub struct OrchestratorFixture {
    pub orchestrator: Orchestrator,
    #[allow(dead_code)]
    pub session_service: Arc<dyn SessionService>,
}

impl OrchestratorFixture {
    /// Build a fixture where the scripted LLM emits exactly one text-only
    /// response — the Think loop sees no tool_calls and terminates immediately.
    pub async fn new_with_scripted_response(response: &str) -> Self {
        let mut spec_map = FlowSet::new();
        spec_map.insert(
            "default-agent".into(),
            Arc::new(FlowSpec {
                id: "default-agent".into(),
                description: "e2e default".into(),
                agent: "main".into(),
                brain: BrainRef::Default,
                sandbox_kind: SandboxKind::None,
                session_strategy: SessionStrategy::Fresh,
                overrides: FlowOverrides::default(),
            }),
        );
        let flow_registry = Arc::new(FlowRegistry::new(spec_map));

        let mut defaults: HashMap<String, String> = HashMap::new();
        defaults.insert("main".into(), "default-agent".into());

        let session_service = fresh_session_service();

        // Workspace builder is unused because the preset targets SandboxKind::None,
        // but SandboxFactory requires a closure — hand it a DenyAllSandbox too.
        let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
            Ok(Arc::new(DenyAllSandbox::new()) as Arc<dyn Sandbox>)
        }));

        let agent_registry = Arc::new(AgentRegistry::with_builtins());
        let tool_service: Arc<dyn ToolService> = Arc::new(NoopToolService);
        let scripted: Arc<dyn AiProvider> = ScriptedLlm::new([response.to_string()]);

        let harness_runner = Arc::new(AgentHarnessRunner {
            agent_registry,
            session_service: session_service.clone(),
            tool_service,
            default_provider: scripted,
            named_providers: HashMap::new(),
        });

        let orchestrator = Orchestrator::new(
            flow_registry,
            Arc::new(RoutingOverrides::default()),
            Arc::new(defaults),
            session_service.clone(),
            sandbox_factory,
            harness_runner,
        );

        Self {
            orchestrator,
            session_service,
        }
    }
}
