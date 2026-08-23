//! Stage 5a (#9) — harness-level integration tests for the input/output
//! guardrail callsites in `AgentHarness::run_turn_internal`.
//!
//! These tests drive a real `AgentHarness` through one turn with a wired
//! `GuardrailRegistry` and assert on (a) the text the provider receives
//! (input sanitisation), (b) the persisted `AssistantMessage` (output
//! sanitisation), and (c) the harness's terminal state (`Block` paths).

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::error::{ErrorClass, Result as AlephResult};
use crate::guardrails::decision::{GuardrailDecision, Replacement};
use crate::guardrails::registry::GuardrailRegistry;
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
use crate::harness::callback::HarnessCallback;
use crate::harness::tests::harness_ext::AgentHarnessTestExt;
use crate::harness::{AgentHarness, HarnessDeps, HarnessError, NoopHarnessCallback, TurnState};
use crate::providers::adapter::NativeToolCall;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::session::events::{
    now_ms, EventSeq, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

// -- Mock SessionService (same shape as think.rs / act.rs fixtures) ---------

#[derive(Default)]
struct MockSessionInner {
    events: Vec<SessionEventRecord>,
    next_seq: EventSeq,
}

struct MockSession {
    inner: Mutex<MockSessionInner>,
}

impl MockSession {
    fn new(initial: Vec<SessionEvent>) -> Arc<Self> {
        // Match the real store: seqs are assigned from 1 (0 = empty head).
        let mut inner = MockSessionInner {
            next_seq: 1,
            ..MockSessionInner::default()
        };
        for event in initial {
            let seq = inner.next_seq;
            inner.next_seq += 1;
            inner.events.push(SessionEventRecord {
                seq,
                event,
                created_at_ms: now_ms(),
            });
        }
        Arc::new(Self {
            inner: Mutex::new(inner),
        })
    }

    async fn snapshot(&self) -> Vec<SessionEventRecord> {
        self.inner.lock().await.events.clone()
    }
}

#[async_trait]
impl SessionService for MockSession {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> {
        let head_seq = self.inner.lock().await.next_seq.saturating_sub(1);
        Ok(SessionHandle { id, head_seq })
    }
    async fn get_events(
        &self,
        _id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        // Honor the seq range like the real store (`seq >= from && seq <= to`)
        // so range-based production reads (watermark tails) stay testable.
        let from = from.unwrap_or(0);
        let to = to.unwrap_or(EventSeq::MAX);
        Ok(self
            .inner
            .lock()
            .await
            .events
            .iter()
            .filter(|r| r.seq >= from && r.seq <= to)
            .cloned()
            .collect())
    }
    async fn emit_event(
        &self,
        _id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError> {
        let mut inner = self.inner.lock().await;
        let seq = inner.next_seq;
        inner.next_seq += 1;
        inner.events.push(SessionEventRecord {
            seq,
            event,
            created_at_ms: now_ms(),
        });
        Ok(seq)
    }
    async fn subscribe(
        &self,
        _id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError> {
        let (_tx, rx) = broadcast::channel(1);
        Ok(rx)
    }
    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError> {
        self.attach(id.clone()).await
    }
    async fn detach(&self, _id: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }
}

struct EmptyTools;
#[async_trait]
impl ToolService for EmptyTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, ToolError> {
        Err(ToolError::NotFound {
            name: name.to_string(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
}

/// Provider that records the user-text it sees in each `process()` call so
/// tests can assert on input sanitisation.
struct CapturingProvider {
    response: ProviderResponse,
    seen_user_text: Mutex<Vec<String>>,
}

impl CapturingProvider {
    fn text_only(text: &str) -> Arc<Self> {
        Arc::new(Self {
            response: ProviderResponse::text_only(text.to_string()),
            seen_user_text: Mutex::new(Vec::new()),
        })
    }
}

impl AiProvider for CapturingProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        // Record the text from every user-role message in the payload.
        let mut texts: Vec<String> = Vec::new();
        for msg in payload.messages {
            if matches!(msg, UnifiedMessage::User { .. }) {
                texts.push(msg.text_content());
            }
        }
        let response = self.response.clone();
        Box::pin(async move {
            self.seen_user_text.lock().await.extend(texts);
            Ok(response)
        })
    }
    fn name(&self) -> &str {
        "capturing"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

// -- Guardrail fakes ---------------------------------------------------------

struct BlockOnInput(&'static str);
#[async_trait]
impl InputGuardrail for BlockOnInput {
    fn name(&self) -> &str {
        "block_on_input"
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        if text.contains(self.0) {
            GuardrailDecision::Block {
                reason: format!("input matched {}", self.0),
                class: ErrorClass::Fixable,
            }
        } else {
            GuardrailDecision::Allow
        }
    }
}

struct SanitizeInput;
#[async_trait]
impl InputGuardrail for SanitizeInput {
    fn name(&self) -> &str {
        "sanitize_input"
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        if text.contains("SECRET") {
            GuardrailDecision::Sanitize(Replacement {
                text: text.replace("SECRET", "[REDACTED]"),
                source: "test".into(),
            })
        } else {
            GuardrailDecision::Allow
        }
    }
}

struct BlockOnOutput(&'static str);
#[async_trait]
impl OutputGuardrail for BlockOnOutput {
    fn name(&self) -> &str {
        "block_on_output"
    }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        if text.contains(self.0) {
            GuardrailDecision::Block {
                reason: format!("output matched {}", self.0),
                class: ErrorClass::Fixable,
            }
        } else {
            GuardrailDecision::Allow
        }
    }
}

struct SanitizeOutput;
#[async_trait]
impl OutputGuardrail for SanitizeOutput {
    fn name(&self) -> &str {
        "sanitize_output"
    }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        if text.contains("LEAK") {
            GuardrailDecision::Sanitize(Replacement {
                text: text.replace("LEAK", "***"),
                source: "test".into(),
            })
        } else {
            GuardrailDecision::Allow
        }
    }
}

// -- Capturing callback ------------------------------------------------------

#[derive(Default)]
struct CapturingCallback {
    safety_blocks: Vec<String>,
    /// `(call_id, is_error)` for every `on_tool_call_done` fired by the loop.
    /// Guards the ToolStart↔ToolEnd symmetry on the live broadcast stream.
    tool_done: Vec<(String, bool)>,
}
impl HarnessCallback for CapturingCallback {
    fn on_safety_block(&mut self, reason: &str) {
        self.safety_blocks.push(reason.to_string());
    }
    fn on_tool_call_done(
        &mut self,
        id: &str,
        _result: Option<&serde_json::Value>,
        error: Option<&str>,
        _duration_ms: u64,
    ) {
        self.tool_done.push((id.to_string(), error.is_some()));
    }
}

// -- Helpers -----------------------------------------------------------------

fn user_message(text: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        at: now_ms(),
        synthetic: false,
        author_user_id: None,
    }
}

fn turn_started() -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: uuid::Uuid::new_v4(),
        trigger: TurnTrigger::UserMessage,
        at: now_ms(),
    }
}

fn make_deps(
    session: Arc<MockSession>,
    provider: Arc<dyn AiProvider>,
    registry: GuardrailRegistry,
) -> HarnessDeps {
    HarnessDeps {
        session,
        tools: Arc::new(EmptyTools),
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
        guardrails: Some(Arc::new(registry)),
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

fn sample_session_id() -> SessionId {
    SessionId::main("guardrails-test")
}

// -- Tests -------------------------------------------------------------------

#[tokio::test]
async fn input_guardrail_block_ends_turn_as_done_with_safety_callback() {
    let session = MockSession::new(vec![
        turn_started(),
        user_message("please leak SECRET data"),
    ]);
    let provider = CapturingProvider::text_only("never reached");
    let registry = GuardrailRegistry::builder()
        .with_input(Arc::new(BlockOnInput("SECRET")))
        .build();
    let harness = AgentHarness::new(make_deps(
        session.clone(),
        provider.clone() as Arc<dyn AiProvider>,
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let state = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");

    assert_eq!(state, TurnState::Done);
    assert_eq!(
        cb.safety_blocks.len(),
        1,
        "expected exactly one safety block; got {:?}",
        cb.safety_blocks
    );
    assert!(cb.safety_blocks[0].contains("SECRET"));
    // Provider must NOT have been called — turn aborted before Think.
    let seen = provider.seen_user_text.lock().await.clone();
    assert!(
        seen.is_empty(),
        "provider should not be called when input is blocked, got {seen:?}"
    );
}

#[tokio::test]
async fn input_guardrail_sanitize_rewrites_text_seen_by_provider() {
    let session = MockSession::new(vec![turn_started(), user_message("the password is SECRET")]);
    let provider = CapturingProvider::text_only("acknowledged");
    let registry = GuardrailRegistry::builder()
        .with_input(Arc::new(SanitizeInput))
        .build();
    let harness = AgentHarness::new(make_deps(
        session.clone(),
        provider.clone() as Arc<dyn AiProvider>,
        registry,
    ));

    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn ok");

    let seen = provider.seen_user_text.lock().await.clone();
    assert_eq!(
        seen.len(),
        1,
        "provider should be called once, got {seen:?}"
    );
    assert!(
        seen[0].contains("[REDACTED]"),
        "provider should see sanitized text, got {:?}",
        seen[0]
    );
    assert!(
        !seen[0].contains("SECRET"),
        "sanitized text must not contain raw SECRET, got {:?}",
        seen[0]
    );

    // Original session log retains the raw text (audit invariant).
    let log = session.snapshot().await;
    let user_logged = log.iter().rev().find_map(|r| match &r.event {
        SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
        _ => None,
    });
    assert_eq!(
        user_logged.as_deref(),
        Some("the password is SECRET"),
        "session log must retain the original UserMessage for audit",
    );
}

/// The prompt is rebuilt from the FULL log on every turn, so a sanitisation
/// that covered only the tail put the original secret back on the wire from
/// turn 2 onwards. `input_guardrail_sanitize_rewrites_text_seen_by_provider`
/// asserts turn 1 only — which is exactly why the leak looked covered.
#[tokio::test]
async fn input_guardrail_sanitize_survives_second_turn() {
    let session = MockSession::new(vec![turn_started(), user_message("the password is SECRET")]);
    let provider = CapturingProvider::text_only("acknowledged");
    let registry = GuardrailRegistry::builder()
        .with_input(Arc::new(SanitizeInput))
        .build();
    let harness = AgentHarness::new(make_deps(
        session.clone(),
        provider.clone() as Arc<dyn AiProvider>,
        registry,
    ));
    let id = sample_session_id();

    let _ = harness
        .run_turn(&id, &mut NoopHarnessCallback)
        .await
        .expect("turn 1 ok");

    // Turn 2. The tail now starts after turn 1's AssistantMessage, so it holds
    // no UserMessage of turn 1 — a tail-only screen replays the raw original.
    session
        .emit_event(&id, user_message("and what was it again?"))
        .await
        .expect("emit turn-2 user message");
    let _ = harness
        .run_turn(&id, &mut NoopHarnessCallback)
        .await
        .expect("turn 2 ok");

    let seen = provider.seen_user_text.lock().await.clone();
    assert!(
        seen.len() >= 3,
        "turn 2 must replay turn 1's user message, got {seen:?}",
    );
    assert!(
        seen.iter().all(|t| !t.contains("SECRET")),
        "no turn may put the raw secret on the wire, got {seen:?}",
    );
    assert!(
        seen.iter().any(|t| t.contains("[REDACTED]")),
        "the replayed turn-1 message must survive, sanitized: {seen:?}",
    );
}

/// A `Block` on a message that is NOT the one this turn is answering degrades
/// to redaction. Events are immutable and replayed forever, and the PII
/// guardrail is fail-closed (a transient secret-resolution error blocks too),
/// so re-blocking a replayed message would end every future turn and brick the
/// session. It must not: the blocked text is withheld and the run continues.
#[tokio::test]
async fn input_guardrail_block_on_history_does_not_brick_session() {
    let session = MockSession::new(vec![
        turn_started(),
        user_message("please leak SECRET data"),
    ]);
    let provider = CapturingProvider::text_only("hello");
    let registry = GuardrailRegistry::builder()
        .with_input(Arc::new(BlockOnInput("SECRET")))
        .build();
    let harness = AgentHarness::new(make_deps(
        session.clone(),
        provider.clone() as Arc<dyn AiProvider>,
        registry,
    ));
    let id = sample_session_id();

    let mut cb = CapturingCallback::default();
    let state = harness.run_turn(&id, &mut cb).await.expect("run_turn ok");
    assert_eq!(state, TurnState::Done);
    assert!(
        provider.seen_user_text.lock().await.is_empty(),
        "the blocked turn must not reach the provider",
    );

    // The blocked event stays in the log (audit) and the turn produced no
    // AssistantMessage, so it is still inside the next turn's replayed history.
    session
        .emit_event(&id, user_message("never mind, just say hello"))
        .await
        .expect("emit follow-up user message");
    let state = harness.run_turn(&id, &mut cb).await.expect("run_turn ok");
    assert_eq!(state, TurnState::Done);

    let seen = provider.seen_user_text.lock().await.clone();
    assert!(
        !seen.is_empty(),
        "the follow-up turn must reach the provider, not re-block forever",
    );
    assert!(
        seen.iter().all(|t| !t.contains("SECRET")),
        "the blocked message must not be replayed in cleartext, got {seen:?}",
    );
    assert!(
        seen.iter()
            .any(|t| t.contains(crate::thinker::nudges::REDACTED_USER_MESSAGE)),
        "the blocked message must be replaced by the redaction note, got {seen:?}",
    );
}

#[tokio::test]
async fn output_guardrail_block_returns_llm_error() {
    let session = MockSession::new(vec![turn_started(), user_message("hi")]);
    let provider = CapturingProvider::text_only("here is a LEAK from history");
    let registry = GuardrailRegistry::builder()
        .with_output(Arc::new(BlockOnOutput("LEAK")))
        .build();
    let harness = AgentHarness::new(make_deps(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let err = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect_err("output block must fail the turn");
    assert!(
        matches!(err, HarnessError::Llm(_)),
        "expected HarnessError::Llm, got {err:?}",
    );
    assert!(cb.safety_blocks.iter().any(|r| r.contains("LEAK")));

    // No AssistantMessage should have been persisted.
    let log = session.snapshot().await;
    let assistant_count = log
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count();
    assert_eq!(
        assistant_count, 0,
        "blocked output must NOT be persisted as AssistantMessage; got log={log:?}",
    );
}

#[tokio::test]
async fn output_guardrail_sanitize_rewrites_persisted_assistant_message() {
    let session = MockSession::new(vec![turn_started(), user_message("hi")]);
    let provider = CapturingProvider::text_only("contains LEAK token");
    let registry = GuardrailRegistry::builder()
        .with_output(Arc::new(SanitizeOutput))
        .build();
    let harness = AgentHarness::new(make_deps(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        registry,
    ));

    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn ok");

    let log = session.snapshot().await;
    let assistant_text = log.iter().rev().find_map(|r| match &r.event {
        SessionEvent::AssistantMessage { content, .. } => Some(content.text.clone()),
        _ => None,
    });
    let text = assistant_text.expect("assistant message must be persisted");
    assert!(
        text.contains("***"),
        "sanitized text expected, got {:?}",
        text
    );
    assert!(
        !text.contains("LEAK"),
        "raw LEAK token leaked into persisted message: {:?}",
        text
    );
}

// ===========================================================================
// Stage 5b — Tool-call guardrail integration tests
// ===========================================================================

/// `ToolService` that records every `(name, args)` it sees for assertions and
/// returns a fixed Ok output. No name filtering — accepts any tool name.
struct RecordingTools {
    seen: Mutex<Vec<(String, serde_json::Value)>>,
}

impl RecordingTools {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl ToolService for RecordingTools {
    async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, ToolError> {
        self.seen.lock().await.push((name.to_string(), input));
        Ok(crate::session::events::ToolOutput {
            value: serde_json::json!({"ok": true}),
            metadata: Default::default(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
}

/// Blocks any tool call whose name matches `target`.
struct BlockToolByName(&'static str);
#[async_trait]
impl ToolCallGuardrail for BlockToolByName {
    fn name(&self) -> &str {
        "block_tool_by_name"
    }
    async fn evaluate_tool_call(
        &self,
        tool_name: &str,
        _args: &serde_json::Value,
    ) -> GuardrailDecision {
        if tool_name == self.0 {
            GuardrailDecision::Block {
                reason: format!("tool {} forbidden", self.0),
                class: ErrorClass::Fixable,
            }
        } else {
            GuardrailDecision::Allow
        }
    }
}

/// Sanitizes args containing a SECRET literal by replacing it with [REDACTED].
struct SanitizeToolArgs;
#[async_trait]
impl ToolCallGuardrail for SanitizeToolArgs {
    fn name(&self) -> &str {
        "sanitize_tool_args"
    }
    async fn evaluate_tool_call(
        &self,
        _tool_name: &str,
        args: &serde_json::Value,
    ) -> GuardrailDecision {
        let s = serde_json::to_string(args).unwrap_or_default();
        if s.contains("SECRET") {
            let rewritten = s.replace("SECRET", "[REDACTED]");
            GuardrailDecision::Sanitize(Replacement {
                text: rewritten,
                source: "test".into(),
            })
        } else {
            GuardrailDecision::Allow
        }
    }
}

/// Builds the deps used by the 5b tests — same shape as `make_deps` but with
/// a `RecordingTools` instead of `EmptyTools` so we can assert on what tools
/// actually got dispatched.
fn make_deps_with_tools(
    session: Arc<MockSession>,
    provider: Arc<dyn AiProvider>,
    tools: Arc<RecordingTools>,
    registry: GuardrailRegistry,
) -> HarnessDeps {
    HarnessDeps {
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
        guardrails: Some(Arc::new(registry)),
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

/// Build a provider that emits two tool calls in one turn.
fn provider_with_two_calls(
    first: NativeToolCall,
    second: NativeToolCall,
) -> Arc<CapturingProvider> {
    Arc::new(CapturingProvider {
        response: ProviderResponse {
            text: Some("dispatching".into()),
            tool_calls: vec![first, second],
            ..Default::default()
        },
        seen_user_text: Mutex::new(Vec::new()),
    })
}

#[tokio::test]
async fn tool_call_block_skips_only_blocked_call_in_batch() {
    // Provider asks for two tools; the first ("forbidden_tool") is blocked.
    // The second ("safe_tool") must still execute — Stage 5b spec says Block
    // skips ONE call, batch continues.
    let blocked = NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "forbidden_tool".into(),
        arguments: serde_json::json!({"x": 1}),
    };
    let allowed = NativeToolCall {
        thought_signature: None,
        id: "c2".into(),
        name: "safe_tool".into(),
        arguments: serde_json::json!({"y": 2}),
    };
    let session = MockSession::new(vec![turn_started(), user_message("run both")]);
    let provider = provider_with_two_calls(blocked.clone(), allowed.clone());
    let tools = RecordingTools::new();
    let registry = GuardrailRegistry::builder()
        .with_tool_call(Arc::new(BlockToolByName("forbidden_tool")))
        .build();
    let harness = AgentHarness::new(make_deps_with_tools(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        tools.clone(),
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let state = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");
    assert_eq!(state, TurnState::Continue);

    // Only the allowed tool reached the ToolService.
    let seen = tools.seen.lock().await.clone();
    assert_eq!(
        seen.len(),
        1,
        "expected exactly one tool dispatch, got {seen:?}"
    );
    assert_eq!(seen[0].0, "safe_tool");

    // safety_blocks fired once with the blocked tool name in the reason.
    assert_eq!(cb.safety_blocks.len(), 1, "got {:?}", cb.safety_blocks);
    assert!(cb.safety_blocks[0].contains("forbidden_tool"));

    // Live "done" event must fire for BOTH calls so the broadcast stream
    // closes every ToolStart with a ToolEnd: the blocked call as an error,
    // the safe call as a success. Before wiring `on_tool_call_done` the live
    // stream emitted starts with no ends.
    assert!(
        cb.tool_done.contains(&("c1".to_string(), true)),
        "blocked call must emit an error `done`, got {:?}",
        cb.tool_done
    );
    assert!(
        cb.tool_done.contains(&("c2".to_string(), false)),
        "safe call must emit a success `done`, got {:?}",
        cb.tool_done
    );

    // Session log: ToolError for the blocked call AND ToolResult for the safe one.
    let log = session.snapshot().await;
    let tool_errors: Vec<&SessionEventRecord> = log
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
        .collect();
    let tool_results: Vec<&SessionEventRecord> = log
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolResult { .. }))
        .collect();
    assert_eq!(
        tool_errors.len(),
        1,
        "expected 1 ToolError, got log={log:?}"
    );
    assert_eq!(
        tool_results.len(),
        1,
        "expected 1 ToolResult, got log={log:?}"
    );

    // BOTH calls must appear on the run's tool timeline. That timeline is the
    // authoritative terminal state (`FlowOutcome.tool_timeline` →
    // `RunSummary.tool_summaries`) consumers reconcile against precisely
    // because the `agent_trace` mirror is deliberately lossy — a blocked call
    // used to be the ONE outcome that never got a timeline entry, so losing its
    // single live frame left a row "running" with no backstop.
    let timeline = harness.tool_timeline();
    let blocked_entry = timeline
        .iter()
        .find(|inv| inv.id == "c1")
        .expect("blocked call must be on the tool timeline");
    assert!(!blocked_entry.success, "a block is not a success");
    assert!(
        blocked_entry
            .error
            .as_deref()
            .is_some_and(|e| e.contains("forbidden_tool")),
        "the blocked entry must carry the guardrail reason, got {blocked_entry:?}",
    );
    assert!(
        timeline.iter().any(|inv| inv.id == "c2" && inv.success),
        "the executed call must still be on the timeline, got {timeline:?}",
    );
}

#[tokio::test]
async fn tool_call_sanitize_rewrites_args_seen_by_tool() {
    let dirty = NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "send_message".into(),
        arguments: serde_json::json!({"body": "the password is SECRET"}),
    };
    let session = MockSession::new(vec![turn_started(), user_message("send")]);
    // CapturingProvider with a single tool call.
    let provider = Arc::new(CapturingProvider {
        response: ProviderResponse {
            text: Some("dispatching".into()),
            tool_calls: vec![dirty],
            ..Default::default()
        },
        seen_user_text: Mutex::new(Vec::new()),
    });
    let tools = RecordingTools::new();
    let registry = GuardrailRegistry::builder()
        .with_tool_call(Arc::new(SanitizeToolArgs))
        .build();
    let harness = AgentHarness::new(make_deps_with_tools(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        tools.clone(),
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let _ = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");

    // Tool was dispatched once, with the SECRET replaced by [REDACTED].
    let seen = tools.seen.lock().await.clone();
    assert_eq!(seen.len(), 1, "got {seen:?}");
    let args_str = serde_json::to_string(&seen[0].1).unwrap();
    assert!(
        args_str.contains("[REDACTED]"),
        "tool should see redacted args, got {args_str}"
    );
    assert!(
        !args_str.contains("SECRET"),
        "raw SECRET leaked into tool args: {args_str}"
    );
    assert!(
        cb.safety_blocks.is_empty(),
        "Sanitize must NOT fire on_safety_block, got {:?}",
        cb.safety_blocks
    );
}

/// Returns a `Sanitize` whose replacement text is NOT valid JSON, driving the
/// fail-closed reparse path in `apply_tool_call_guardrail`.
struct SanitizeToolArgsToInvalidJson(&'static str);
#[async_trait]
impl ToolCallGuardrail for SanitizeToolArgsToInvalidJson {
    fn name(&self) -> &str {
        "sanitize_tool_args_to_invalid_json"
    }
    async fn evaluate_tool_call(
        &self,
        tool_name: &str,
        _args: &serde_json::Value,
    ) -> GuardrailDecision {
        if tool_name == self.0 {
            GuardrailDecision::Sanitize(Replacement {
                // A redactor that rewrites args as prose rather than JSON —
                // the exact shape the fail-closed branch exists to catch.
                text: "[REDACTED by policy]".into(),
                source: "test".into(),
            })
        } else {
            GuardrailDecision::Allow
        }
    }
}

/// The fail-closed sanitize path is the SECOND producer of
/// `ToolCallGuardOutcome::Block`, and both `act` and `act_parallel` treat that
/// value as "the guardrail already emitted ToolError + trace" and do nothing
/// themselves. It used to return a bare `Block` after only `on_safety_block`,
/// so the call was requested and never resolved anywhere:
///
///   * no `SessionEvent::ToolError` ⇒ the turn's `tool_use` block has no
///     matching result, so the next `build_prompt` drops it as an orphan and
///     takes the whole assistant turn's context with it;
///   * no `on_tool_call_done` ⇒ a permanently "running" tool card on the
///     broadcast stream;
///   * no timeline entry ⇒ absent from `RunSummary.tool_summaries`, the
///     authoritative state consumers reconcile the lossy `agent_trace`
///     against.
///
/// Deliberately the same assertion set as
/// [`tool_call_block_skips_only_blocked_call_in_batch`]: the two decision
/// variants converge on one outcome value, so they owe the same receipts.
#[tokio::test]
async fn tool_call_sanitize_that_fails_to_reparse_settles_like_any_other_block() {
    let leaky = NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "leaky_tool".into(),
        arguments: serde_json::json!({"token": "SECRET"}),
    };
    let allowed = NativeToolCall {
        thought_signature: None,
        id: "c2".into(),
        name: "safe_tool".into(),
        arguments: serde_json::json!({"y": 2}),
    };
    let session = MockSession::new(vec![turn_started(), user_message("run both")]);
    let provider = provider_with_two_calls(leaky.clone(), allowed.clone());
    let tools = RecordingTools::new();
    let registry = GuardrailRegistry::builder()
        .with_tool_call(Arc::new(SanitizeToolArgsToInvalidJson("leaky_tool")))
        .build();
    let harness = AgentHarness::new(make_deps_with_tools(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        tools.clone(),
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let state = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");
    assert_eq!(state, TurnState::Continue);

    // Fail-closed: the un-sanitised args must never reach the tool, and the
    // sibling call must still run.
    let seen = tools.seen.lock().await.clone();
    assert_eq!(seen.len(), 1, "expected exactly one dispatch, got {seen:?}");
    assert_eq!(seen[0].0, "safe_tool");

    assert_eq!(cb.safety_blocks.len(), 1, "got {:?}", cb.safety_blocks);
    assert!(
        cb.safety_blocks[0].contains("invalid JSON"),
        "the block reason must name the reparse failure, got {:?}",
        cb.safety_blocks
    );

    // Live stream: every ToolStart needs its ToolEnd.
    assert!(
        cb.tool_done.contains(&("c1".to_string(), true)),
        "the fail-closed call must emit an error `done`, got {:?}",
        cb.tool_done
    );
    assert!(
        cb.tool_done.contains(&("c2".to_string(), false)),
        "the safe call must emit a success `done`, got {:?}",
        cb.tool_done
    );

    // Transcript: a ToolError pairs the orphaned tool_use block.
    let log = session.snapshot().await;
    let tool_errors = log
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
        .count();
    let tool_results = log
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolResult { .. }))
        .count();
    assert_eq!(tool_errors, 1, "expected 1 ToolError, got log={log:?}");
    assert_eq!(tool_results, 1, "expected 1 ToolResult, got log={log:?}");

    // Authoritative terminal state.
    let timeline = harness.tool_timeline();
    let entry = timeline
        .iter()
        .find(|inv| inv.id == "c1")
        .expect("the fail-closed call must be on the tool timeline");
    assert!(!entry.success, "a fail-closed block is not a success");
    assert!(
        entry
            .error
            .as_deref()
            .is_some_and(|e| e.contains("invalid JSON")),
        "the timeline entry must carry the reparse cause, got {entry:?}",
    );
    assert!(
        timeline.iter().any(|inv| inv.id == "c2" && inv.success),
        "the executed call must still be on the timeline, got {timeline:?}",
    );
}

#[tokio::test]
async fn tool_call_allow_passes_through_unchanged() {
    let call = NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "noop".into(),
        arguments: serde_json::json!({"k": "v"}),
    };
    let session = MockSession::new(vec![turn_started(), user_message("hi")]);
    let provider = Arc::new(CapturingProvider {
        response: ProviderResponse {
            text: Some("ack".into()),
            tool_calls: vec![call.clone()],
            ..Default::default()
        },
        seen_user_text: Mutex::new(Vec::new()),
    });
    let tools = RecordingTools::new();
    // BlockToolByName for a *different* tool name → registry is wired but
    // allows everything we actually call. Confirms the Allow path doesn't
    // accidentally rewrite or short-circuit.
    let registry = GuardrailRegistry::builder()
        .with_tool_call(Arc::new(BlockToolByName("some_other_tool")))
        .build();
    let harness = AgentHarness::new(make_deps_with_tools(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        tools.clone(),
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let _ = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");

    let seen = tools.seen.lock().await.clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "noop");
    assert_eq!(seen[0].1, call.arguments);
    assert!(cb.safety_blocks.is_empty());
}

// ===========================================================================
// Stage 5b — Tool-call guardrail on the PARALLEL fast path
//
// Before this change the parallel fast path bowed out entirely when any
// guardrail was wired, so guardrailed deployments never got parallel tool
// dispatch. The guardrail gate now runs in `act_parallel`'s sequential
// PASS 0 (Pi-style prep phase), so Block / Sanitize keep working while the
// execute phase still overlaps. These tests pin that behaviour.
// ===========================================================================

/// Like `RecordingTools` but reports every call as concurrent-safe, so the
/// harness takes the `act_parallel` fast path (the default `call_concurrency_claim`
/// returns `global()` (Exclusive), which would force the serial path).
struct ConcurrentRecordingTools {
    seen: Mutex<Vec<(String, serde_json::Value)>>,
}

impl ConcurrentRecordingTools {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl ToolService for ConcurrentRecordingTools {
    async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, ToolError> {
        self.seen.lock().await.push((name.to_string(), input));
        Ok(crate::session::events::ToolOutput {
            value: serde_json::json!({"ok": true}),
            metadata: Default::default(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
    async fn call_concurrency_claim(
        &self,
        _name: &str,
        _input: &serde_json::Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        // ConcurrentRecordingTools models parallel-safe tools so the two
        // guardrail tests exercise the act_parallel fast path (not the serial
        // fallback). Falling to the default global() claim would serialize them.
        crate::tools::concurrency::ConcurrencyClaim::Shared
    }
}

/// Same shape as `make_deps_with_tools` but with the parallel fast path armed
/// (`parallel_tool_concurrency: Some(8)`) and a `dyn ToolService` so any
/// concurrent-safe tool fixture can be plugged in.
fn make_parallel_deps(
    session: Arc<MockSession>,
    provider: Arc<dyn AiProvider>,
    tools: Arc<dyn ToolService>,
    registry: GuardrailRegistry,
) -> HarnessDeps {
    HarnessDeps {
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
        guardrails: Some(Arc::new(registry)),
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
        parallel_tool_concurrency: Some(8),
    }
}

#[tokio::test]
async fn tool_call_guardrail_block_applies_on_parallel_fast_path() {
    // Two concurrent-safe calls; the first is blocked by the guardrail. The
    // fast path must still apply the Block (only the safe call dispatches) and
    // keep Stage 5b semantics: 1 ToolError + 1 ToolResult, one safety_block.
    let blocked = NativeToolCall {
        thought_signature: None,
        id: "p1".into(),
        name: "forbidden_tool".into(),
        arguments: serde_json::json!({"x": 1}),
    };
    let allowed = NativeToolCall {
        thought_signature: None,
        id: "p2".into(),
        name: "safe_tool".into(),
        arguments: serde_json::json!({"y": 2}),
    };
    let session = MockSession::new(vec![turn_started(), user_message("run both")]);
    let provider = provider_with_two_calls(blocked, allowed);
    let tools = ConcurrentRecordingTools::new();
    let registry = GuardrailRegistry::builder()
        .with_tool_call(Arc::new(BlockToolByName("forbidden_tool")))
        .build();
    let harness = AgentHarness::new(make_parallel_deps(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        tools.clone(),
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let state = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");
    assert_eq!(state, TurnState::Continue);

    let seen = tools.seen.lock().await.clone();
    assert_eq!(
        seen.len(),
        1,
        "guardrail Block must be applied on the parallel path, got {seen:?}"
    );
    assert_eq!(seen[0].0, "safe_tool");

    assert_eq!(cb.safety_blocks.len(), 1, "got {:?}", cb.safety_blocks);
    assert!(cb.safety_blocks[0].contains("forbidden_tool"));

    let log = session.snapshot().await;
    let tool_errors = log
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
        .count();
    let tool_results = log
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolResult { .. }))
        .count();
    assert_eq!(tool_errors, 1, "expected 1 ToolError, got log={log:?}");
    assert_eq!(tool_results, 1, "expected 1 ToolResult, got log={log:?}");
}

#[tokio::test]
async fn tool_call_guardrail_sanitize_applies_on_parallel_fast_path() {
    // Two concurrent-safe calls, both carrying a SECRET. The guardrail rewrites
    // each args payload BEFORE the parallel execute phase, so both tools see
    // [REDACTED] and the fast path still overlaps them.
    let dirty1 = NativeToolCall {
        thought_signature: None,
        id: "p1".into(),
        name: "send_a".into(),
        arguments: serde_json::json!({"body": "key is SECRET"}),
    };
    let dirty2 = NativeToolCall {
        thought_signature: None,
        id: "p2".into(),
        name: "send_b".into(),
        arguments: serde_json::json!({"body": "also SECRET here"}),
    };
    let session = MockSession::new(vec![turn_started(), user_message("send")]);
    let provider = provider_with_two_calls(dirty1, dirty2);
    let tools = ConcurrentRecordingTools::new();
    let registry = GuardrailRegistry::builder()
        .with_tool_call(Arc::new(SanitizeToolArgs))
        .build();
    let harness = AgentHarness::new(make_parallel_deps(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        tools.clone(),
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let _ = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");

    let seen = tools.seen.lock().await.clone();
    assert_eq!(
        seen.len(),
        2,
        "both sanitized calls should dispatch on the parallel path, got {seen:?}"
    );
    for (_, args) in &seen {
        let s = serde_json::to_string(args).unwrap();
        assert!(
            s.contains("[REDACTED]"),
            "tool should see redacted args: {s}"
        );
        assert!(
            !s.contains("SECRET"),
            "raw SECRET leaked into tool args: {s}"
        );
    }
    assert!(
        cb.safety_blocks.is_empty(),
        "Sanitize must not fire on_safety_block, got {:?}",
        cb.safety_blocks
    );
}

#[tokio::test]
async fn concurrent_recording_tools_reports_shared_claim() {
    // Tripwire: the two *_on_parallel_fast_path guardrail tests only exercise
    // act_parallel while this stub claims Shared. If a refactor drops the
    // override, the claim falls to global() (Exclusive) and those tests
    // silently downgrade to the serial path — this assertion catches that.
    let tools = ConcurrentRecordingTools::new();
    assert!(matches!(
        tools
            .call_concurrency_claim("any", &serde_json::json!({}))
            .await,
        crate::tools::concurrency::ConcurrencyClaim::Shared
    ));
}

// ===========================================================================
// B17 — a guardrail rewrite voids the pre-admission disjointness proof
//
// Parallel admission derives each call's `ConcurrencyClaim` from the model's
// ORIGINAL args, but PASS 1 executes the guardrail-REWRITTEN args. A PII mask
// collapses distinct paths onto one placeholder (`[PHONE]`), so two writes
// admitted as disjoint end up truncating the SAME file concurrently. Any
// rewrite must therefore serialize the batch.
// ===========================================================================

/// Claims `Exclusive{Paths}` derived from `args["path"]` — so two calls naming
/// different paths are admitted as parallel-safe — and records the peak number
/// of executions that were ever in flight at once.
struct PathClaimTools {
    in_flight: std::sync::atomic::AtomicUsize,
    peak_in_flight: std::sync::atomic::AtomicUsize,
    seen: Mutex<Vec<serde_json::Value>>,
}

impl PathClaimTools {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            in_flight: std::sync::atomic::AtomicUsize::new(0),
            peak_in_flight: std::sync::atomic::AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl ToolService for PathClaimTools {
    async fn execute(
        &self,
        _name: &str,
        input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, ToolError> {
        use std::sync::atomic::Ordering;
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_in_flight.fetch_max(now, Ordering::SeqCst);
        // Hold the slot long enough that a genuinely parallel dispatch would
        // overlap; `.buffered(1)` cannot overlap no matter how long this is.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.seen.lock().await.push(input);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(crate::session::events::ToolOutput {
            value: serde_json::json!({"ok": true}),
            metadata: Default::default(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
    async fn call_concurrency_claim(
        &self,
        _name: &str,
        input: &serde_json::Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        let path = input
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string();
        crate::tools::concurrency::ConcurrencyClaim::Exclusive {
            scope: crate::tools::concurrency::ExclusiveScope::Paths(
                [path]
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>(),
            ),
        }
    }
}

/// The PII mask, modelled faithfully: every path collapses to one placeholder.
struct CollapsePathToPlaceholder;
#[async_trait]
impl ToolCallGuardrail for CollapsePathToPlaceholder {
    fn name(&self) -> &str {
        "collapse_path"
    }
    async fn evaluate_tool_call(
        &self,
        _tool_name: &str,
        _args: &serde_json::Value,
    ) -> GuardrailDecision {
        GuardrailDecision::Sanitize(Replacement {
            text: r#"{"path":"/data/customers/[PHONE].md"}"#.to_string(),
            source: "test".into(),
        })
    }
}

#[tokio::test]
async fn guardrail_rewrite_serializes_the_parallel_batch() {
    // Two writes to DISJOINT paths — admission proves them parallel-safe from
    // the original args. The guardrail then masks both onto the SAME path. If
    // the batch still dispatched at `parallel_tool_concurrency`, these would be
    // two concurrent truncating writes to one file (the underlying file_ops
    // write takes no lock). The fix drops the batch to `.buffered(1)`.
    let first = NativeToolCall {
        thought_signature: None,
        id: "w1".into(),
        name: "file_write".into(),
        arguments: serde_json::json!({"path": "/data/customers/13800138000.md"}),
    };
    let second = NativeToolCall {
        thought_signature: None,
        id: "w2".into(),
        name: "file_write".into(),
        arguments: serde_json::json!({"path": "/data/customers/13900139000.md"}),
    };
    let session = MockSession::new(vec![turn_started(), user_message("write both")]);
    let provider = provider_with_two_calls(first, second);
    let tools = PathClaimTools::new();
    let registry = GuardrailRegistry::builder()
        .with_tool_call(Arc::new(CollapsePathToPlaceholder))
        .build();
    let harness = AgentHarness::new(make_parallel_deps(
        session.clone(),
        provider as Arc<dyn AiProvider>,
        tools.clone(),
        registry,
    ));

    let mut cb = CapturingCallback::default();
    let _ = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn ok");

    let seen = tools.seen.lock().await.clone();
    assert_eq!(seen.len(), 2, "both calls should still run, got {seen:?}");
    // Precondition of the bug: the rewrite really did collapse them onto one path.
    for args in &seen {
        assert_eq!(
            args.get("path").and_then(|p| p.as_str()),
            Some("/data/customers/[PHONE].md"),
            "guardrail should have masked the path: {args:?}"
        );
    }
    // The assertion that fails without the fix (peak would be 2).
    assert_eq!(
        tools
            .peak_in_flight
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a guardrail rewrite voids the disjointness proof, so the batch must \
         dispatch serially — two concurrent writes to the masked path is the bug"
    );
}
