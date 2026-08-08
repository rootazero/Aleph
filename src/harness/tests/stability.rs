//! Stability rescue test suite — covers TraceSink wiring, act() error
//! rescue, per-turn timeout, and StallTracker dispersion.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::error::Result as AlephResult;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::trace_sink::TraceSink;
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, ToolOutput, ToolOutputMetadata, TurnTrigger,
};
use crate::session::in_process::InProcessActorSessionService;
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

/// Captures every `LoopTraceEvent` for assertion.
pub(super) struct RecordingTraceSink {
    pub(super) events: Arc<Mutex<Vec<LoopTraceEvent>>>,
}

impl RecordingTraceSink {
    pub(super) fn new() -> (Arc<Self>, Arc<Mutex<Vec<LoopTraceEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Self {
            events: events.clone(),
        });
        (sink, events)
    }
}

impl TraceSink for RecordingTraceSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
    }
    fn flush(&self) {}
}

/// Provider whose `process` future never resolves. Used for timeout tests.
pub(super) struct HangingProvider;

impl AiProvider for HangingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
    fn name(&self) -> &str {
        "hanging"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider that returns one text-only response carrying a fixed token
/// `usage` — the Think loop sees no tool_calls and terminates in one turn.
pub(super) struct UsageTextProvider {
    pub(super) usage: crate::providers::adapter::TokenUsage,
}

impl AiProvider for UsageTextProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let usage = self.usage.clone();
        Box::pin(async move {
            Ok(ProviderResponse {
                text: Some("done".to_string()),
                stop_reason: crate::providers::adapter::StopReason::EndTurn,
                truncated_tool_call: None,
                usage: Some(usage),
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "usage-text"
    }
    fn color(&self) -> &str {
        "#00ff00"
    }
}

/// Provider that returns one tool_call (`name`) once, then text-only "done".
pub(super) struct OneShotToolProvider {
    pub(super) name: String,
    pub(super) calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl AiProvider for OneShotToolProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let calls = self.calls.clone();
        let tool = self.name.clone();
        Box::pin(async move {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        thought_signature: None,
                        id: format!("c-{n}"),
                        name: tool,
                        arguments: serde_json::json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    truncated_tool_call: None,
                    usage: None,
                })
            } else {
                Ok(ProviderResponse::text_only("done".into()))
            }
        })
    }
    fn name(&self) -> &str {
        "oneshot"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Tool service that succeeds for tools whose name starts with "ok_" and
/// fails for tools whose name starts with "fail_".
pub(super) struct MixedTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for MixedTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        if name.starts_with("fail_") {
            Err(crate::tools::service::ToolError::Other(format!(
                "mixed tool {name} forced fail"
            )))
        } else {
            Ok(ToolOutput {
                value: serde_json::json!({"name": name}),
                metadata: ToolOutputMetadata::default(),
            })
        }
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
}

/// A `LoopTool` that never returns, declaring a 150ms budget for itself.
///
/// Behind the production `ScopedToolService` (see [`hanging_tool_service`]) —
/// the tool layer is where a call's wall clock lives now, so a hang is bounded
/// there, not by the harness.
pub(super) struct HangingLoopTool;

#[async_trait::async_trait]
impl crate::tools::runtime::LoopTool for HangingLoopTool {
    fn name(&self) -> &str {
        "slow_tool"
    }
    fn description(&self) -> &str {
        "never returns"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> crate::tools::runtime::ToolResult {
        std::future::pending().await
    }
    fn max_duration_ms(&self) -> Option<u64> {
        Some(150)
    }
}

pub(super) fn hanging_tool_service() -> Arc<dyn crate::tools::service::ToolService> {
    let mut registry = crate::tools::runtime::LoopToolRegistry::new();
    registry.register(Box::new(HangingLoopTool));
    Arc::new(crate::tools::ScopedToolService::new(
        Arc::new(registry),
        std::collections::BTreeSet::new(),
    ))
}

/// Build a fresh attached session with one `TurnStarted` + `UserMessage`
/// pair so `harness.run` has work on first call.
pub(super) async fn fresh_session(
    tag: &str,
) -> (
    Arc<dyn crate::session::service::SessionService>,
    crate::session::service::SessionId,
) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session: Arc<dyn crate::session::service::SessionService> =
        Arc::new(InProcessActorSessionService::new(store));

    let sid = SessionKey::ephemeral(tag);
    session.attach(sid.clone()).await.unwrap();
    let turn = uuid::Uuid::new_v4();
    session
        .emit_event(
            &sid,
            SessionEvent::TurnStarted {
                turn_id: turn,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
        )
        .await
        .unwrap();
    session
        .emit_event(
            &sid,
            SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "go".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
                author_user_id: None,
            },
        )
        .await
        .unwrap();
    (session, sid)
}

/// Minimal `HarnessDeps` builder used by stability tests. All `Option` fields
/// default to `None`. Trace sink is `None` unless the test injects one.
///
/// Tests that need a different LLM/tool/sandbox set construct deps directly.
pub(super) fn minimal_deps(
    session: Arc<dyn crate::session::service::SessionService>,
    tools: Arc<dyn crate::tools::service::ToolService>,
    llm: Arc<dyn AiProvider>,
) -> HarnessDeps {
    HarnessDeps {
        session,
        tools,
        llm,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    }
}

use crate::harness::agent::AgentHarness;
use crate::harness::trait_def::HarnessError;

#[tokio::test]
async fn recording_sink_captures_full_lifecycle() {
    let (sink, events) = RecordingTraceSink::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(OneShotToolProvider {
        name: "ok_tool".into(),
        calls,
    });
    let (session, sid) = fresh_session("trace-lifecycle").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.trace_sink = Some(sink);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

    let captured = events.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let names: Vec<&str> = captured
        .iter()
        .map(|e| match e {
            LoopTraceEvent::TurnStarted { .. } => "TurnStarted",
            LoopTraceEvent::TurnStateEntered { .. } => "TurnStateEntered",
            LoopTraceEvent::TextEmitted { .. } => "TextEmitted",
            LoopTraceEvent::ToolCallStarted { .. } => "ToolCallStarted",
            LoopTraceEvent::ToolCallCompleted { .. } => "ToolCallCompleted",
            LoopTraceEvent::TurnCompleted { .. } => "TurnCompleted",
            LoopTraceEvent::SessionCompleted { .. } => "SessionCompleted",
            LoopTraceEvent::WorktreeCreated { .. } => "WorktreeCreated",
            LoopTraceEvent::WorktreeCleanedUp { .. } => "WorktreeCleanedUp",
            LoopTraceEvent::McpScopeAttached { .. } => "McpScopeAttached",
            LoopTraceEvent::McpScopeCleaned { .. } => "McpScopeCleaned",
            LoopTraceEvent::ProviderUsage { .. } => "ProviderUsage",
            LoopTraceEvent::ReactiveCompactionAttempted { .. } => "ReactiveCompactionAttempted",
            LoopTraceEvent::VerifierVeto { .. } => "VerifierVeto",
            LoopTraceEvent::MoaAdvisor { .. } => "MoaAdvisor",
            LoopTraceEvent::MoaAggregating { .. } => "MoaAggregating",
            LoopTraceEvent::MoaAdvisorSpend { .. } => "MoaAdvisorSpend",
            LoopTraceEvent::MoaTurnTrace { .. } => "MoaTurnTrace",
        })
        .collect();
    // 2 turns: tool turn + final text turn. Then SessionCompleted.
    assert!(
        names.contains(&"TurnStarted"),
        "missing TurnStarted: {names:?}"
    );
    assert!(
        names.iter().filter(|n| **n == "TurnStateEntered").count() >= 2,
        "expected at least 2 TurnStateEntered events: {names:?}",
    );
    assert!(
        names.contains(&"ToolCallStarted") && names.contains(&"ToolCallCompleted"),
        "missing tool lifecycle events: {names:?}",
    );
    assert!(
        names.last().copied() == Some("SessionCompleted"),
        "SessionCompleted should be last: {names:?}",
    );
}

#[tokio::test]
async fn noop_sink_zero_overhead() {
    // No sink wired — the harness must complete without ever building events.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(OneShotToolProvider {
        name: "ok_tool".into(),
        calls,
    });
    let (session, sid) = fresh_session("trace-zero").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    // trace_sink stays None — the helper sets it that way.
    let deps = minimal_deps(session, tools, provider);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("ok");
}

/// After Task 2 lands, a tool failure becomes a tool_result(is_error=true) in
/// the session log and the model gets a chance to recover on the next Think.
/// Currently (pre-Task 2), the harness aborts via `HarnessError::Tool`.
#[tokio::test]
async fn tool_failure_recovers_in_next_think() {
    struct RecoveryProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AiProvider for RecoveryProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![NativeToolCall {
                            thought_signature: None,
                            id: "c-0".into(),
                            name: "fail_one".into(),
                            arguments: serde_json::json!({}),
                        }],
                        thinking: None,
                        thinking_signature: None,
                        stop_reason: StopReason::ToolUse,
                        truncated_tool_call: None,
                        usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("recovered".into()))
                }
            })
        }
        fn name(&self) -> &str {
            "recovery"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(RecoveryProvider {
        calls: calls.clone(),
    });
    let (session, sid) = fresh_session("recover-tool-fail").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let deps = minimal_deps(session.clone(), tools, provider);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = harness.run(&sid, &mut cb, &cancel).await;

    assert!(
        outcome.is_ok(),
        "harness must not abort on tool error: {outcome:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "model should be called twice (tool turn + recovery turn)",
    );
    let events = session.get_events(&sid, None, None).await.unwrap();
    let has_tool_error = events
        .iter()
        .any(|r| matches!(r.event, SessionEvent::ToolError { .. }));
    assert!(has_tool_error, "session log must contain ToolError event");
}

#[tokio::test]
async fn partial_batch_failure_continues() {
    struct BatchProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AiProvider for BatchProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![
                            NativeToolCall {
                                thought_signature: None,
                                id: "a".into(),
                                name: "ok_a".into(),
                                arguments: serde_json::json!({}),
                            },
                            NativeToolCall {
                                thought_signature: None,
                                id: "b".into(),
                                name: "fail_b".into(),
                                arguments: serde_json::json!({}),
                            },
                            NativeToolCall {
                                thought_signature: None,
                                id: "c".into(),
                                name: "ok_c".into(),
                                arguments: serde_json::json!({}),
                            },
                        ],
                        thinking: None,
                        thinking_signature: None,
                        stop_reason: StopReason::ToolUse,
                        truncated_tool_call: None,
                        usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("done".into()))
                }
            })
        }
        fn name(&self) -> &str {
            "batch"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(BatchProvider { calls });
    let (session, sid) = fresh_session("partial-batch").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let deps = minimal_deps(session.clone(), tools, provider);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("ok");

    let events = session.get_events(&sid, None, None).await.unwrap();
    let n_results = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolResult { .. }))
        .count();
    let n_errors = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
        .count();
    assert_eq!(
        n_results, 2,
        "expected 2 ToolResult (ok_a, ok_c): events={events:#?}"
    );
    assert_eq!(
        n_errors, 1,
        "expected 1 ToolError (fail_b): events={events:#?}"
    );
}

#[tokio::test]
async fn consecutive_total_failure_caps_loop() {
    struct AlwaysFailProvider;
    impl AiProvider for AlwaysFailProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        thought_signature: None,
                        id: format!("c-{}", uuid::Uuid::new_v4()),
                        name: "fail_x".into(),
                        arguments: serde_json::json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    truncated_tool_call: None,
                    usage: None,
                })
            })
        }
        fn name(&self) -> &str {
            "always-fail"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(AlwaysFailProvider);
    let (session, sid) = fresh_session("cap-loop").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.consecutive_failure_cap = Some(3);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    )
    .await;
    outcome.expect("must terminate within 2s").expect("Ok exit");
    assert!(harness.hit_limit(), "hit_limit should be true after cap");
}

// Phase-2: per-turn timeouts (Think + Act) and the cross-turn stall watchdog
// route through Ok(HitLimit) instead of Err(HarnessError::StalledTurn|Stalled)
// so the gateway maps them to a friendly i18n ErrLoopExhausted reply rather
// than a FlowError::Internal red banner. Phase + tool_name detail is now
// emitted via tracing::warn! at the watchdog trip site, not the public API.

#[tokio::test]
async fn think_phase_timeout_terminates_via_hit_limit() {
    let provider: Arc<dyn AiProvider> = Arc::new(HangingProvider);
    let (session, sid) = fresh_session("think-timeout").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.turn_timeout = Some(std::time::Duration::from_millis(200));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    )
    .await
    .expect("must return within 2s");

    result
        .expect("Phase-2: Think-phase timeout must surface as Ok(HitLimit), not Err(StalledTurn)");
    assert!(
        harness.hit_limit(),
        "hit_limit must be true after Think turn timeout",
    );
    assert!(
        started.elapsed() < std::time::Duration::from_millis(800),
        "harness must abort within ~3× timeout, took {:?}",
        started.elapsed(),
    );
}

/// The Act phase is no longer judged by `turn_timeout`. That clock's one
/// remaining home is `think.rs::race_llm_call` — an LLM call has no human gate
/// in front of it, a tool call does. The harness used to wrap `turn_timeout`
/// around the whole Act future, which put the operator's approval wait *inside*
/// the tool's execution budget; production `turn_timeout` (120s) is the same
/// order as the approval timeout, so a slow human aborted the entire run.
///
/// A hung tool is now bounded where it should be — by its own budget, in
/// `ScopedToolService::execute_inner`, below the approval gate — and the overrun
/// comes back as a recoverable `ToolError`, so the run finishes normally instead
/// of dying with `hit_limit`.
#[tokio::test]
async fn hung_tool_is_bounded_by_its_own_budget_not_the_turn_timeout() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(OneShotToolProvider {
        name: "slow_tool".into(),
        calls,
    });
    let (session, sid) = fresh_session("act-timeout").await;

    let mut deps = minimal_deps(session, hanging_tool_service(), provider);
    // Ten seconds: long enough that if `turn_timeout` were still the Act clock,
    // the outer 2s guard below would fire first and fail the test.
    deps.turn_timeout = Some(std::time::Duration::from_secs(10));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    )
    .await
    .expect("the tool's own 150ms budget must bound the hang");

    result.expect("a tool budget overrun is recoverable, not a run abort");
    assert!(
        !harness.hit_limit(),
        "a recoverable tool timeout must not exhaust the run",
    );
}

#[tokio::test]
async fn parent_cancel_takes_precedence_over_timeout() {
    let provider: Arc<dyn AiProvider> = Arc::new(HangingProvider);
    let (session, sid) = fresh_session("cancel-vs-timeout").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.turn_timeout = Some(std::time::Duration::from_secs(1));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    )
    .await
    .expect("must return within 2s");

    assert!(
        matches!(result, Err(HarnessError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
}

#[tokio::test]
async fn outcome_mapping_for_stalled_turn() {
    let (sink, events) = RecordingTraceSink::new();
    let provider: Arc<dyn AiProvider> = Arc::new(HangingProvider);
    let (session, sid) = fresh_session("trace-stalled").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.turn_timeout = Some(std::time::Duration::from_millis(150));
    deps.trace_sink = Some(sink);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    )
    .await
    .expect("must return within 2s");

    let captured = events.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let session_completed = captured
        .iter()
        .rev()
        .find_map(|e| match e {
            LoopTraceEvent::SessionCompleted { outcome, .. } => Some(*outcome),
            _ => None,
        })
        .expect("SessionCompleted must be emitted");
    // Phase-2: StalledTurn now maps to HitLimit (not Cancelled) so the
    // gateway can render the friendly i18n ErrLoopExhausted reply.
    assert_eq!(
        session_completed,
        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
        "Phase-2: StalledTurn must map to HitLimit outcome",
    );
}

use crate::harness::StallConfig;

#[tokio::test]
async fn cross_turn_stall_still_works() {
    struct SlowTextProvider;
    impl AiProvider for SlowTextProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move {
                // 100ms per LLM call; nothing happens between calls.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                Ok(ProviderResponse::text_only("...".into()))
            })
        }
        fn name(&self) -> &str {
            "slow-text"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(SlowTextProvider);
    let (session, sid) = fresh_session("cross-turn-stall").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    // After Task 4, record_activity also fires inside the Think completion
    // path, so this test specifically exercises the "no Think completion at all"
    // case. We force that by pre-stalling the tracker via a 50ms budget.
    deps.stall_config =
        Some(StallConfig::default().with_timeout(std::time::Duration::from_millis(50)));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();

    // Sleep first to age the tracker past its budget BEFORE first turn.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    )
    .await
    .expect("must return within 2s");

    // Phase-2: cross-turn stall now routes through Ok(HitLimit) so the
    // gateway maps it to friendly i18n ErrLoopExhausted, not a fatal banner.
    result.expect("Phase-2: stall must surface as Ok(HitLimit), not Err(Stalled)");
    assert!(
        harness.hit_limit(),
        "hit_limit must be true after cross-turn stall trip",
    );
}

#[tokio::test]
async fn long_think_does_not_falsely_trip_stall() {
    // Provider takes 80ms per Think. Stall budget is 200ms. Model produces
    // text-only after first turn → Done. Without Task 4 dispersion, the
    // tracker would be aged 80ms+ at top of next iteration check, but with
    // dispersion it's reset right after Think.
    struct EightyMsThinkProvider {
        n: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AiProvider for EightyMsThinkProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let n = self.n.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                let v = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if v == 0 {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![NativeToolCall {
                            thought_signature: None,
                            id: "c".into(),
                            name: "ok_x".into(),
                            arguments: serde_json::json!({}),
                        }],
                        thinking: None,
                        thinking_signature: None,
                        stop_reason: StopReason::ToolUse,
                        truncated_tool_call: None,
                        usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("done".into()))
                }
            })
        }
        fn name(&self) -> &str {
            "80ms"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(EightyMsThinkProvider {
        n: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let (session, sid) = fresh_session("no-false-stall").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.stall_config =
        Some(StallConfig::default().with_timeout(std::time::Duration::from_millis(200)));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        harness.run(&sid, &mut cb, &cancel),
    )
    .await
    .expect("must finish within 3s");

    result.expect("legitimate two-turn run must succeed without stalling");
}

#[tokio::test]
async fn session_error_path_carries_error_class_in_tracing() {
    // Regression test for Stage 1: pin the class mapping that the
    // Err(e) branch in agent.rs now dispatches on. Pure unit-level
    // contract test — proves the seam is wired correctly without
    // having to spin up a full Harness session.
    use crate::error::{AlephError, ErrorClass};
    use crate::harness::trait_def::HarnessError;
    use crate::tools::service::ToolError;

    assert_eq!(
        HarnessError::Llm(AlephError::network("net blip")).class(),
        ErrorClass::Transient,
    );
    assert_eq!(HarnessError::Cancelled.class(), ErrorClass::Recoverable);
    assert_eq!(
        HarnessError::Tool(ToolError::NotFound {
            name: "ghost".into()
        })
        .class(),
        ErrorClass::Fixable,
    );
}

#[tokio::test]
async fn session_completed_and_turn_metrics_carry_total_tokens() {
    use crate::providers::adapter::TokenUsage;
    let (sink, events) = RecordingTraceSink::new();
    let provider: Arc<dyn AiProvider> = Arc::new(UsageTextProvider {
        usage: TokenUsage {
            input_tokens: 8,
            output_tokens: 14,
            cache_read_tokens: Some(2),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        },
    });
    let (session, sid) = fresh_session("trace-tokens").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.trace_sink = Some(sink);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

    let captured = events.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // Single text-only turn: 8 + 14 + 2 = 24.
    let session_completed = captured.iter().find_map(|e| match e {
        LoopTraceEvent::SessionCompleted { total_tokens, .. } => Some(*total_tokens),
        _ => None,
    });
    assert_eq!(
        session_completed,
        Some(24),
        "SessionCompleted.total_tokens should be the cumulative sum",
    );
    let turn_metrics = captured.iter().find_map(|e| match e {
        LoopTraceEvent::TurnCompleted { metrics, .. } => Some(metrics.total_tokens),
        _ => None,
    });
    assert_eq!(
        turn_metrics,
        Some(24),
        "TurnCompleted metrics.total_tokens should be the turn's usage sum",
    );
}
