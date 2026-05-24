//! Tests for `AgentHarness::run_turn` — Act phase + tool_use prompt
//! reconstruction (Task 9 / Phase 4b.3).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::error::Result as AlephResult;
use crate::harness::{AgentHarness, Harness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
use crate::sandbox::test_util::MockSandbox;
use crate::sandbox::SandboxOutput;
use crate::session::events::{
    now_ms, EventSeq, MessageContent, SessionEvent, SessionEventRecord, ToolOutput, TurnTrigger,
};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

// -- Mock SessionService -----------------------------------------------------

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

// -- Mock ToolService --------------------------------------------------------

/// Records each invocation and returns pre-programmed outcomes in order.
struct ScriptedTools {
    log: Mutex<Vec<(String, serde_json::Value)>>,
    outcomes: Mutex<Vec<Result<ToolOutput, ToolError>>>,
    /// Per-tool delay before resolving, used to verify that the parallel
    /// fast path actually overlaps execution. Sticky across calls; zero
    /// (the default) means resolve immediately.
    exec_delay: std::time::Duration,
    /// Static "every call I see is concurrent-safe" toggle. Used by the
    /// `act_parallel` test to opt into the fast path without changing
    /// global mocks.
    concurrent_safe: bool,
}

impl ScriptedTools {
    fn new(outcomes: Vec<Result<ToolOutput, ToolError>>) -> Arc<Self> {
        Arc::new(Self {
            log: Mutex::new(Vec::new()),
            outcomes: Mutex::new(outcomes),
            exec_delay: std::time::Duration::ZERO,
            concurrent_safe: false,
        })
    }

    fn new_concurrent(
        outcomes: Vec<Result<ToolOutput, ToolError>>,
        delay: std::time::Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            log: Mutex::new(Vec::new()),
            outcomes: Mutex::new(outcomes),
            exec_delay: delay,
            concurrent_safe: true,
        })
    }

    async fn calls(&self) -> Vec<(String, serde_json::Value)> {
        self.log.lock().await.clone()
    }
}

#[async_trait]
impl ToolService for ScriptedTools {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // Log the call BEFORE the delay so concurrent execution shows up as
        // overlapping log entries when multiple calls run in parallel.
        self.log
            .lock()
            .await
            .push((name.to_string(), input.clone()));
        if !self.exec_delay.is_zero() {
            tokio::time::sleep(self.exec_delay).await;
        }
        let mut outcomes = self.outcomes.lock().await;
        if outcomes.is_empty() {
            return Err(ToolError::Other(format!(
                "ScriptedTools ran out of outcomes (called {name})"
            )));
        }
        outcomes.remove(0)
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

    async fn is_call_concurrent_safe(&self, _name: &str, _input: &serde_json::Value) -> bool {
        self.concurrent_safe
    }
}

// -- Mock AiProvider with capture -------------------------------------------

/// Returns a fixed response; records the messages from each request so tests
/// can assert on prompt reconstruction.
struct CapturingProvider {
    response: ProviderResponse,
    captured: Mutex<Vec<Vec<UnifiedMessage>>>,
}

impl CapturingProvider {
    fn new(response: ProviderResponse) -> Arc<Self> {
        Arc::new(Self {
            response,
            captured: Mutex::new(Vec::new()),
        })
    }

    fn text_only(text: &str) -> Arc<Self> {
        Self::new(ProviderResponse::text_only(text.to_string()))
    }

    fn with_tool_calls(text: &str, calls: Vec<NativeToolCall>) -> Arc<Self> {
        Self::new(ProviderResponse {
            text: Some(text.to_string()),
            tool_calls: calls,
            ..Default::default()
        })
    }

    async fn last_request_messages(&self) -> Option<Vec<UnifiedMessage>> {
        self.captured.lock().await.last().cloned()
    }
}

impl AiProvider for CapturingProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let captured: Vec<UnifiedMessage> = payload.messages.to_vec();
        let response = self.response.clone();
        Box::pin(async move {
            self.captured.lock().await.push(captured);
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

// -- Helpers -----------------------------------------------------------------

fn sample_session_id() -> SessionId {
    SessionId::main("test")
}

fn noop_sandbox_output() -> SandboxOutput {
    SandboxOutput {
        exit_code: Some(0),
        ..Default::default()
    }
}

fn ok_output(value: serde_json::Value) -> ToolOutput {
    ToolOutput {
        value,
        metadata: Default::default(),
    }
}

fn user_message_event(text: &str) -> SessionEvent {
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
    }
}

fn turn_started_event() -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: uuid::Uuid::new_v4(),
        trigger: TurnTrigger::UserMessage,
        at: now_ms(),
    }
}

// -- Tests -------------------------------------------------------------------

#[tokio::test]
async fn act_executes_tools_sequentially() {
    let tool_calls = vec![
        NativeToolCall {
            thought_signature: None,
            id: "c1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a.txt"}),
        },
        NativeToolCall {
            thought_signature: None,
            id: "c2".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "b.txt"}),
        },
    ];

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let tools = ScriptedTools::new(vec![
        Ok(ok_output(serde_json::json!({"content": "A"}))),
        Ok(ok_output(serde_json::json!({"content": "B"}))),
    ]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    assert_eq!(state, TurnState::Continue);

    // Execution log has both calls, in order.
    let log = tools.calls().await;
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].0, "read_file");
    assert_eq!(log[0].1, serde_json::json!({"path": "a.txt"}));
    assert_eq!(log[1].0, "read_file");
    assert_eq!(log[1].1, serde_json::json!({"path": "b.txt"}));

    // Events: Assistant + 2×(ToolCallRequested, ToolResult).
    let events = session.snapshot().await;
    let requested = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolCallRequested { .. }))
        .count();
    let results = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolResult { .. }))
        .count();
    let errors = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
        .count();
    assert_eq!(requested, 2, "one ToolCallRequested per call");
    assert_eq!(results, 2, "one ToolResult per successful call");
    assert_eq!(errors, 0, "no ToolError on success path");
}

#[tokio::test]
async fn act_tool_failure_returns_harness_tool_error() {
    let tool_calls = vec![NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "missing.txt"}),
    }];

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let tools = ScriptedTools::new(vec![Err(ToolError::Execution {
        name: "read_file".into(),
        cause: "boom".into(),
    })]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools,
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
    let harness = AgentHarness::new(deps);

    // After Task 2: tool failures are rescued back to the model as
    // tool_result(is_error=true). run_turn must return Ok, not Err.
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn must succeed even on tool error");
    assert_eq!(
        state,
        TurnState::Continue,
        "tool failure → Continue, not Done"
    );

    let events = session.snapshot().await;
    let has_tool_error = events
        .iter()
        .any(|r| matches!(r.event, SessionEvent::ToolError { .. }));
    assert!(has_tool_error, "a ToolError event must have been emitted");
}

/// Seed a session with a prior completed tool turn, then assert that the
/// next Think pass reconstructs the assistant tool_use message and the
/// tool_result with the real tool name.
#[tokio::test]
async fn think_rebuilds_tool_use_turn_in_prompt() {
    let turn_id = uuid::Uuid::new_v4();

    // A prior tool_use round: user → assistant(tool_use) → requested → result.
    let assistant_blocks = vec![serde_json::json!({
        "type": "tool_use",
        "id": "c1",
        "name": "read_file",
        "input": {"path": "a.txt"},
    })];

    let initial = vec![
        SessionEvent::TurnStarted {
            turn_id,
            trigger: TurnTrigger::UserMessage,
            at: now_ms(),
        },
        SessionEvent::UserMessage {
            turn_id,
            content: MessageContent {
                text: "read a.txt".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: false,
        },
        SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent {
                text: "calling…".into(),
                blocks: assistant_blocks,
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
        },
        SessionEvent::ToolCallRequested {
            turn_id,
            call_id: "c1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "a.txt"}),
            at: now_ms(),
        },
        SessionEvent::ToolResult {
            turn_id,
            call_id: "c1".into(),
            output: ok_output(serde_json::json!({"content": "hello"})),
            at: now_ms(),
        },
    ];

    let session = MockSession::new(initial);
    let provider = CapturingProvider::text_only("done");

    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(ScriptedToolsNever),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");
    assert_eq!(state, TurnState::Done);

    let captured = provider
        .last_request_messages()
        .await
        .expect("provider should have captured one request");

    // Locate the assistant turn, then the tool_result immediately after.
    let mut saw_assistant_tool_use = false;
    let mut saw_named_tool_result = false;
    let mut i = 0;
    while i < captured.len() {
        if let UnifiedMessage::Assistant { content } = &captured[i] {
            let has_tool_use = content.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::ToolCall { id, name, .. }
                        if id == "c1" && name == "read_file"
                )
            });
            if has_tool_use {
                saw_assistant_tool_use = true;
                // Next message should be the tool_result with resolved name.
                if let Some(UnifiedMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    ..
                }) = captured.get(i + 1)
                {
                    if tool_call_id == "c1" && tool_name == "read_file" {
                        saw_named_tool_result = true;
                    }
                }
            }
        }
        i += 1;
    }

    assert!(
        saw_assistant_tool_use,
        "captured prompt must reconstruct assistant tool_use block; got: {captured:#?}"
    );
    assert!(
        saw_named_tool_result,
        "tool_result must carry the real tool name (not \"unknown\"); got: {captured:#?}"
    );
}

// Never-invoked tool service for tests where the LLM returns no tool_calls.
struct ScriptedToolsNever;

#[async_trait]
impl ToolService for ScriptedToolsNever {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        panic!("tool {name} must not be invoked in this test")
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

// -- Regression test for Fix 1: tool-error shadowing -------------------------

/// Wraps a `MockSession` but fails ONLY when the emitted event is
/// `SessionEvent::ToolError`. All other emits succeed as normal.
struct ToolErrorFailingSession {
    inner: Arc<MockSession>,
}

#[async_trait]
impl SessionService for ToolErrorFailingSession {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> {
        self.inner.attach(id).await
    }

    async fn get_events(
        &self,
        id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        self.inner.get_events(id, from, to).await
    }

    async fn emit_event(
        &self,
        id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError> {
        if matches!(event, SessionEvent::ToolError { .. }) {
            return Err(SessionError::Storage(
                "simulated storage failure on ToolError".into(),
            ));
        }
        self.inner.emit_event(id, event).await
    }

    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError> {
        self.inner.subscribe(id).await
    }

    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError> {
        self.inner.wake(id).await
    }

    async fn detach(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.detach(id).await
    }
}

#[tokio::test]
async fn act_tool_error_emit_failure_does_not_shadow_tool_error() {
    let tool_calls = vec![NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "boom.txt"}),
    }];

    let inner = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let session = Arc::new(ToolErrorFailingSession {
        inner: inner.clone(),
    });
    let tools = ScriptedTools::new(vec![Err(ToolError::Execution {
        name: "read_file".into(),
        cause: "nope".into(),
    })]);

    let deps = HarnessDeps {
        session,
        tools,
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
    let harness = AgentHarness::new(deps);

    // After Task 2: tool failures are rescued. The session emit failure for
    // ToolError is swallowed with a warning — run_turn must still return Ok.
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn must succeed even when ToolError emit fails");
    assert_eq!(state, TurnState::Continue, "tool failure → Continue");
}

// -- Round-trip test for Fix 3: writer/reader agreement ----------------------

/// Guards against drift between `tool_use_blocks` (writer in `Think`) and
/// `parse_tool_use_block` (reader in `build_prompt`): renaming a JSON field
/// on only one side would break the tool_use continuity across turns.
#[test]
fn tool_use_blocks_round_trip_through_parse_tool_use_block() {
    use crate::harness::agent::tool_use_blocks;
    use crate::harness::prompt::parse_tool_use_block;

    let calls = vec![
        NativeToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "/a"}),
            thought_signature: None,
        },
        NativeToolCall {
            id: "c2".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"cmd": "ls"}),
            thought_signature: None,
        },
        // A Gemini 3 call carrying a thoughtSignature — it must survive the
        // writer/reader round-trip so it can be replayed on later turns.
        NativeToolCall {
            id: "c3".into(),
            name: "search".into(),
            arguments: serde_json::json!({"q": "rust"}),
            thought_signature: Some("sig_xyz_base64".into()),
        },
    ];

    let blocks = tool_use_blocks(&calls);
    assert_eq!(blocks.len(), calls.len(), "one block per call");

    for (i, block) in blocks.iter().enumerate() {
        let parsed = parse_tool_use_block(block)
            .unwrap_or_else(|| panic!("parse_tool_use_block rejected block {i}: {block}"));
        match parsed {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
                thought_signature,
            } => {
                assert_eq!(id, calls[i].id, "id must round-trip for block {i}");
                assert_eq!(name, calls[i].name, "name must round-trip for block {i}");
                assert_eq!(
                    arguments, calls[i].arguments,
                    "arguments must round-trip for block {i}"
                );
                assert_eq!(
                    thought_signature, calls[i].thought_signature,
                    "thought_signature must round-trip for block {i}"
                );
            }
            other => panic!("expected ContentBlock::ToolCall, got {other:?}"),
        }
    }
}

// =============================================================================
// MED-2 — a retryable tool error is traced with `retryable: true`, not the
// hardcoded `false` that previously discarded `ToolError::is_retryable()`.
// =============================================================================
#[tokio::test]
async fn tool_error_trace_carries_retryable_flag() {
    let tool_calls = vec![NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "search".into(),
        arguments: serde_json::json!({}),
    }];
    let session = MockSession::new(vec![turn_started_event(), user_message_event("go")]);
    // `Transport` is a retryable `ToolError` variant (`is_retryable() == true`).
    let tools = ScriptedTools::new(vec![Err(ToolError::Transport {
        name: "search".into(),
        cause: "connection reset".into(),
    })]);
    let (sink, traced) = super::stability::RecordingTraceSink::new();
    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: Some(sink as Arc<dyn crate::harness::TraceSink>),
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
    let harness = AgentHarness::new(deps);
    harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    let events = traced.lock().unwrap();
    let retryable = events.iter().find_map(|e| match e {
        crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
            result: crate::tools::runtime::ToolResult::Error { retryable, .. },
            ..
        } => Some(*retryable),
        _ => None,
    });
    assert_eq!(
        retryable,
        Some(true),
        "a Transport (retryable) tool error must trace as retryable: true",
    );
}

// --- G3: tool-name auto-repair --------------------------------------------

/// ToolService that only knows lowercase tool names — `describe()` returns
/// `Some` only for the lowercase form. `execute()` records every name it
/// receives so the test can verify the harness sent the repaired form.
struct LowercaseOnlyTools {
    known: &'static str,
    calls: Mutex<Vec<String>>,
}

impl LowercaseOnlyTools {
    fn new(known: &'static str) -> Arc<Self> {
        Arc::new(Self {
            known,
            calls: Mutex::new(Vec::new()),
        })
    }

    async fn names_seen(&self) -> Vec<String> {
        self.calls.lock().await.clone()
    }
}

#[async_trait]
impl ToolService for LowercaseOnlyTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.lock().await.push(name.to_string());
        if name == self.known {
            Ok(ok_output(serde_json::json!({"ok": true})))
        } else {
            Err(ToolError::Other(format!("unknown tool: {name}")))
        }
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: self.known.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: crate::tools::service::ToolSource::Builtin,
            metadata: Default::default(),
        }]
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        if name == self.known {
            Some(ToolDefinition {
                name: self.known.to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: crate::tools::service::ToolSource::Builtin,
                metadata: Default::default(),
            })
        } else {
            None
        }
    }

    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
}

/// G3: a model that emits a CamelCase tool name (`Read_File`) when the
/// registry only knows the lowercase form (`read_file`) gets a silent
/// rename — the underlying ToolService.execute is invoked under the
/// repaired name, the call succeeds, and no ToolError event is emitted.
#[tokio::test]
async fn g3_repairs_case_mismatched_tool_name() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let tools = LowercaseOnlyTools::new("read_file");

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls(
            "calling…",
            vec![NativeToolCall {
                thought_signature: None,
                id: "c1".into(),
                name: "Read_File".into(),
                arguments: serde_json::json!({"path": "a.txt"}),
            }],
        ),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
    let harness = AgentHarness::new(deps);
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn ok");
    assert_eq!(state, TurnState::Continue);

    let names = tools.names_seen().await;
    assert_eq!(
        names,
        vec!["read_file".to_string()],
        "execute() must see the repaired lowercase name",
    );

    let events = session.snapshot().await;
    let errors = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
        .count();
    assert_eq!(errors, 0, "repaired call must succeed, no ToolError");
    let results = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolResult { .. }))
        .count();
    assert_eq!(results, 1, "repaired call produces exactly one ToolResult");
}

/// G3 negative case: if the lowercase variant is ALSO unknown, the call
/// proceeds with the original name and the normal ToolError path fires.
/// No silent rename, no shadow execution.
#[tokio::test]
async fn g3_does_not_repair_when_lowercase_is_also_unknown() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let tools = LowercaseOnlyTools::new("read_file");

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls(
            "calling…",
            vec![NativeToolCall {
                thought_signature: None,
                id: "c1".into(),
                name: "TotallyUnknownTool".into(),
                arguments: serde_json::json!({}),
            }],
        ),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
    let harness = AgentHarness::new(deps);
    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn returns Ok even on tool error");

    let names = tools.names_seen().await;
    assert_eq!(
        names,
        vec!["TotallyUnknownTool".to_string()],
        "execute() must see the original (untouched) name",
    );
    let events = session.snapshot().await;
    let errors = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
        .count();
    assert_eq!(errors, 1, "unknown tool must surface as ToolError");
}

// --- G1: MAX_STEPS soft hint ----------------------------------------------

/// G1: when `max_iterations` is set and this is the last allowed turn,
/// the assembled messages must include a trailing user-role
/// `<system-reminder>` warning the model to respond with text only. Saves
/// the post-hoc C1 grace-turn LLM round-trip in the common case.
#[tokio::test]
async fn g1_last_step_injects_max_steps_hint() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let tools = ScriptedTools::new(vec![]);
    let provider = CapturingProvider::text_only("final summary");

    let deps = HarnessDeps {
        session: session.clone(),
        tools,
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        // cap=1, fresh session → iterations passed = 1, 1+1 >= 1 → hint injects.
        max_iterations: Some(1),
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
    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn ok");

    let req = provider.last_request_messages().await.expect("captured");
    let last_user_text = req
        .iter()
        .rev()
        .find_map(|m| match m {
            UnifiedMessage::User { content } => content.first().and_then(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .expect("a user message in the request");
    assert!(
        last_user_text.contains("MAXIMUM ITERATIONS REACHED"),
        "last-step hint must be present in the prompt; got: {last_user_text}",
    );
}

/// G1 negative: when there are plenty of iterations left, no hint should
/// be injected.
#[tokio::test]
async fn g1_does_not_inject_when_not_last_step() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let tools = ScriptedTools::new(vec![]);
    let provider = CapturingProvider::text_only("ok");

    let deps = HarnessDeps {
        session: session.clone(),
        tools,
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        // cap=10, fresh session → iterations=1, 1+1=2 < 10 → no hint.
        max_iterations: Some(10),
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
    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn ok");

    let req = provider.last_request_messages().await.expect("captured");
    let any_user_text: String = req
        .iter()
        .filter_map(|m| match m {
            UnifiedMessage::User { content } => content.first().and_then(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !any_user_text.contains("MAXIMUM ITERATIONS REACHED"),
        "hint must NOT appear when iterations remain; got: {any_user_text}",
    );
}

// =============================================================================
// Parallel-dispatch fast path (opencode-parity)
// =============================================================================

/// When every call in a batch reports concurrent-safe and the harness has
/// `parallel_tool_concurrency` wired, the Act phase overlaps execution. We
/// assert this two ways:
///
/// 1. Wall-clock: 3 calls × 200ms delay each finish in well under 600ms
///    (the serial-path lower bound).
/// 2. Side-effect ordering: ToolCallRequested + ToolResult events still
///    interleave in input order, matching the serial-path contract.
#[tokio::test]
async fn act_parallel_overlaps_concurrent_safe_calls_and_preserves_order() {
    let tool_calls = vec![
        NativeToolCall {
            thought_signature: None,
            id: "p1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a.txt"}),
        },
        NativeToolCall {
            thought_signature: None,
            id: "p2".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "b.txt"}),
        },
        NativeToolCall {
            thought_signature: None,
            id: "p3".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "c.txt"}),
        },
    ];

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let tools = ScriptedTools::new_concurrent(
        vec![
            Ok(ok_output(serde_json::json!({"content": "A"}))),
            Ok(ok_output(serde_json::json!({"content": "B"}))),
            Ok(ok_output(serde_json::json!({"content": "C"}))),
        ],
        std::time::Duration::from_millis(200),
    );

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls.clone()),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
        parallel_tool_concurrency: Some(8),
    };
    let harness = AgentHarness::new(deps);

    let started = std::time::Instant::now();
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");
    let elapsed = started.elapsed();
    assert_eq!(state, TurnState::Continue);

    // 1. Wall-clock parity assertion. Serial would take ≥600ms; parallel
    //    should finish in well under that. Allow 500ms slack for slow CI.
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "parallel dispatch should overlap execution: 3×200ms calls took {:?}",
        elapsed
    );

    // 2. Side-effect ordering — ToolCallRequested events come out in input
    //    order, followed by ToolResult events also in input order (per the
    //    Pass 0 → Pass 1 → Pass 2 contract of act_parallel).
    let events = session.snapshot().await;
    let requested_ids: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolCallRequested { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let result_ids: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        requested_ids,
        vec!["p1".to_string(), "p2".to_string(), "p3".to_string()],
        "ToolCallRequested events must be emitted in input order",
    );
    assert_eq!(
        result_ids,
        vec!["p1".to_string(), "p2".to_string(), "p3".to_string()],
        "ToolResult events must be emitted in input order",
    );

    // The execution log records the actual ToolService::execute order. With
    // a concurrency cap of 8 and 3 calls, all three should kick off before
    // any completes (i.e., before the 200ms delay elapses).
    let log = tools.calls().await;
    assert_eq!(log.len(), 3, "every call must reach execute()");
}

/// When even one call in a batch is concurrent-unsafe, the fast path bows
/// out and the existing serial loop handles the whole batch. Wall-clock is
/// the cheapest signal: a 2-call batch with 200ms delay each must take
/// ≥400ms serially.
#[tokio::test]
async fn act_falls_back_to_serial_when_any_call_is_unsafe() {
    let tool_calls = vec![
        NativeToolCall {
            thought_signature: None,
            id: "s1".into(),
            name: "safe".into(),
            arguments: serde_json::json!({"k": 1}),
        },
        NativeToolCall {
            thought_signature: None,
            id: "u1".into(),
            name: "unsafe".into(),
            arguments: serde_json::json!({"k": 2}),
        },
    ];

    // `ScriptedTools::new` defaults to concurrent_safe = false, so even
    // though both names look benign, the harness sees the batch as unsafe
    // and routes through the serial loop.
    let tools = Arc::new(ScriptedTools {
        log: Mutex::new(Vec::new()),
        outcomes: Mutex::new(vec![
            Ok(ok_output(serde_json::json!({"ok": "s1"}))),
            Ok(ok_output(serde_json::json!({"ok": "u1"}))),
        ]),
        exec_delay: std::time::Duration::from_millis(200),
        concurrent_safe: false,
    });
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
        parallel_tool_concurrency: Some(8),
    };
    let harness = AgentHarness::new(deps);

    let started = std::time::Instant::now();
    harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(400),
        "serial fallback should take ≥400ms for 2×200ms calls; took {:?}",
        elapsed,
    );
}
