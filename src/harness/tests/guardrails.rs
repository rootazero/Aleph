//! Stage 5a (#9) — harness-level integration tests for the input/output
//! guardrail callsites in `AgentHarness::run_turn_internal`.
//!
//! These tests drive a real `AgentHarness` through one turn with a wired
//! `GuardrailRegistry` and assert on (a) the text the provider receives
//! (input sanitisation), (b) the persisted `AssistantMessage` (output
//! sanitisation), and (c) the harness's terminal state (`Block` paths).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::error::{ErrorClass, Result as AlephResult};
use crate::guardrails::decision::{GuardrailDecision, Replacement};
use crate::guardrails::registry::GuardrailRegistry;
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
use crate::providers::adapter::NativeToolCall;
use crate::harness::callback::HarnessCallback;
use crate::harness::{
    AgentHarness, Harness, HarnessDeps, HarnessError, NoopHarnessCallback, TurnState,
};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sandbox::test_util::MockSandbox;
use crate::sandbox::SandboxOutput;
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
        let mut inner = MockSessionInner::default();
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
        _from: Option<EventSeq>,
        _to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        Ok(self.inner.lock().await.events.clone())
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
    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
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
}
impl HarnessCallback for CapturingCallback {
    fn on_safety_block(&mut self, reason: &str) {
        self.safety_blocks.push(reason.to_string());
    }
}

// -- Helpers -----------------------------------------------------------------

fn noop_sandbox_output() -> SandboxOutput {
    SandboxOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: Some(0),
        signal: None,
        truncated: false,
        duration_ms: 0,
    }
}

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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: Some(Arc::new(registry)),
        fallback_llm: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
    }
}

fn sample_session_id() -> SessionId {
    SessionId::main("guardrails-test")
}

// -- Tests -------------------------------------------------------------------

#[tokio::test]
async fn input_guardrail_block_ends_turn_as_done_with_safety_callback() {
    let session = MockSession::new(vec![turn_started(), user_message("please leak SECRET data")]);
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
    assert!(seen.is_empty(), "provider should not be called when input is blocked, got {seen:?}");
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
    assert_eq!(seen.len(), 1, "provider should be called once, got {seen:?}");
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
    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: Some(Arc::new(registry)),
        fallback_llm: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
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
        id: "c1".into(),
        name: "forbidden_tool".into(),
        arguments: serde_json::json!({"x": 1}),
    };
    let allowed = NativeToolCall {
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
    assert_eq!(seen.len(), 1, "expected exactly one tool dispatch, got {seen:?}");
    assert_eq!(seen[0].0, "safe_tool");

    // safety_blocks fired once with the blocked tool name in the reason.
    assert_eq!(cb.safety_blocks.len(), 1, "got {:?}", cb.safety_blocks);
    assert!(cb.safety_blocks[0].contains("forbidden_tool"));

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
    assert_eq!(tool_errors.len(), 1, "expected 1 ToolError, got log={log:?}");
    assert_eq!(tool_results.len(), 1, "expected 1 ToolResult, got log={log:?}");
}

#[tokio::test]
async fn tool_call_sanitize_rewrites_args_seen_by_tool() {
    let dirty = NativeToolCall {
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

#[tokio::test]
async fn tool_call_allow_passes_through_unchanged() {
    let call = NativeToolCall {
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
