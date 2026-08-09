//! End-to-end test for `AgentHarness::run` (Phase 4b Task 10 / 4b.4).
//!
//! Exercises `AgentHarness`'s own Think→Act loop with a real
//! `InProcessActorSessionService` backed by an in-memory SQLite event store.
//! (The header used to claim it exercised the `Harness` trait's *default* `run`
//! body. It never did — `AgentHarness` always overrode `run`, so that default
//! executed nowhere; the trait has since been deleted.)
//! A scripted `AiProvider` drives the Think→Act loop:
//!   * Turn 0: assistant text + one `noop` tool_call (forces `TurnState::Continue`).
//!   * Turn 1: text-only response → `TurnState::Done` ends the loop.
//!
//! The second test runs the same fixture against two independent `SessionId`s
//! and verifies each session's event log stays isolated from the other's id.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use alephcore::harness::{AgentHarness, HarnessDeps, NoopHarnessCallback};
use alephcore::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload};
use alephcore::providers::AiProvider;
use alephcore::routing::session_key::SessionKey;
use alephcore::session::events::{SessionEvent, ToolOutput};
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::{SessionId, SessionService};
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService};
use alephcore::Result as AlephResult;

// -- Fixtures ---------------------------------------------------------------

/// Build a fresh `SessionService` backed by an in-memory SQLite store.
fn fresh_session_service() -> Arc<dyn SessionService> {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    migrate_add_session_events(&conn).expect("migrate session_events");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

/// A no-op sandbox. `HarnessDeps::sandbox` is required but the `NoopTool`
/// used here returns before ever reaching the sandbox, so `execute` should
/// never actually be called.
/// A tool that always succeeds with an empty JSON object — exercises the Act
/// phase without depending on real filesystem / network behavior.
struct NoopTool;

#[async_trait]
impl ToolService for NoopTool {
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

/// Scripted provider: turn 0 returns text + a single tool_call, every
/// subsequent turn returns a text-only response (driving the loop to `Done`).
///
/// Each instance owns its own step counter; tests that run multiple
/// sessions should use a fresh `Scripted` per session to avoid cross-session
/// step leakage.
struct Scripted {
    step: Mutex<u32>,
}

impl Scripted {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            step: Mutex::new(0),
        })
    }
}

impl AiProvider for Scripted {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let mut step = self.step.lock().await;
            let current = *step;
            *step += 1;
            if current == 0 {
                Ok(ProviderResponse {
                    text: Some("first turn".to_string()),
                    tool_calls: vec![NativeToolCall {
                        thought_signature: None,
                        id: "c".to_string(),
                        name: "noop".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                    ..Default::default()
                })
            } else {
                Ok(ProviderResponse::text_only("done".to_string()))
            }
        })
    }

    fn name(&self) -> &str {
        "scripted"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

/// Assemble a harness wired to a given session service + fresh Scripted llm.
fn make_harness(session: Arc<dyn SessionService>) -> AgentHarness {
    AgentHarness::new(HarnessDeps {
        session,
        tools: Arc::new(NoopTool),
        llm: Scripted::new(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        // `HarnessDeps.chain_context` was removed when the harness ratchet
        // turned back (see `src/harness/CLAUDE.md`); the module still exists,
        // the field does not. This initializer kept referencing it because
        // neither `cargo check` nor `cargo test --lib` compiles `tests/*` —
        // CLAUDE.md §10 blind spot, found by running the five-command set.
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(
            alephcore::memory::tool_signal_sink::NoopToolSignalSink,
        ),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
        robustness_profile: alephcore::verification::ModelRobustnessProfile::conservative(),
    })
}

// -- Tests ------------------------------------------------------------------

#[tokio::test]
async fn harness_run_loops_until_done() {
    let session = fresh_session_service();
    let harness = make_harness(session.clone());
    let sid: SessionId = SessionKey::ephemeral("e2e-1");

    harness
        .run(&sid, &mut NoopHarnessCallback, &CancellationToken::new())
        .await
        .expect("harness.run should complete");

    let events = session
        .get_events(&sid, None, None)
        .await
        .expect("get_events");

    let assistant_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count();
    let tool_result_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolResult { .. }))
        .count();

    assert_eq!(
        assistant_count, 2,
        "expected one AssistantMessage per turn (turn 0 tool_use + turn 1 done); got events: {events:#?}"
    );
    assert_eq!(
        tool_result_count, 1,
        "expected one ToolResult from turn 0's Act phase; got events: {events:#?}"
    );
}

#[tokio::test]
async fn multiple_sessions_do_not_cross_contaminate() {
    let session = fresh_session_service();
    let sid_a: SessionId = SessionKey::ephemeral("sess-A");
    let sid_b: SessionId = SessionKey::ephemeral("sess-B");

    // Two independent harnesses (each with its own Scripted step counter) so
    // session B doesn't inherit session A's step=2 state and short-circuit.
    let harness_a = make_harness(session.clone());
    let harness_b = make_harness(session.clone());

    let cancel = CancellationToken::new();
    harness_a
        .run(&sid_a, &mut NoopHarnessCallback, &cancel)
        .await
        .expect("run sid_a");
    harness_b
        .run(&sid_b, &mut NoopHarnessCallback, &cancel)
        .await
        .expect("run sid_b");

    let events_a = session
        .get_events(&sid_a, None, None)
        .await
        .expect("events sid_a");
    let events_b = session
        .get_events(&sid_b, None, None)
        .await
        .expect("events sid_b");

    assert!(!events_a.is_empty(), "session A should have events");
    assert!(!events_b.is_empty(), "session B should have events");

    // Neither log should mention the other session's agent id.
    for rec in &events_a {
        let json = serde_json::to_string(&rec.event).expect("serialize event");
        assert!(
            !json.contains("sess-B"),
            "session A event leaked sess-B reference: {json}"
        );
    }
    for rec in &events_b {
        let json = serde_json::to_string(&rec.event).expect("serialize event");
        assert!(
            !json.contains("sess-A"),
            "session B event leaked sess-A reference: {json}"
        );
    }
}
