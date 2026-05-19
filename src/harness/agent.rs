//! AgentHarness — the concrete Think→Act implementation.
//!
//! Task 8 implemented the Think half of the loop. Task 9 added:
//!   * Act dispatch (executing tool_calls sequentially, emitting ToolResult /
//!     ToolError events).
//!   * Preservation of assistant `tool_use` intent inside `AssistantMessage`
//!     events so later Think cycles can reconstruct the conversation.
//!   * Full-history prompt assembly (now in `prompt.rs`) that re-emits the
//!     preceding assistant tool_use turn and resolves real tool names for
//!     `ToolResult` messages.
//!
//! Task 10 (Phase 6b) additionally consumes the optional triad on
//! `HarnessDeps`:
//!   * `context_budget.before_turn(...)` — drives compaction / hit_limit.
//!   * `context_compactor.compact(...)` — fires when budget directs warning.
//!   * `verifier_chain` — consulted between Think and Act every turn;
//!     a blocking verdict forces one more `Continue` so the model reacts.
//!
//! Split into sub-modules:
//!   * `think.rs` — `run_turn_internal`, `race_llm_call`, `run_verifiers`
//!   * `act.rs` — `act`
//!   * `guardrails.rs` — `apply_input_guardrail`, `apply_tool_call_guardrail`

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::harness::callback::{HarnessCallback, NoopHarnessCallback};
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnState};
use crate::providers::adapter::NativeToolCall;

use crate::session::events::{SessionEvent, SessionEventRecord, ToolOutput, TurnId};
use crate::session::service::SessionId;
use crate::verification::ToolCallSummary;

mod act;
mod guardrails;
mod think;

/// Outcome of `AgentHarness::apply_input_guardrail`. The two non-block
/// variants both carry the (possibly mutated) events vector; the caller
/// rebinds `events` to the returned vector before assembling the prompt.
pub(crate) enum InputGuardrailOutcome {
    /// Pass-through; events are unchanged.
    Allow(Vec<crate::session::events::SessionEventRecord>),
    /// Latest UserMessage's text was rewritten in-memory only — the
    /// session log retains the original event for audit.
    Sanitized(Vec<crate::session::events::SessionEventRecord>),
    /// Guardrail blocked the turn; caller emits `on_safety_block` and
    /// returns `TurnState::Done` without invoking the LLM.
    Blocked(String),
}

/// Stage 5b tool-call guardrail outcome. `Block` means the helper already
/// fired `on_safety_block` + emitted `ToolError`; the caller `continue`s.
pub(crate) enum ToolCallGuardOutcome {
    Pass,
    Sanitize(Value),
    Block,
}

pub struct AgentHarness {
    pub(super) deps: HarnessDeps,
    /// Tracks agent activity for stall detection. `None` when stall detection
    /// is disabled (no `stall_config` in deps).
    pub(super) stall_tracker: Option<crate::harness::deps::StallTracker>,
    /// Set when `context_budget.before_turn` returns `FinalReply`. Surfaced
    /// through [`AgentHarness::hit_limit`] so the orchestrator bridge can
    /// populate `FlowOutcome::hit_limit`.
    hit_limit: AtomicBool,
    /// Cumulative provider-reported token usage across every LLM call in
    /// this run (`input + output + cache_read + cache_creation`). Read after
    /// the run via [`AgentHarness::total_tokens`] by the orchestrator bridge
    /// and subagent spawner. A harness instance serves a single run, so the
    /// counter is never reset.
    total_tokens: AtomicU64,
}

impl AgentHarness {
    pub fn new(deps: HarnessDeps) -> Self {
        let stall_tracker = deps
            .stall_config
            .as_ref()
            .map(|config| crate::harness::deps::StallTracker::new(config.clone()));
        Self {
            deps,
            stall_tracker,
            hit_limit: AtomicBool::new(false),
            total_tokens: AtomicU64::new(0),
        }
    }

    /// `true` if a budget directive forced an early exit during this run.
    /// Cleared by [`AgentHarness::reset_hit_limit`] before a fresh run.
    pub fn hit_limit(&self) -> bool {
        self.hit_limit.load(Ordering::Relaxed)
    }

    /// Reset the hit_limit flag. Called before a fresh session drive so a
    /// previous run's budget trip does not leak into the next outcome.
    pub fn reset_hit_limit(&self) {
        self.hit_limit.store(false, Ordering::Relaxed);
    }

    /// Cumulative provider-reported token usage observed across every LLM
    /// call in this run. Components summed: `input + output + cache_read +
    /// cache_creation` (see `turn_token_total`). A harness instance serves
    /// exactly one run, so this counter is never reset.
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens.load(Ordering::Relaxed)
    }

    /// Read-only accessor for this harness's position in the subagent chain.
    /// Returns the root context for top-level agents (the `HarnessDeps`
    /// default). The subagent spawner overrides this with the descended
    /// chain when assembling a child harness. Stage 4 seam (#11).
    pub fn chain_context(&self) -> &crate::harness::chain_context::ChainContext {
        &self.deps.chain_context
    }

    /// Convenience: wrap this harness as an `Arc<dyn SessionDriver>` so it
    /// can be stored in containers that don't depend on the concrete type.
    pub fn into_session_driver(self) -> std::sync::Arc<dyn crate::session::SessionDriver> {
        std::sync::Arc::new(self)
    }

    /// Max consecutive verifier vetos before the harness gives up and
    /// forces Done. Prevents infinite loops when a hook permanently blocks.
    const MAX_VERIFIER_VETOS: usize = 10;

    /// Lazy-construct a `LoopTraceEvent` and forward to `trace_sink`.
    /// Returns immediately when no sink is wired — the closure is not invoked.
    pub(crate) fn emit<F>(&self, build: F)
    where
        F: FnOnce() -> crate::harness::trace::LoopTraceEvent,
    {
        if let Some(ref sink) = self.deps.trace_sink {
            sink.on_trace(&build());
        }
    }
}

#[async_trait]
impl crate::session::SessionDriver for AgentHarness {
    async fn drive(&self, session_id: &SessionId) -> crate::error::Result<()> {
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        self.run(session_id, &mut cb, &cancel)
            .await
            .map_err(|e| match e {
                HarnessError::Cancelled => crate::error::AlephError::Cancelled,
                HarnessError::Llm(inner) => inner,
                HarnessError::Tool(tool_err) => {
                    crate::error::AlephError::provider(format!("harness tool error: {tool_err}"))
                }
                HarnessError::Session(sess_err) => {
                    crate::error::AlephError::provider(format!("harness session error: {sess_err}"))
                }
                HarnessError::Stalled { elapsed } => {
                    crate::error::AlephError::provider(format!("agent stalled after {:?}", elapsed))
                }
                HarnessError::StalledTurn { phase, elapsed } => crate::error::AlephError::provider(
                    format!("agent turn stalled in {phase} after {elapsed:?}"),
                ),
            })
    }
}

#[async_trait]
impl Harness for AgentHarness {
    fn chain_context(&self) -> Option<&crate::harness::chain_context::ChainContext> {
        Some(self.chain_context())
    }

    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        let cap = self.deps.max_iterations;
        let mut iterations: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut verifier_veto_count: usize = 0;
        let mut consecutive_failure_turns: usize = 0;
        let mut tool_history: std::collections::VecDeque<ToolCallSummary> =
            std::collections::VecDeque::with_capacity(8);
        let mut tool_call_cache: std::collections::HashMap<(String, String), ToolOutput> =
            std::collections::HashMap::new();
        let result: Result<crate::harness::trace::LoopTraceSessionOutcome, HarnessError> = loop {
            if cancel.is_cancelled() {
                break Err(HarnessError::Cancelled);
            }
            if let Some(ref tracker) = self.stall_tracker {
                if tracker.is_stalled().await {
                    let elapsed = tracker.elapsed().await;
                    tracing::warn!(
                        ?session_id,
                        ?elapsed,
                        "stall watchdog tripped; forcing Done with hit_limit",
                    );
                    self.hit_limit.store(true, Ordering::Relaxed);
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                }
            }
            match self
                .run_turn_internal(
                    session_id,
                    callback,
                    iterations,
                    tool_calls_made,
                    &mut tool_history,
                    &mut tool_call_cache,
                    cancel,
                )
                .await
            {
                Err(HarnessError::StalledTurn { phase, elapsed }) => {
                    tracing::warn!(
                        ?session_id,
                        ?phase,
                        ?elapsed,
                        "per-turn timeout tripped; forcing Done with hit_limit",
                    );
                    self.hit_limit.store(true, Ordering::Relaxed);
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                }
                Err(e) => break Err(e),
                Ok((TurnState::Continue, executed, is_veto)) => {
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                    iterations = iterations.saturating_add(1);
                    tool_calls_made = tool_calls_made.saturating_add(executed);
                    if executed == 0 && !is_veto {
                        let events = self
                            .deps
                            .session
                            .get_events(session_id, None, None)
                            .await
                            .map_err(HarnessError::Session)?;
                        let last_assistant_idx = events
                            .iter()
                            .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
                            .unwrap_or(0);
                        let had_failure = events[last_assistant_idx..]
                            .iter()
                            .any(|r| matches!(r.event, SessionEvent::ToolError { .. }));
                        if had_failure {
                            consecutive_failure_turns = consecutive_failure_turns.saturating_add(1);
                            if let Some(cap) = self.deps.consecutive_failure_cap {
                                if consecutive_failure_turns >= cap {
                                    tracing::warn!(
                                        ?session_id,
                                        cap,
                                        "consecutive total-failure cap reached; forcing Done",
                                    );
                                    self.hit_limit.store(true, Ordering::Relaxed);
                                    callback.on_complete();
                                    break Ok(
                                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                                    );
                                }
                            }
                        } else {
                            consecutive_failure_turns = 0;
                        }
                    } else if executed > 0 {
                        consecutive_failure_turns = 0;
                    }
                    if is_veto {
                        verifier_veto_count = verifier_veto_count.saturating_add(1);
                        if verifier_veto_count >= Self::MAX_VERIFIER_VETOS {
                            tracing::warn!(
                                ?session_id,
                                max_vetos = Self::MAX_VERIFIER_VETOS,
                                "verifier veto limit reached; forcing Done to prevent infinite loop",
                            );
                            self.hit_limit.store(true, Ordering::Relaxed);
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    } else {
                        verifier_veto_count = 0;
                    }
                    if let Some(limit) = cap {
                        if iterations >= limit {
                            self.hit_limit.store(true, Ordering::Relaxed);
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    }
                }
                Ok((TurnState::Done, _, _)) => {
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::Completed);
                }
            }
        };

        match result {
            Ok(outcome) => {
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: self.total_tokens.load(Ordering::Relaxed) as usize,
                    hit_limit: matches!(
                        outcome,
                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                    ),
                    final_text: None,
                });
                Ok(())
            }
            Err(e) => {
                let error_class = e.class();
                tracing::warn!(
                    ?session_id,
                    ?error_class,
                    error = %e,
                    "harness session ended in error",
                );
                #[allow(clippy::match_same_arms)]
                let session_outcome = match error_class {
                    crate::error::ErrorClass::Recoverable
                    | crate::error::ErrorClass::Transient
                    | crate::error::ErrorClass::Fixable
                    | crate::error::ErrorClass::Unexpected => {
                        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                    }
                };
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome: session_outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: self.total_tokens.load(Ordering::Relaxed) as usize,
                    hit_limit: false,
                    final_text: None,
                });
                Err(e)
            }
        }
    }

    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError> {
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let iterations = count_assistant_messages(&events).saturating_add(1);
        let tool_calls_made = count_tool_calls(&events);
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut history = std::collections::VecDeque::new();
        let mut cache: std::collections::HashMap<(String, String), ToolOutput> =
            std::collections::HashMap::new();
        self.run_turn_internal(
            session_id,
            callback,
            iterations,
            tool_calls_made,
            &mut history,
            &mut cache,
            &cancel,
        )
        .await
        .map(|(state, _, _)| state)
    }
}

/// Extension trait on HarnessCallback so we can call `on_complete` even when
/// holding `&mut dyn HarnessCallback`. The direct fn call works on the
/// trait object; this is just a named shim to keep the call site readable.
pub(crate) trait HarnessCallbackExt {
    fn on_complete_via_harness(&mut self);
}

impl HarnessCallbackExt for dyn HarnessCallback + '_ {
    fn on_complete_via_harness(&mut self) {
        self.on_complete();
    }
}

/// Index at which events "since the last AssistantMessage" begin.
pub(crate) fn tail_start_index(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

/// Count `AssistantMessage` events in the log.
pub(crate) fn count_assistant_messages(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count()
}

/// Count `ToolCallRequested` events.
pub(crate) fn count_tool_calls(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolCallRequested { .. }))
        .count()
}

/// Serialize each `NativeToolCall` as a JSON `tool_use` block.
pub(crate) fn tool_use_blocks(tool_calls: &[NativeToolCall]) -> Vec<Value> {
    tool_calls
        .iter()
        .map(|c| {
            json!({
                "type": "tool_use",
                "id": c.id,
                "name": c.name,
                "input": c.arguments,
            })
        })
        .collect()
}

/// Stable serialization of a JSON value for memo cache keys.
pub(crate) fn canonical_json_string(value: &Value) -> String {
    fn canon(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::with_capacity(keys.len());
                for k in keys {
                    out.insert(k.clone(), canon(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&canon(value)).unwrap_or_default()
}

/// Find the most recent `TurnStarted` id; generate a fresh one if none exists.
pub(crate) fn current_turn_id(events: &[SessionEventRecord]) -> TurnId {
    events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .unwrap_or_else(uuid::Uuid::new_v4)
}

/// Sum the provider-reported token components for one LLM call.
///
/// `total_tokens` = `input + output + cache_read + cache_creation`. Cache
/// components default to 0 when the provider omits them. `thinking_tokens`
/// is intentionally excluded for a consistent cross-provider token
/// definition: Anthropic/OpenAI already fold thinking tokens into
/// `output_tokens` (counting them would double-count), and Gemini reports
/// `thoughtsTokenCount` separately. If Gemini thinking tokens ever need to
/// be counted, adjust the sum here.
fn turn_token_total(usage: &Option<crate::providers::adapter::TokenUsage>) -> u64 {
    match usage {
        None => 0,
        Some(u) => {
            u64::from(u.input_tokens)
                + u64::from(u.output_tokens)
                + u64::from(u.cache_read_tokens.unwrap_or(0))
                + u64::from(u.cache_creation_tokens.unwrap_or(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use crate::error::Result as AlephResult;
    use crate::harness::callback::NoopHarnessCallback;
    use crate::harness::deps::HarnessDeps;
    use crate::harness::trait_def::Harness;
    use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
    use crate::providers::AiProvider;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::ToolOutput;
    use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::service::{SessionId, SessionService};
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use serde_json::{json, Value};

    #[allow(dead_code)]
    struct RecordingProvider {
        captured: Arc<Mutex<Option<String>>>,
    }

    impl AiProvider for RecordingProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let captured = self.captured.clone();
            *captured.lock().unwrap() = payload.system_prompt.map(|s| s.to_string());
            Box::pin(async move { Ok(ProviderResponse::text_only("ok".to_string())) })
        }

        fn name(&self) -> &str {
            "recording"
        }

        fn color(&self) -> &str {
            "#000000"
        }
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

        fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
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
                },
            )
            .await
            .unwrap();
        (session, sid)
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
        assert_eq!(super::turn_token_total(&usage), 400);
    }

    #[test]
    fn turn_token_total_none_usage_is_zero() {
        assert_eq!(super::turn_token_total(&None), 0);
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
        assert_eq!(super::turn_token_total(&usage), 18);
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
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            fallback_llm: None,
            max_iterations: Some(3),
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

        // Single text-only turn: input + output + cache_read + cache_creation
        // = 10 + 20 + 5 + 3 = 38. thinking_tokens (99) is excluded.
        assert_eq!(harness.total_tokens(), 38);
    }

    #[tokio::test]
    async fn max_iterations_stops_runaway_loop() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn AiProvider> = Arc::new(LoopingProvider {
            calls: calls.clone(),
        });

        let (session, sid) = fresh_session("test-cap").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            fallback_llm: None,
            max_iterations: Some(3),
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = harness.run(&sid, &mut cb, &cancel).await;
        assert!(result.is_ok(), "run should succeed even when capped");
        assert!(harness.hit_limit(), "hit_limit should be true after cap");
        // 3 iterations = 3 calls to provider (0, 1, 2)
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn turn_timeout_returns_ok_with_hit_limit_not_err() {
        let provider: Arc<dyn AiProvider> = Arc::new(SleepingProvider {
            sleep: std::time::Duration::from_millis(200),
        });

        let (session, sid) = fresh_session("test-turn-timeout-ok-path").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            fallback_llm: None,
            max_iterations: None,
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: Some(std::time::Duration::from_millis(20)),
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = harness.run(&sid, &mut cb, &cancel).await;
        assert!(result.is_ok(), "turn timeout should return Ok, not Err");
        assert!(
            harness.hit_limit(),
            "hit_limit should be true after timeout"
        );
    }
}
