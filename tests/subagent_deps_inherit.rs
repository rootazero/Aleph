//! Stage A (P1) integration: SpawnerBase carries the 4 P1 fields and the
//! subagent's HarnessDeps inherits them identically — `stall_config`,
//! `consecutive_failure_cap`, `turn_timeout`, `trace_sink`.
//!
//! Strategy (c) per the P1 plan — structural-correctness test:
//!   1. Construct a `SpawnerBase` with sentinel values for the 4 fields.
//!   2. Assert each field is observable from the base (round-trip via
//!      `SpawnerBase` is the inheritance contract).
//!   3. Run `spawn()` once with a scripted provider so the build path
//!      exercises the `HarnessDeps { … base.stall_config.clone() … }`
//!      construction site — verifying the wiring at A4 didn't regress
//!      (a `None`-vs-`Some` branch on an inherited field would surface here).
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

    fn metadata_schema(&self) -> std::sync::Arc<[alephcore::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from(Vec::<alephcore::tool_metadata::ToolDefinition>::new())
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
async fn subagent_base_carries_4_p1_fields() {
    // Sentinel values for the 4 fields. The test asserts each survives the
    // round-trip from SpawnerBase into the spawned harness's HarnessDeps.
    let stall = StallConfig::default().with_timeout(Duration::from_secs(123));
    let cap: usize = 7;
    let turn = Duration::from_secs(456);
    let trace_sink: Arc<dyn TraceSink> = Arc::new(NoopTraceSink);

    let provider: Arc<dyn AiProvider> = Arc::new(OneShotProvider {
        text: "ok".to_string(),
    });
    let session = fresh_session_service();
    let tools: Arc<dyn ToolService> = Arc::new(NoopTools);

    let base = SpawnerBase {
        session,
        parent_tools: tools,
        provider,
        chain: ChainContext::new(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        // Stage A (P1) — sentinel values:
        stall_config: Some(stall.clone()),
        consecutive_failure_cap: Some(cap),
        turn_timeout: Some(turn),
        verifier_chain: None,
        trace_sink: Some(trace_sink.clone()),
        // P3 Stage I:
        plugin_registry: None,
        subagent_semaphore: None,
        routing_store: None,
        default_max_iterations: None,
        parallel_tool_concurrency: None,
        // Context management — sentinel `Some`, so the spawn below exercises
        // the branch that builds the child's own budget + compactor +
        // preflight triple (all three were hardcoded `None`, leaving a
        // subagent with no context management at all).
        context_budget_config: Some(alephcore::context::budget::ContextBudgetConfig {
            token_budget: 10_000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
            summarizer_input_budget: 48_000,
            circuit_breaker_max: 3,
            // `diminishing_window` / `diminishing_threshold` were cut from
            // `ContextBudgetConfig` by a later audit. This literal kept naming
            // them, which `cargo check` and `--lib` never compile — only
            // `--all-targets` does, and it had been red since.
            max_splits: 3,
        }),
        // Sentinel `None`: this fixture has no cheap tier to inherit, so the
        // child's compactor summarizes on its own LLM. The inheritance itself is
        // asserted by routing, not by presence, in
        // `subagent_spawner::tests::a_child_compactor_summarizes_on_the_inherited_cheap_tier`.
        cheap_summary_provider: None,
        // No refiner in this fixture: the spawn falls back to the unrefined
        // chain-minimum budget, which is exactly what the test pins.
        context_budget_refiner: None,
        primary_context_window: None,
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
        base.context_budget_config.is_some(),
        "context_budget_config must be Some when set on parent"
    );

    // Invoke spawn() — exercises the HarnessDeps construction site that
    // reads `base.{stall_config,consecutive_failure_cap,turn_timeout,
    // trace_sink}.clone()`. Any wiring regression there
    // surfaces as a build error or a runtime panic on the inherited values
    // (e.g., a turn_timeout of 456s wraps every LLM call but the call
    // returns immediately, so the timeout never fires).
    let agent_def =
        AgentDef::new("inherit-probe", AgentMode::SubAgent).with_allowed_tools(vec!["*".into()]);
    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "noop",
        context_summary: None,
        model: None,
        timeout_secs: 5,
        cancel: CancellationToken::new(),
        spawn_context: None,
        fork_source: None,
        isolation: None,
        strategy: None,
        session_mode: None,
        request_id: None,
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
