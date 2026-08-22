// Integration tests are compiled as separate crates; each binary uses
// only a subset of this shared helper module. Suppress dead-code and
// unused-import noise for items the current binary doesn't reference.
#![allow(dead_code, unused_imports)]

pub mod channel_contract;
pub mod mock_http;
pub mod mock_tcp;
pub mod mock_ws;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use alephcore::agents::AgentRegistry;
use alephcore::orchestrator::{
    build_sandbox_factory, AgentHarnessRunner, BrainRef, FlowOverrides, FlowRegistry, FlowSet,
    FlowSpec, Orchestrator, SessionStrategy,
};
use alephcore::providers::adapter::{ProviderResponse, RequestPayload, StopReason, TokenUsage};
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

    fn metadata_schema(&self) -> Arc<[alephcore::tool_metadata::ToolDefinition]> {
        Arc::from(Vec::<alephcore::tool_metadata::ToolDefinition>::new())
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
            // Report a fixed token usage so e2e tests can assert the
            // usage-surfacing path. 7 + 11 = 18 tokens per call.
            Ok(ProviderResponse {
                text: Some(text),
                stop_reason: StopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 7,
                    output_tokens: 11,
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    thinking_tokens: None,
                    cost: None,
                }),
                ..Default::default()
            })
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
        // The SHIPPED catalog, not a hand-written stand-in. The fixture used to
        // build its own `default-agent` with `SessionStrategy::Fresh`, which
        // meant the one end-to-end test of the orchestrator never touched
        // `presets/default_flows.toml` and could not have noticed that the real
        // presets discarded the caller's session key. A fixture that constructs
        // a state production never produces is guarding its own author.
        let flow_registry = Arc::new(FlowRegistry::new(
            alephcore::orchestrator::loader::load_presets().expect("presets parse"),
        ));

        let mut defaults: HashMap<String, String> = HashMap::new();
        defaults.insert("main".into(), "default-agent".into());

        let session_service = fresh_session_service();

        // The fixture never executes anything through the sandbox; the factory
        // just has to hand back some handle.
        let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
            Ok(Arc::new(alephcore::sandbox::NoopSandbox) as Arc<dyn Sandbox>)
        }));

        let agent_registry = Arc::new(AgentRegistry::with_builtins());
        let tool_service: Arc<dyn ToolService> = Arc::new(NoopToolService);
        let scripted: Arc<dyn AiProvider> = ScriptedLlm::new([response.to_string()]);

        let harness_runner = Arc::new(AgentHarnessRunner {
            agent_registry,
            session_service: session_service.clone(),
            tool_service,
            default_provider: Arc::new(alephcore::providers::StaticDefault::new(scripted))
                as Arc<dyn alephcore::providers::DefaultProviderHandle>,
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
        });

        let orchestrator = Orchestrator::new(
            flow_registry,
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
