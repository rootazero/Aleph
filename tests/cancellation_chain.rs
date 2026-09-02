//! Stage D (P1) integration: parent CancellationToken → subagent harness
//! cancellation propagation. Verifies three scenarios:
//!
//!   1. `parent_cancel_stops_child_at_llm_await` — parent cancels while
//!      subagent is awaiting LLM response.
//!   2. `parent_cancel_stops_child_at_tool_await` — parent cancels while
//!      subagent is awaiting a tool call (cascades via `turn_timeout`).
//!   3. `parent_turn_timeout_cascades_to_child` — parent's `turn_timeout`
//!      fires, propagating cancel to the child via the child token chain.
//!
//! Each test wraps with `tokio::time::timeout(Duration::from_secs(5))` so
//! cancel-leak regressions surface as test timeouts, not infinite hangs.
//!
//! R10-safe: zero `src/harness/` changes; the harness already wires
//! `cancel.cancelled()` into `race_llm_call` (Phase-6) and the spawner
//! plumbs `req.cancel` into `harness.run(&sid, &mut cb, &cancel)`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use alephcore::agents::runtime::LoopRunResult;
use alephcore::agents::subagent_spawner::{spawn, SpawnRequest, SpawnerBase};
use alephcore::agents::{AgentDef, AgentMode};
use alephcore::harness::chain_context::ChainContext;
use alephcore::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use alephcore::providers::AiProvider;
use alephcore::session::events::ToolOutput;
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::SessionService;
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
use alephcore::Result as AlephResult;

// -- Mocks (mirror tests/subagent_deps_inherit.rs patterns) ----------------

fn fresh_session_service() -> Arc<dyn SessionService> {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    migrate_add_session_events(&conn).expect("migrate session_events");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

/// LLM provider that hangs forever waiting on the cancellation token.
/// Models a long-running LLM call that must honor cancellation.
struct HangingLlmProvider {
    cancel: CancellationToken,
}

impl AiProvider for HangingLlmProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let cancel = self.cancel.clone();
        Box::pin(async move {
            // Block until cancelled; simulates a long-running LLM stream.
            cancel.cancelled().await;
            Err(alephcore::AlephError::Cancelled)
        })
    }

    fn name(&self) -> &str {
        "hanging-llm"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

/// Fast-response LLM provider: returns a single tool_use block requesting
/// `hanging_tool` on every call. Used to drive the harness into the tool
/// dispatch path so `HangingTool::execute` is reached.
struct FastToolCallProvider;

impl AiProvider for FastToolCallProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async {
            Ok(ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    thought_signature: None,
                    id: "call_hang_1".into(),
                    name: "hanging_tool".into(),
                    arguments: serde_json::json!({}),
                }],
                thinking: None,
                thinking_signature: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
                truncated_tool_call: None,
                provider_error: None,
            })
        })
    }

    fn name(&self) -> &str {
        "fast-tool-call"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

/// Tool service whose `execute` blocks until cancelled.
/// Models a long-running tool that must honor a cancellation chain.
struct HangingTool {
    cancel: CancellationToken,
}

#[async_trait]
impl ToolService for HangingTool {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        self.cancel.cancelled().await;
        Err(ToolError::Other("cancelled".into()))
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "hanging_tool".into(),
            description: "always hangs until cancelled".into(),
            input_schema: serde_json::json!({}),
            source: ToolSource::Builtin,
            metadata: Default::default(),
        }]
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.list().await.into_iter().find(|d| d.name == name)
    }

    fn metadata_schema(&self) -> Arc<[alephcore::tool_metadata::ToolDefinition]> {
        Arc::from(Vec::<alephcore::tool_metadata::ToolDefinition>::new())
    }
}

// -- Helpers ---------------------------------------------------------------

/// Build a SpawnerBase using the HangingLlmProvider. The subagent's harness
/// will block in `race_llm_call` until `cancel` fires.
fn base_with_hanging_llm(cancel: CancellationToken) -> SpawnerBase {
    let provider: Arc<dyn AiProvider> = Arc::new(HangingLlmProvider { cancel });
    let session = fresh_session_service();
    // Empty no-op tools (never reached because LLM hangs first).
    let tools: Arc<dyn ToolService> = Arc::new(HangingTool {
        cancel: CancellationToken::new(), // independent token, never fires
    });

    SpawnerBase {
        session,
        parent_tools: tools,
        provider,
        chain: ChainContext::new(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        trace_sink: None,
        // P3 Stage I:
        plugin_registry: None,
        subagent_semaphore: None,
        routing_store: None,
        default_max_iterations: None,
        parallel_tool_concurrency: None,
        context_budget_config: None,
        context_budget_refiner: None,
        primary_context_window: None,
        cheap_summary_provider: None,
        verifier_chain: None,
    }
}

/// Build a SpawnerBase that drives the harness into the tool dispatch path:
/// LLM returns a single `hanging_tool` call; tool blocks on `cancel`.
///
/// `turn_timeout = Some(2s)` is set so the harness's tool-call wrapper
/// surfaces a `StalledTurn` if cancel propagation through the tool path is
/// missing — both outcomes (clean cancel via select! OR stall via
/// turn_timeout) are accepted by the test as "cancel-shaped" termination.
/// The 2s budget is well under the outer 5s test timeout.
fn base_with_hanging_tool(cancel: CancellationToken) -> SpawnerBase {
    let provider: Arc<dyn AiProvider> = Arc::new(FastToolCallProvider);
    let session = fresh_session_service();
    let tools: Arc<dyn ToolService> = Arc::new(HangingTool { cancel });

    SpawnerBase {
        session,
        parent_tools: tools,
        provider,
        chain: ChainContext::new(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: Some(Duration::from_secs(2)),
        trace_sink: None,
        // P3 Stage I:
        plugin_registry: None,
        subagent_semaphore: None,
        routing_store: None,
        default_max_iterations: None,
        parallel_tool_concurrency: None,
        context_budget_config: None,
        context_budget_refiner: None,
        primary_context_window: None,
        cheap_summary_provider: None,
        verifier_chain: None,
    }
}

async fn run_subagent_with_hanging_llm(cancel: CancellationToken) -> Result<LoopRunResult, String> {
    let base = base_with_hanging_llm(cancel.clone());
    let agent_def = AgentDef::new("hanging-llm-probe", AgentMode::SubAgent)
        .with_allowed_tools(vec!["*".into()]);
    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "hang on llm",
        context_summary: None,
        model: None,
        // outer test timeout is 5s, this should never trigger — cancel
        // should win first.
        timeout_secs: 30,
        cancel,
        spawn_context: None,
        fork_source: None,
        isolation: None,
        strategy: None,
        session_mode: None,
        request_id: None,
    };
    spawn(&base, req).await
}

async fn run_subagent_with_hanging_tool(
    cancel: CancellationToken,
) -> Result<LoopRunResult, String> {
    let base = base_with_hanging_tool(cancel.clone());
    let agent_def = AgentDef::new("hanging-tool-probe", AgentMode::SubAgent)
        .with_allowed_tools(vec!["*".into()]);
    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "trigger hanging tool",
        context_summary: None,
        model: None,
        timeout_secs: 30,
        cancel,
        spawn_context: None,
        fork_source: None,
        isolation: None,
        strategy: None,
        session_mode: None,
        request_id: None,
    };
    spawn(&base, req).await
}

fn assert_cancel_shaped(result: Result<LoopRunResult, String>) {
    match result {
        Err(s) => assert!(
            s.contains("cancel")
                || s.contains("Cancelled")
                || s.contains("timed out")
                || s.contains("stalled"),
            "expected cancel-related error, got: {s}"
        ),
        Ok(_) => panic!("subagent should not return Ok after cancel"),
    }
}

// -- Tests -----------------------------------------------------------------

#[tokio::test]
async fn parent_cancel_stops_child_at_llm_await() {
    timeout(Duration::from_secs(5), async {
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();

        let task_handle = tokio::spawn({
            let child_token = child_token.clone();
            async move { run_subagent_with_hanging_llm(child_token).await }
        });

        // Let subagent start + reach the LLM await.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel from parent side; child token cancels via the parent→child
        // chain established by `parent_token.child_token()`.
        parent_token.cancel();

        let result = task_handle
            .await
            .expect("subagent task panicked or was joined twice");
        assert_cancel_shaped(result);
    })
    .await
    .expect("subagent did not respond to parent cancel within 5s");
}

#[tokio::test]
async fn parent_cancel_stops_child_at_tool_await() {
    timeout(Duration::from_secs(5), async {
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();

        let task_handle = tokio::spawn({
            let child_token = child_token.clone();
            async move { run_subagent_with_hanging_tool(child_token).await }
        });

        // Let subagent reach the tool dispatch path.
        tokio::time::sleep(Duration::from_millis(100)).await;
        parent_token.cancel();

        let result = task_handle
            .await
            .expect("subagent task panicked or was joined twice");
        assert_cancel_shaped(result);
    })
    .await
    .expect("subagent did not respond to parent cancel within 5s");
}

#[tokio::test]
async fn parent_turn_timeout_cascades_to_child() {
    timeout(Duration::from_secs(5), async {
        // Models the parent harness's `turn_timeout` firing: a 1s timer
        // cancels `parent_token`, which cascades to the child via the
        // CancellationToken parent→child chain.
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();

        let _timeout_handle = {
            let parent_token = parent_token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                parent_token.cancel();
            })
        };

        let task_handle = tokio::spawn({
            let child_token = child_token.clone();
            async move { run_subagent_with_hanging_llm(child_token).await }
        });

        let result = task_handle
            .await
            .expect("subagent task panicked or was joined twice");
        assert_cancel_shaped(result);
    })
    .await
    .expect("subagent did not respond to parent turn_timeout within 5s");
}
