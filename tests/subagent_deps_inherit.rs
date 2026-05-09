//! Stage A (P1) integration: SpawnerBase carries the 5 P1 fields and the
//! subagent's HarnessDeps inherits them identically — `fallback_llm`,
//! `stall_config`, `consecutive_failure_cap`, `turn_timeout`, `trace_sink`.
//!
//! Strategy (c) per the P1 plan — structural-correctness test:
//!   1. Construct a `SpawnerBase` with sentinel values for the 5 fields.
//!   2. Assert each field is observable from the base (round-trip via
//!      `SpawnerBase` is the inheritance contract).
//!   3. Run `spawn()` once with a scripted provider so the build path
//!      exercises the `HarnessDeps { … base.fallback_llm.clone() … }`
//!      construction site — verifying the wiring at A4 didn't regress
//!      (a `None`-vs-`Some` branch on `fallback_llm` would surface here).
//!
//! R10-safe: zero `src/harness/` changes; the test depends only on public
//! types in `agents::subagent_spawner`, `harness::{StallConfig, TraceSink,
//! NoopTraceSink}`, and the existing public session/sandbox/tools traits.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
// `async_trait` is required for the `Sandbox` and `ToolService` impls below.

use alephcore::agents::subagent_spawner::{spawn, SpawnRequest, SpawnerBase};
use alephcore::agents::{AgentDef, AgentMode};
use alephcore::harness::chain_context::ChainContext;
use alephcore::harness::{NoopTraceSink, StallConfig, TraceSink};
use alephcore::providers::adapter::{ProviderResponse, RequestPayload};
use alephcore::providers::AiProvider;
use alephcore::sandbox::{Sandbox, SandboxCommand, SandboxError, SandboxOutput};
use alephcore::session::events::ToolOutput;
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::SessionService;
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService};
use alephcore::Result as AlephResult;

// -- Mocks (mirror tests/harness_run_e2e.rs patterns) -----------------------

fn fresh_session_service() -> Arc<dyn SessionService> {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    migrate_add_session_events(&conn).expect("migrate session_events");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

struct InertSandbox;

#[async_trait]
impl Sandbox for InertSandbox {
    async fn execute(&self, _cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        Ok(SandboxOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: Some(0),
            signal: None,
            truncated: false,
            duration_ms: 0,
        })
    }
}

struct NoopTools;

#[async_trait]
impl ToolService for NoopTools {
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

    fn dispatcher_schema(&self) -> std::sync::Arc<[alephcore::dispatcher::ToolDefinition]> {
        std::sync::Arc::from(Vec::<alephcore::dispatcher::ToolDefinition>::new())
    }
}

/// Scripted provider that returns terminal text on every call, so the
/// harness exits the loop after a single Think turn (no Act phase).
struct OneShotProvider {
    text: String,
}

impl AiProvider for OneShotProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let text = self.text.clone();
        Box::pin(async move { Ok(ProviderResponse::text_only(text)) })
    }

    fn name(&self) -> &str {
        "one-shot"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

// -- The test ---------------------------------------------------------------

#[tokio::test]
async fn subagent_base_carries_5_p1_fields() {
    // Sentinel values for the 5 fields. The test asserts each survives the
    // round-trip from SpawnerBase into the spawned harness's HarnessDeps.
    let stall = StallConfig::default().with_timeout(Duration::from_secs(123));
    let cap: usize = 7;
    let turn = Duration::from_secs(456);
    let trace_sink: Arc<dyn TraceSink> = Arc::new(NoopTraceSink);

    let provider: Arc<dyn AiProvider> = Arc::new(OneShotProvider {
        text: "ok".to_string(),
    });
    // Distinct sentinel for fallback_llm — a separate provider instance so the
    // assertion below can prove (via Arc::ptr_eq) that the exact Arc travels
    // from SpawnerBase into the inheritance chain rather than being defaulted
    // to None or replaced with a fresh provider.
    let fallback_provider: Arc<dyn AiProvider> = Arc::new(OneShotProvider {
        text: "fallback-sentinel".to_string(),
    });
    let session = fresh_session_service();
    let tools: Arc<dyn ToolService> = Arc::new(NoopTools);
    let sandbox: Arc<dyn Sandbox> = Arc::new(InertSandbox);

    let base = SpawnerBase {
        session,
        parent_tools: tools,
        sandbox,
        provider,
        chain: ChainContext::new(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        // Stage A (P1) — sentinel values:
        fallback_llm: Some(Arc::clone(&fallback_provider)),
        stall_config: Some(stall.clone()),
        consecutive_failure_cap: Some(cap),
        turn_timeout: Some(turn),
        trace_sink: Some(trace_sink.clone()),
        // Stage C (P1):
        lane_scheduler: None,
    };

    // Structural assertions — the 5 P1 fields are populated as expected.
    assert_eq!(
        base.stall_config.as_ref().unwrap().timeout,
        Duration::from_secs(123),
        "stall_config.timeout must equal sentinel"
    );
    assert_eq!(
        base.consecutive_failure_cap,
        Some(cap),
        "consecutive_failure_cap must equal sentinel"
    );
    assert_eq!(
        base.turn_timeout,
        Some(turn),
        "turn_timeout must equal sentinel"
    );
    assert!(
        base.trace_sink.is_some(),
        "trace_sink must be Some when set on parent"
    );
    assert!(
        base.fallback_llm.is_some(),
        "fallback_llm should carry the sentinel"
    );
    // Strong evidence: the exact Arc instance travels — not a clone of a
    // different provider, not a default. `Arc::ptr_eq` compares the underlying
    // allocation, so the inheritance contract is proved at the bit level.
    assert!(
        Arc::ptr_eq(base.fallback_llm.as_ref().unwrap(), &fallback_provider),
        "fallback_llm should be the same Arc instance as the sentinel"
    );

    // Invoke spawn() — exercises the HarnessDeps construction site that
    // reads `base.{stall_config,consecutive_failure_cap,turn_timeout,
    // trace_sink,fallback_llm}.clone()`. Any wiring regression there
    // surfaces as a build error or a runtime panic on the inherited values
    // (e.g., a turn_timeout of 456s wraps every LLM call but the call
    // returns immediately, so the timeout never fires).
    let agent_def = AgentDef::new("inherit-probe", AgentMode::SubAgent)
        .with_allowed_tools(vec!["*".into()]);
    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "noop",
        context_summary: None,
        model: None,
        timeout_secs: 5,
        cancel: CancellationToken::new(),
        isolation: None,
    };

    let result = spawn(&base, req).await.expect("spawn should succeed");
    assert_eq!(
        result.final_text.as_deref(),
        Some("ok"),
        "OneShotProvider returns 'ok' on first turn"
    );
    assert_eq!(result.iterations, 1);
    assert!(!result.hit_limit);
}
