//! Tests for `AgentHarness` core behavior: failure/clean-turn
//! streak helpers, `has_unanswered_user_message` follow-up detection, token
//! accounting, and loop-cap termination (max iterations / turn timeout).
//!
//! Relocated (as part of the R10 harness-diet workflow) from an inline
//! `#[cfg(test)] mod tests` at the bottom of `src/harness/agent.rs`. Logic and
//! assertions are unchanged; only `super::X` references became
//! `crate::harness::agent::X` since this file lives under `harness::tests`, a
//! sibling of `harness::agent` rather than a child of it.

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;

use crate::error::Result as AlephResult;
use crate::harness::agent::{is_clean_turn, is_failure_turn, turn_token_total, AgentHarness};
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::session::events::ToolOutput;
use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
use crate::session::in_process::InProcessActorSessionService;
use crate::session::service::{SessionId, SessionService};
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use serde_json::{json, Value};

#[test]
fn failure_streak_counts_majority_failure_not_just_total_failure() {
    // (executed, errors) -> should this turn increment the streak?
    assert!(is_failure_turn(0, 2)); // total failure
    assert!(is_failure_turn(1, 3)); // majority failure (1 ok, 3 err)
    assert!(!is_failure_turn(3, 1)); // mostly success → not a failure turn
    assert!(!is_failure_turn(2, 0)); // clean → not a failure turn
}

#[test]
fn failure_streak_resets_only_on_clean_turn() {
    assert!(is_clean_turn(2, 0)); // zero errors → reset
    assert!(!is_clean_turn(2, 1)); // any error → hold/increment, don't reset
    assert!(!is_clean_turn(0, 1));
}

struct AlwaysOkTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for AlwaysOkTools {
    async fn execute(
        &self,
        _name: &str,
        _args: Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        Ok(ToolOutput {
            value: Value::String("ok".to_string()),
            metadata: Default::default(),
        })
    }

    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        vec![]
    }

    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }

    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
        Arc::from(vec![])
    }
}

struct LoopingProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl AiProvider for LoopingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ProviderResponse {
                tool_calls: vec![NativeToolCall {
                    thought_signature: None,
                    id: "loop".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"text": "loop"}),
                }],
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            })
        })
    }

    fn name(&self) -> &str {
        "looping"
    }

    fn color(&self) -> &str {
        "#ff0000"
    }
}

struct SleepingProvider {
    sleep: std::time::Duration,
}

impl AiProvider for SleepingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let sleep = self.sleep;
        Box::pin(async move {
            tokio::time::sleep(sleep).await;
            Ok(ProviderResponse::text_only("done".to_string()))
        })
    }

    fn name(&self) -> &str {
        "sleeping"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

struct UsageProvider {
    usage: crate::providers::adapter::TokenUsage,
}

impl AiProvider for UsageProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let usage = self.usage.clone();
        Box::pin(async move {
            Ok(ProviderResponse {
                text: Some("done".to_string()),
                stop_reason: StopReason::EndTurn,
                usage: Some(usage),
                ..Default::default()
            })
        })
    }

    fn name(&self) -> &str {
        "usage"
    }

    fn color(&self) -> &str {
        "#00ff00"
    }
}

async fn fresh_session(agent_id: &str) -> (Arc<InProcessActorSessionService>, SessionId) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).expect("migrate");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session = Arc::new(InProcessActorSessionService::new(store));
    let sid = SessionKey::Main {
        agent_id: agent_id.to_string(),
        main_key: "main".to_string(),
        epoch: 0,
    };
    session
        .emit_event(
            &sid,
            SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
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
                turn_id: uuid::Uuid::new_v4(),
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

// ---- Pi `getFollowUpMessages` parity: has_unanswered_user_message ----

/// Build a fresh, empty in-memory session (no seeded events, unlike
/// [`fresh_session`]) so each follow-up test controls the exact ordering.
fn empty_session(agent_id: &str) -> (Arc<InProcessActorSessionService>, SessionId) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).expect("migrate");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session = Arc::new(InProcessActorSessionService::new(store));
    let sid = SessionKey::Main {
        agent_id: agent_id.to_string(),
        main_key: "main".to_string(),
        epoch: 0,
    };
    (session, sid)
}

/// Minimal harness for unit-testing `has_unanswered_user_message`. The LLM
/// provider is never invoked — only the session dep is exercised.
fn followup_harness(session: Arc<InProcessActorSessionService>) -> AgentHarness {
    let deps = HarnessDeps {
        session,
        tools: Arc::new(AlwaysOkTools),
        llm: Arc::new(SleepingProvider {
            sleep: std::time::Duration::from_secs(0),
        }),
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
    };
    AgentHarness::new(deps)
}

async fn emit_user(
    session: &InProcessActorSessionService,
    sid: &SessionId,
    text: &str,
    synthetic: bool,
) {
    session
        .emit_event(
            sid,
            SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic,
                author_user_id: None,
            },
        )
        .await
        .unwrap();
}

async fn emit_assistant(session: &InProcessActorSessionService, sid: &SessionId, text: &str) {
    session
        .emit_event(
            sid,
            SessionEvent::AssistantMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                usage: None,
                at: now_ms(),
            },
        )
        .await
        .unwrap();
}

/// Set the per-turn prompt-boundary watermark the way `run_turn_internal`
/// would after reading a log whose last event has seq `seq`. Mirrors the
/// production store so the follow-up tests exercise the real watermark
/// logic. (The real store assigns seqs from 1, so a log of N contiguous
/// events has last seq N.)
fn set_watermark(harness: &AgentHarness, seq: u64) {
    harness
        .last_prompt_seq
        .store(seq, crate::sync_primitives::Ordering::Relaxed);
}

/// Normal completion: the model's last act is its assistant turn, so there
/// is nothing to follow up on — the loop must be allowed to terminate.
#[tokio::test]
async fn no_followup_when_assistant_is_last() {
    let (session, sid) = empty_session("fu-normal");
    emit_user(&session, &sid, "do the thing", false).await;
    emit_assistant(&session, &sid, "done").await;
    let harness = followup_harness(session);
    // Final turn built its prompt from the single leading user message.
    set_watermark(&harness, 1);
    assert!(!harness.has_unanswered_user_message(&sid).await);
}

/// The boundary race: a real user message lands *after* the final assistant
/// turn. The model has not seen it, so the loop must continue.
#[tokio::test]
async fn followup_when_user_message_arrives_after_final_turn() {
    let (session, sid) = empty_session("fu-race");
    emit_user(&session, &sid, "do the thing", false).await;
    emit_assistant(&session, &sid, "done").await;
    // Steering message injected after the closing turn committed.
    emit_user(&session, &sid, "actually, also do this", false).await;
    let harness = followup_harness(session);
    set_watermark(&harness, 1);
    assert!(harness.has_unanswered_user_message(&sid).await);
}

/// The harder boundary race the watermark exists for: a steering message is
/// injected *while the final turn's LLM call is still streaming*, so it lands
/// in the log *before* the assistant message that turn goes on to commit.
/// A naive "is there a user after the last assistant" test misses it; the
/// watermark (the turn's pre-prompt boundary) catches it.
#[tokio::test]
async fn followup_when_user_injected_during_final_turn() {
    let (session, sid) = empty_session("fu-during");
    emit_user(&session, &sid, "do the thing", false).await;
    // Final turn read the log here (len == 1) and started streaming.
    emit_user(&session, &sid, "wait, also handle errors", false).await;
    // ...then the turn commits its assistant message, now positioned *after*
    // the injected steering message.
    emit_assistant(&session, &sid, "done").await;
    let harness = followup_harness(session);
    set_watermark(&harness, 1);
    assert!(harness.has_unanswered_user_message(&sid).await);
}

/// A *synthetic* trailing user message (verifier-veto / grace-turn nudge) is
/// harness-internal, not genuine user input — it must not trigger a
/// follow-up continuation.
#[tokio::test]
async fn no_followup_for_synthetic_trailing_message() {
    let (session, sid) = empty_session("fu-synthetic");
    emit_user(&session, &sid, "do the thing", false).await;
    emit_assistant(&session, &sid, "done").await;
    emit_user(&session, &sid, "[verifier veto] keep going", true).await;
    let harness = followup_harness(session);
    set_watermark(&harness, 1);
    assert!(!harness.has_unanswered_user_message(&sid).await);
}

/// Before the first assistant turn there is no completed turn to "follow up"
/// on; the leading user prompt must not be mistaken for a late arrival.
#[tokio::test]
async fn no_followup_before_first_assistant_turn() {
    let (session, sid) = empty_session("fu-pre");
    emit_user(&session, &sid, "do the thing", false).await;
    let harness = followup_harness(session);
    assert!(!harness.has_unanswered_user_message(&sid).await);
}

#[test]
fn turn_token_total_sums_four_components() {
    use crate::providers::adapter::TokenUsage;
    let usage = Some(TokenUsage {
        input_tokens: 100,
        output_tokens: 250,
        cache_read_tokens: Some(40),
        cache_creation_tokens: Some(10),
        thinking_tokens: Some(999),
        cost: None,
    });
    // 100 + 250 + 40 + 10 = 400. thinking_tokens (999) is excluded.
    assert_eq!(turn_token_total(&usage), 400);
}

#[test]
fn turn_token_total_none_usage_is_zero() {
    assert_eq!(turn_token_total(&None), 0);
}

#[test]
fn turn_token_total_treats_missing_cache_as_zero() {
    use crate::providers::adapter::TokenUsage;
    let usage = Some(TokenUsage {
        input_tokens: 7,
        output_tokens: 11,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        thinking_tokens: None,
        cost: None,
    });
    assert_eq!(turn_token_total(&usage), 18);
}

#[test]
fn last_turn_context_tokens_reflects_latest_call_not_cumulative() {
    use crate::providers::adapter::TokenUsage;
    let (session, _sid) = empty_session("gauge-snapshot");
    let harness = followup_harness(session);

    // No LLM call yet → gauge numerator is 0.
    assert_eq!(harness.last_turn_context_tokens(), 0);

    let first = Some(TokenUsage {
        input_tokens: 100,
        output_tokens: 10,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        thinking_tokens: None,
        cost: None,
    });
    let second = Some(TokenUsage {
        input_tokens: 300,
        output_tokens: 20,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        thinking_tokens: None,
        cost: None,
    });
    harness.accumulate_token_breakdown(&first);
    harness.accumulate_token_breakdown(&second);

    // last-writer-wins: reflects the SECOND call (300 prompt + 20 output =
    // 320), NOT the run-cumulative 100+10+300+20 = 430.
    assert_eq!(harness.last_turn_context_tokens(), 320);
}

#[tokio::test]
async fn harness_accumulates_provider_token_usage() {
    use crate::providers::adapter::TokenUsage;
    let provider: Arc<dyn AiProvider> = Arc::new(UsageProvider {
        usage: TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: Some(5),
            cache_creation_tokens: Some(3),
            thinking_tokens: Some(99),
            cost: None,
        },
    });

    let (session, sid) = fresh_session("test-tokens").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);

    let deps = HarnessDeps {
        session,
        tools,
        llm: provider,
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
        max_iterations: Some(3),
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
    };
    let harness = AgentHarness::new(deps);
    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

    // Single text-only turn: input + output + cache_read + cache_creation
    // = 10 + 20 + 5 + 3 = 38. thinking_tokens (99) is excluded.
    assert_eq!(harness.total_tokens(), 38);

    // P2: token_breakdown captures per-component figures including the
    // reasoning slot that `total_tokens` deliberately drops.
    let bd = harness.token_breakdown();
    assert_eq!(bd.input, 10);
    assert_eq!(bd.output, 20);
    assert_eq!(bd.cache_read, 5);
    assert_eq!(bd.cache_creation, 3);
    assert_eq!(bd.reasoning, 99);
    assert_eq!(
        bd.total(),
        38,
        "breakdown.total() must agree with total_tokens()"
    );

    // Clean text-only run keeps the default Completed terminate reason.
    assert_eq!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::Completed,
    );
    // Wall-clock duration was stamped on entry — non-zero (or zero if
    // the run finished within sub-ms; just check it doesn't panic).
    let _ = harness.duration_ms();
}

#[tokio::test]
async fn max_iterations_stops_runaway_loop() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(LoopingProvider {
        calls: calls.clone(),
    });

    let (session, sid) = fresh_session("test-cap").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);

    let deps = HarnessDeps {
        session,
        tools,
        llm: provider,
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
        max_iterations: Some(3),
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
    };
    let harness = AgentHarness::new(deps);
    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let result = harness.run(&sid, &mut cb, &cancel).await;
    assert!(result.is_ok(), "run should succeed even when capped");
    assert!(harness.hit_limit(), "hit_limit should be true after cap");
    // 3 capped iterations = 3 provider calls, plus 1 grace turn fired by
    // the max_iterations cap (C1) — LoopingProvider never produces
    // terminal text, so the grace turn fires once more.
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);

    // P2: terminate_reason precisely identifies the max-iter cap (the
    // legacy hit_limit:bool could not distinguish this from stall /
    // verifier-veto / consecutive-failure / turn-timeout).
    assert_eq!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::HitMaxIterations { used: 3 },
    );

    // P2: tool_timeline captures one entry per Act-phase tool call. The
    // dedup memo (H4) is per-batch, so each of the 3 single-call turns
    // executes its own `echo` invocation.
    let timeline = harness.tool_timeline();
    assert_eq!(timeline.len(), 3, "one timeline entry per Act-phase call");
    assert!(timeline.iter().all(|i| i.success));
    assert_eq!(timeline[0].name, "echo");
}

#[tokio::test]
async fn turn_timeout_returns_ok_with_hit_limit_not_err() {
    let provider: Arc<dyn AiProvider> = Arc::new(SleepingProvider {
        sleep: std::time::Duration::from_millis(200),
    });

    let (session, sid) = fresh_session("test-turn-timeout-ok-path").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);

    let deps = HarnessDeps {
        session,
        tools,
        llm: provider,
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
        turn_timeout: Some(std::time::Duration::from_millis(20)),
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let result = harness.run(&sid, &mut cb, &cancel).await;
    assert!(result.is_ok(), "turn timeout should return Ok, not Err");
    assert!(
        harness.hit_limit(),
        "hit_limit should be true after timeout"
    );

    // P2: TurnTimeout is distinguishable from other cap reasons. The phase
    // string is always "Think" — `turn_timeout` only judges the LLM call; a
    // tool's wall clock lives in the tool layer as a recoverable timeout.
    match harness.terminate_reason() {
        crate::orchestrator::dispatch::TerminateReason::TurnTimeout { phase, .. } => {
            assert!(
                phase.to_lowercase().contains("think"),
                "expected think-phase timeout, got phase={phase}",
            );
        }
        other => panic!("expected TurnTimeout, got {other:?}"),
    }
}
