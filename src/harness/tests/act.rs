//! Tests for `AgentHarness::run_turn` — Act phase + tool_use prompt
//! reconstruction (Task 9 / Phase 4b.3).

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::error::Result as AlephResult;
use crate::harness::tests::harness_ext::AgentHarnessTestExt;
use crate::harness::{AgentHarness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
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

    async fn call_concurrency_claim(
        &self,
        _name: &str,
        _input: &serde_json::Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        if self.concurrent_safe {
            crate::tools::concurrency::ConcurrencyClaim::Shared
        } else {
            crate::tools::concurrency::ConcurrencyClaim::global()
        }
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
        author_user_id: None,
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
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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

/// The persisted `SessionEvent::ToolError.error` must carry the
/// fallback-registry persistence hint so the LLM sees a structured
/// routing signal in its next turn (same channel claude-code uses for
/// `is_error: true` tool_results). The hint is single-line, names the
/// classified error kind, recommends concrete alternative tools when
/// available, and ends with the `doctrine=...` reminder anchoring the
/// persistence doctrine in `thinker::layers::provider_guidance`.
#[tokio::test]
async fn act_tool_error_event_carries_persistence_hint() {
    let tool_calls = vec![NativeToolCall {
        thought_signature: None,
        id: "c1".into(),
        name: "web_fetch".into(),
        arguments: serde_json::json!({"url": "https://reuters.com/x"}),
    }];

    let session = MockSession::new(vec![turn_started_event(), user_message_event("fetch")]);
    let tools = ScriptedTools::new(vec![Err(ToolError::Execution {
        name: "web_fetch".into(),
        // 401 will classify as Unauthorized; the registry should
        // suggest alternate sources via search / autocli.
        cause: "HTTP 401 Unauthorized from reuters.com".into(),
    })]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools,
        llm: CapturingProvider::with_tool_calls("fetching…", tool_calls),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
    assert_eq!(
        state,
        TurnState::Continue,
        "tool failure must keep the loop alive — the persistence hint is consumed next turn"
    );

    let events = session.snapshot().await;
    let recorded = events
        .iter()
        .find_map(|r| match &r.event {
            SessionEvent::ToolError { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("a ToolError event must have been emitted");

    // Original cause survives intact (don't lose information when
    // appending the hint).
    assert!(
        recorded.contains("401"),
        "original 401 must survive in the recorded error: {recorded}"
    );
    // Structured kind label.
    assert!(
        recorded.contains("kind=unauthorized"),
        "kind label must match classifier: {recorded}"
    );
    // Tool name explicit (so the model doesn't have to correlate
    // call_id ↔ tool_name from earlier events).
    assert!(
        recorded.contains("tool=web_fetch"),
        "tool name must appear in hint: {recorded}"
    );
    // Registry-driven suggestion present for `web_fetch + Unauthorized`.
    assert!(
        recorded.contains("alternate source"),
        "registry suggestion missing: {recorded}"
    );
    // Doctrine anchor — persistence-doctrine reminder.
    assert!(
        recorded.contains("doctrine=ladder_must_be_climbed_before_fail"),
        "doctrine reminder must be present: {recorded}"
    );
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
            author_user_id: None,
        },
        SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent {
                text: "calling…".into(),
                blocks: assistant_blocks,
                thinking: None,
                thinking_signature: None,
            },
            usage: None,
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
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
    use crate::harness::agent::prompt::parse_tool_use_block;
    use crate::harness::agent::tool_use_blocks;

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
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: Some(sink as Arc<dyn crate::harness::TraceSink>),
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
    // TurnState is must_use; this test drives the loop once and inspects
    // trace events rather than the returned loop-control signal.
    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    let events = traced.lock().unwrap_or_else(|e| e.into_inner());
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
        llm: CapturingProvider::with_tool_calls(
            "calling…",
            vec![NativeToolCall {
                thought_signature: None,
                id: "c1".into(),
                name: "Read_File".into(),
                arguments: serde_json::json!({"path": "a.txt"}),
            }],
        ),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        llm: CapturingProvider::with_tool_calls(
            "calling…",
            vec![NativeToolCall {
                thought_signature: None,
                id: "c1".into(),
                name: "TotallyUnknownTool".into(),
                arguments: serde_json::json!({}),
            }],
        ),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls.clone()),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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

// -- Completion-order live events ---------------------------------------------

/// Sleeps for `delay_ms` from the call's own arguments, then succeeds. Lets a
/// single batch mix a slow and a fast call, which a sticky per-service delay
/// (`ScriptedTools::exec_delay`) cannot express.
struct DelayByArgTools;

#[async_trait]
impl ToolService for DelayByArgTools {
    async fn execute(
        &self,
        _name: &str,
        input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        let ms = input
            .get("delay_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        }
        Ok(ok_output(serde_json::json!({ "slept_ms": ms })))
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
        crate::tools::concurrency::ConcurrencyClaim::Shared
    }
}

/// Records every `on_tool_call_done` in firing order, with its duration.
#[derive(Default)]
struct DoneOrderCallback {
    done: Vec<(String, u64)>,
}

impl crate::harness::HarnessCallback for DoneOrderCallback {
    fn on_tool_call_done(
        &mut self,
        id: &str,
        _result: Option<&serde_json::Value>,
        _error: Option<&str>,
        duration_ms: u64,
    ) {
        self.done.push((id.to_string(), duration_ms));
    }
}

/// The live/transcript split: in a parallel batch, the live "done" event for a
/// FAST call fires as soon as that call resolves — completion order, real
/// duration — while the persisted `ToolResult` events stay in input order.
/// Before the completion drive loop, both were head-of-line blocked: the fast
/// call's live event waited for the slow sibling and reported an inflated
/// duration (the wait, not the work).
#[tokio::test]
async fn act_parallel_fires_live_done_in_completion_order_with_real_durations() {
    let tool_calls = vec![
        NativeToolCall {
            thought_signature: None,
            id: "p-slow".into(),
            name: "sleep_tool".into(),
            arguments: serde_json::json!({"delay_ms": 300}),
        },
        NativeToolCall {
            thought_signature: None,
            id: "p-fast".into(),
            name: "sleep_tool".into(),
            arguments: serde_json::json!({"delay_ms": 25}),
        },
    ];

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(DelayByArgTools),
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls.clone()),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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

    let mut callback = DoneOrderCallback::default();
    let state = harness
        .run_turn(&sample_session_id(), &mut callback)
        .await
        .expect("run_turn should succeed");
    assert_eq!(state, TurnState::Continue);

    // Live events fire in COMPLETION order: the fast call first…
    let done_ids: Vec<&str> = callback.done.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        done_ids,
        vec!["p-fast", "p-slow"],
        "live done events must fire in completion order, not input order",
    );
    // …and its duration is its own wall clock, not the slow sibling's wait.
    let fast_dur = callback.done[0].1;
    assert!(
        fast_dur < 250,
        "the fast call's live duration must not be inflated by head-of-line \
         waiting (got {fast_dur}ms; the slow sibling sleeps 300ms)",
    );

    // The transcript stays in INPUT order.
    let events = session.snapshot().await;
    let result_ids: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        result_ids,
        vec!["p-slow".to_string(), "p-fast".to_string()],
        "ToolResult session events must stay in input order",
    );
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
        llm: CapturingProvider::with_tool_calls("calling…", tool_calls),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
    // TurnState is must_use; this test measures wall-clock fallback timing,
    // not the returned loop-control signal.
    let _ = harness
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

// =============================================================================
// Cooperative steer checkpoint — deferred tool results
// =============================================================================

/// `emit_deferred_tool_results` must emit exactly one synthetic
/// `SessionEvent::ToolResult` per skipped call, in input order, carrying the
/// `"deferred": true` marker so the provider sees a result for every prior
/// `tool_use` block (otherwise the next request is rejected).
#[tokio::test]
async fn deferred_results_emit_one_tool_result_per_skipped_call() {
    let calls = vec![
        NativeToolCall {
            thought_signature: None,
            id: "call_a".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a"}),
        },
        NativeToolCall {
            thought_signature: None,
            id: "call_b".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "b"}),
        },
    ];

    let session = MockSession::new(vec![]);
    let tools = ScriptedTools::new(vec![]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools,
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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

    let sid = sample_session_id();
    let turn = uuid::Uuid::new_v4();

    harness
        .emit_deferred_tool_results(&sid, turn, &calls)
        .await
        .expect("emit_deferred_tool_results should succeed");

    let events = session.snapshot().await;
    let deferred: Vec<(String, serde_json::Value)> = events
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolResult {
                call_id, output, ..
            } => Some((call_id.clone(), output.value.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(deferred.len(), 2, "one ToolResult per skipped call");
    assert_eq!(deferred[0].0, "call_a", "first result is call_a");
    assert_eq!(deferred[1].0, "call_b", "second result is call_b");
    assert_eq!(
        deferred[0].1["deferred"],
        serde_json::json!(true),
        "first result carries the deferred marker",
    );
    assert_eq!(
        deferred[1].1["deferred"],
        serde_json::json!(true),
        "second result carries the deferred marker",
    );
}

// -- Cooperative steer checkpoint (serial loop) ------------------------------

/// Assistant message event for steer-checkpoint seeding (mirrors the
/// `agent.rs` follow-up tests' field set).
fn assistant_message_event(text: &str) -> SessionEvent {
    SessionEvent::AssistantMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        usage: None,
        at: now_ms(),
    }
}

/// Seed a session that, in order, holds: a non-synthetic user request, an
/// assistant "working on it" turn (so `has_unanswered_user_message`'s
/// assistant-turn guard is satisfied), and a non-synthetic mid-turn steer.
fn steer_seed() -> Vec<SessionEvent> {
    vec![
        user_message_event("original request"),
        assistant_message_event("working on it"),
        user_message_event("actually, stop and do X instead"),
    ]
}

fn read_call(id: &str, path: &str) -> NativeToolCall {
    NativeToolCall {
        thought_signature: None,
        id: id.into(),
        name: "read_file".into(),
        arguments: serde_json::json!({ "path": path }),
    }
}

#[tokio::test]
async fn serial_batch_defers_all_when_steer_present_before_first_call() {
    let session = MockSession::new(steer_seed());
    // No tools should run: a steer is already pending before the first call.
    let tools = ScriptedTools::new(vec![]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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

    // Watermark = seq 2 (the prompt covered the first two events); the steer
    // lands at seq 3, past the boundary, so `has_unanswered_user_message` fires.
    harness
        .last_prompt_seq
        .store(2, std::sync::atomic::Ordering::Relaxed);

    let sid = sample_session_id();
    let turn = uuid::Uuid::new_v4();
    let calls = vec![
        read_call("c1", "a"),
        read_call("c2", "b"),
        read_call("c3", "c"),
    ];

    let executed = harness
        .act(
            &sid,
            turn,
            calls,
            &mut NoopHarnessCallback,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("act should succeed");

    assert_eq!(executed, 0, "no tool runs when a steer precedes the batch");
    assert_eq!(tools.calls().await.len(), 0, "no tool was executed");

    let deferred = session
        .snapshot()
        .await
        .into_iter()
        .filter(|r| {
            matches!(
                &r.event,
                SessionEvent::ToolResult { output, .. } if output.value["deferred"] == serde_json::json!(true)
            )
        })
        .count();
    assert_eq!(deferred, 3, "every call gets a deferred ToolResult");
}

#[tokio::test]
async fn serial_batch_runs_full_when_no_midturn_steer() {
    // Seed only the original request + assistant turn — no mid-turn steer.
    let session = MockSession::new(vec![
        user_message_event("original request"),
        assistant_message_event("working on it"),
    ]);
    let tools = ScriptedTools::new(vec![
        Ok(ToolOutput {
            value: serde_json::json!("ok1"),
            metadata: Default::default(),
        }),
        Ok(ToolOutput {
            value: serde_json::json!("ok2"),
            metadata: Default::default(),
        }),
    ]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        .last_prompt_seq
        .store(2, std::sync::atomic::Ordering::Relaxed);

    let sid = sample_session_id();
    let turn = uuid::Uuid::new_v4();
    let calls = vec![read_call("c1", "a"), read_call("c2", "b")];

    let executed = harness
        .act(
            &sid,
            turn,
            calls,
            &mut NoopHarnessCallback,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("act should succeed");

    assert_eq!(executed, 2, "both tools run when no steer is pending");

    let deferred = session
        .snapshot()
        .await
        .into_iter()
        .filter(|r| {
            matches!(
                &r.event,
                SessionEvent::ToolResult { output, .. } if output.value["deferred"] == serde_json::json!(true)
            )
        })
        .count();
    assert_eq!(deferred, 0, "no deferred results without a mid-turn steer");
}

// -- Cooperative steer checkpoint (multi-group boundary) ---------------------

/// A steer present before the batch defers *every* call regardless of how the
/// batch partitions: the group-boundary checkpoint fires before launching the
/// first group, so `executed == 0` and all calls get a deferred ToolResult.
///
// NOTE: `ScriptedTools` does not override `call_concurrency_claim`, so with
// `concurrent_safe: true` every call yields a `ConcurrencyClaim::Shared`.
// `Shared` claims never conflict, so `partition_parallel_groups` collapses the
// batch into a *single* parallel group `[(0, 3)]`. That single group still has
// `end - start >= 2`, so `act` enters the multi-group loop (it does not route
// to the serial fast-path), exercising the new group-boundary branch — but it
// is NOT a real >= 2-group partition. The mocks cannot produce one: a
// per-call mix of `Shared`/`Global` would require an override of
// `call_concurrency_claim`, and `ScriptedTools` reports a uniform
// `concurrent_safe` for all calls. The group-boundary branch shares its
// predicate (`has_unanswered_user_message`) and helper
// (`emit_deferred_tool_results`) with the serial loop, so the multi-group
// defer behaviour for >= 2 groups is covered by code inspection.
#[tokio::test]
async fn group_boundary_defers_remaining_groups_when_steer_present() {
    let session = MockSession::new(steer_seed());
    // No tools should run: a steer is already pending before the first group.
    let tools = ScriptedTools::new_concurrent(vec![], std::time::Duration::ZERO);

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        parallel_tool_concurrency: Some(2),
    };
    let harness = AgentHarness::new(deps);

    // Watermark = seq 2 (the prompt covered the first two events); the steer
    // lands at seq 3, past the boundary, so `has_unanswered_user_message` fires.
    harness
        .last_prompt_seq
        .store(2, std::sync::atomic::Ordering::Relaxed);

    let sid = sample_session_id();
    let turn = uuid::Uuid::new_v4();
    let calls = vec![
        read_call("c1", "a"),
        read_call("c2", "b"),
        read_call("c3", "c"),
    ];

    let executed = harness
        .act(
            &sid,
            turn,
            calls,
            &mut NoopHarnessCallback,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("act should succeed");

    assert_eq!(executed, 0, "no group runs when a steer precedes the batch");
    assert_eq!(tools.calls().await.len(), 0, "no tool was executed");

    let deferred = session
        .snapshot()
        .await
        .into_iter()
        .filter(|r| {
            matches!(
                &r.event,
                SessionEvent::ToolResult { output, .. } if output.value["deferred"] == serde_json::json!(true)
            )
        })
        .count();
    assert_eq!(deferred, 3, "every call gets a deferred ToolResult");
}

// -- Per-tool budget resolution (production ToolService) ---------------------

/// A `LoopTool` that overruns any sane budget. It declares 50ms for itself —
/// the seam MCP tools use to publish their owning server's request timeout —
/// and is absent from the builtin budget table, so before the definition
/// builders learned to resolve a budget for every tool it advertised `None`.
struct SlowUnlistedTool;

#[async_trait]
impl crate::tools::runtime::LoopTool for SlowUnlistedTool {
    fn name(&self) -> &str {
        "slow_unlisted_tool"
    }
    fn description(&self) -> &str {
        "sleeps well past its budget"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> crate::tools::runtime::ToolResult {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        crate::tools::runtime::ToolResult::Success {
            output: serde_json::json!("never reached"),
        }
    }
    fn is_concurrent_safe(&self, _input: &serde_json::Value) -> bool {
        // Pin the call to the serial dispatch path so the assertion is about
        // the budget branch, not about which path a one-call batch takes.
        false
    }
    fn max_duration_ms(&self) -> Option<u64> {
        Some(50)
    }
}

/// A tool that overruns its wall-clock budget must come back as a recoverable
/// `ToolError` the next Think turn can react to — not as `HarnessError::StalledTurn`,
/// which kills the entire run.
///
/// Regression: the budget only ever reached the harness for the 19 builtins in
/// `tools::budget`'s table. Every other tool — every MCP tool, plugin, skill,
/// `ask_user`, `subagent` — described itself with `max_duration_ms: None`, and
/// `None` is exactly the branch that aborts the run. This drives the real
/// production `ToolService` (`ScopedToolService`) so the whole resolution chain
/// (`LoopTool::max_duration_ms` → builtin table → default) is under test.
#[tokio::test]
async fn overrunning_tool_yields_recoverable_error_not_stalled_turn() {
    let mut registry = crate::tools::runtime::LoopToolRegistry::new();
    registry.register(Box::new(SlowUnlistedTool));
    let tools: Arc<dyn ToolService> = Arc::new(crate::tools::ScopedToolService::new(
        Arc::new(registry),
        std::collections::BTreeSet::new(),
    ));

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools,
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        // The harness-wide fallback: the clock that used to kill the run for
        // every tool the budget table did not name.
        turn_timeout: Some(std::time::Duration::from_millis(300)),
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);

    let executed = harness
        .act(
            &sample_session_id(),
            uuid::Uuid::new_v4(),
            vec![NativeToolCall {
                thought_signature: None,
                id: "c1".into(),
                name: "slow_unlisted_tool".into(),
                arguments: serde_json::json!({}),
            }],
            &mut NoopHarnessCallback,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("a per-tool budget overrun must not abort the run");

    assert_eq!(executed, 0, "the call timed out, so nothing succeeded");

    let errors = session
        .snapshot()
        .await
        .into_iter()
        .filter(|r| matches!(&r.event, SessionEvent::ToolError { .. }))
        .count();
    assert_eq!(
        errors, 1,
        "the overrun must be persisted as a ToolError the model can react to"
    );
}

/// A `LoopTool` that declares nothing and is absent from the builtin budget
/// table — the shape of every MCP tool, plugin, skill, and the ~100 builtins
/// the table never named. It is slower than the harness-wide fallback but well
/// inside any sane per-tool budget.
struct UndeclaredSlowishTool;

#[async_trait]
impl crate::tools::runtime::LoopTool for UndeclaredSlowishTool {
    fn name(&self) -> &str {
        "undeclared_tool"
    }
    fn description(&self) -> &str {
        "slower than the harness fallback, faster than its own budget"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> crate::tools::runtime::ToolResult {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        crate::tools::runtime::ToolResult::Success {
            output: serde_json::json!("done"),
        }
    }
    fn is_concurrent_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }
}

/// The harness-wide `turn_timeout` must no longer be the clock a tool call is
/// judged by. It was, for every tool the budget table did not name: the call
/// was killed at the fallback (120s in production) and — because the budget
/// came back `None` — the kill aborted the whole run instead of erroring the
/// call. Here the fallback is 100ms and the tool takes 400ms; it must survive,
/// because it now resolves its own (default) budget.
#[tokio::test]
async fn undeclared_tool_is_not_judged_by_the_harness_turn_timeout() {
    let mut registry = crate::tools::runtime::LoopToolRegistry::new();
    registry.register(Box::new(UndeclaredSlowishTool));
    let tools: Arc<dyn ToolService> = Arc::new(crate::tools::ScopedToolService::new(
        Arc::new(registry),
        std::collections::BTreeSet::new(),
    ));

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools,
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: Some(std::time::Duration::from_millis(100)),
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);

    let executed = harness
        .act(
            &sample_session_id(),
            uuid::Uuid::new_v4(),
            vec![NativeToolCall {
                thought_signature: None,
                id: "c1".into(),
                name: "undeclared_tool".into(),
                arguments: serde_json::json!({}),
            }],
            &mut NoopHarnessCallback,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("an undeclared tool must not be stalled out by the turn_timeout");

    assert_eq!(
        executed, 1,
        "the call had time to finish under its own budget"
    );
}

// -- The Act-period clock sits BELOW the approval gate ------------------------

/// Deps around a real `ScopedToolService`. `turn_timeout` is deliberately set:
/// it must have no bearing on Act any more — its one legitimate home is
/// `think.rs::race_llm_call`, where no human stands in front of the future.
fn scoped_deps(session: Arc<MockSession>, tools: Arc<dyn ToolService>) -> HarnessDeps {
    HarnessDeps {
        session,
        tools,
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: Some(std::time::Duration::from_secs(120)),
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    }
}

fn one_call(id: &str, name: &str) -> Vec<NativeToolCall> {
    vec![NativeToolCall {
        thought_signature: None,
        id: id.into(),
        name: name.into(),
        arguments: serde_json::json!({}),
    }]
}

/// An operator who takes five seconds to read the command, then approves it.
struct SlowOperator;

#[async_trait]
impl crate::sandbox::exec_approval::gate::ApprovalRequester for SlowOperator {
    async fn request_approval(
        &self,
        _action: &crate::sandbox::exec_approval::ApprovalAction,
    ) -> crate::sandbox::exec_approval::gate::ApprovalResponse {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        crate::sandbox::exec_approval::gate::ApprovalOutcome::Approved.into()
    }
}

/// A confirm-gated tool with a 3s budget that runs for 1s once approved.
struct GatedTool;

#[async_trait]
impl crate::tools::runtime::LoopTool for GatedTool {
    fn name(&self) -> &str {
        "gated_tool"
    }
    fn description(&self) -> &str {
        "needs approval, then runs for a second"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> crate::tools::runtime::ToolResult {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        crate::tools::runtime::ToolResult::Success {
            output: serde_json::json!("ran"),
        }
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    fn max_duration_ms(&self) -> Option<u64> {
        Some(3_000)
    }
}

/// A command the operator explicitly APPROVED must not be killed for having been
/// read slowly.
///
/// Regression: `act.rs` wrapped `tokio::time::timeout(budget, …)` around the
/// ENTIRE execute future, and the human's wait is inside it —
/// `scoped/dispatch.rs` awaits `request_approval` before it ever routes to the
/// tool. So the operator's thinking time was spent out of the tool's execution
/// budget. Here the approval alone (5s) exceeds the tool's whole budget (3s),
/// yet the actual work takes 1s: the call must succeed. It also silently voided
/// a documented invariant — `CodeExecTool` clamps its foreground timeout to 170s
/// precisely so it sits 10s inside its 180s budget and can return a clean
/// exit-124 with partial output; any approval over 10s destroyed that.
///
/// (`start_paused` so the two sleeps are simulated, not slept.)
#[tokio::test(start_paused = true)]
async fn approved_command_survives_a_slow_operator() {
    let mut registry = crate::tools::runtime::LoopToolRegistry::new();
    registry.register(Box::new(GatedTool));
    let tools: Arc<dyn ToolService> = Arc::new(
        crate::tools::ScopedToolService::new(Arc::new(registry), std::collections::BTreeSet::new())
            .with_confirmation(Arc::new(SlowOperator)),
    );

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let harness = AgentHarness::new(scoped_deps(session.clone(), tools));

    let executed = harness
        .act(
            &sample_session_id(),
            uuid::Uuid::new_v4(),
            one_call("c1", "gated_tool"),
            &mut NoopHarnessCallback,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("an approved call must not abort the run");

    assert_eq!(
        executed, 1,
        "the operator approved it and the work fit the budget — it must have run",
    );
    let errors = session
        .snapshot()
        .await
        .into_iter()
        .filter(|r| matches!(&r.event, SessionEvent::ToolError { .. }))
        .count();
    assert_eq!(
        errors, 0,
        "an approved, in-budget call produces no ToolError"
    );
}

/// Times out on its first call (well past its declared 50ms), returns instantly
/// on every call after — the shape of a slow source that recovers.
struct FlakySlowTool {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl crate::tools::runtime::LoopTool for FlakySlowTool {
    fn name(&self) -> &str {
        "flaky_slow"
    }
    fn description(&self) -> &str {
        "slow once, then fine"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> crate::tools::runtime::ToolResult {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        crate::tools::runtime::ToolResult::Success {
            output: serde_json::json!("ok"),
        }
    }
    fn is_concurrent_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }
    fn max_duration_ms(&self) -> Option<u64> {
        Some(50)
    }
}

/// A wall-clock timeout must NOT be remembered as a permanent, cross-batch
/// failure — the identical call has to be allowed to run again next batch.
///
/// Regression: the timeout was synthesised as `ToolError::Execution`, whose own
/// text tells the model to "retry, narrow the query, or switch source/tool", and
/// the `Err(e)` arm then called `record_failure` unconditionally. So the very
/// next batch hit the cross-batch dedup preflight and was refused with
/// `CROSS_BATCH_REFUSED_CAUSE`: the harness wrote a hint it guaranteed it would
/// reject. It is now a `ToolError::Timeout` — the variant `is_retryable()` reads
/// — and only non-retryable failures enter the memo.
#[tokio::test(start_paused = true)]
async fn a_timed_out_call_may_be_retried_in_the_next_batch() {
    let mut registry = crate::tools::runtime::LoopToolRegistry::new();
    registry.register(Box::new(FlakySlowTool {
        calls: std::sync::atomic::AtomicUsize::new(0),
    }));
    let tools: Arc<dyn ToolService> = Arc::new(crate::tools::ScopedToolService::new(
        Arc::new(registry),
        std::collections::BTreeSet::new(),
    ));

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let harness = AgentHarness::new(scoped_deps(session.clone(), tools));
    let sid = sample_session_id();
    let cancel = tokio_util::sync::CancellationToken::new();

    // Batch 1: the call overruns its 50ms budget.
    let first = harness
        .act(
            &sid,
            uuid::Uuid::new_v4(),
            one_call("c1", "flaky_slow"),
            &mut NoopHarnessCallback,
            0,
            &cancel,
        )
        .await
        .expect("a budget overrun is recoverable");
    assert_eq!(first, 0, "the first call timed out");

    // Batch 2: the SAME (tool, args). The source has recovered; the harness must
    // let the model find that out.
    let second = harness
        .act(
            &sid,
            uuid::Uuid::new_v4(),
            one_call("c2", "flaky_slow"),
            &mut NoopHarnessCallback,
            1,
            &cancel,
        )
        .await
        .expect("the retry must not abort the run");
    assert_eq!(
        second, 1,
        "the identical repeat of a TIMED-OUT call must be re-run, not refused",
    );

    let events = session.snapshot().await;
    let refused = events.iter().any(|r| matches!(
        &r.event,
        SessionEvent::ToolError { error, .. } if error.contains("already failed earlier in the run")
    ));
    assert!(
        !refused,
        "cross-batch dedup must not ban a call whose only sin was being slow",
    );
    let timed_out = events.iter().any(|r| {
        matches!(
            &r.event,
            SessionEvent::ToolError { error, .. } if error.contains("timed out after")
        )
    });
    assert!(
        timed_out,
        "batch 1's overrun must surface as ToolError::Timeout"
    );
}

// =============================================================================
// Cancel checkpoint at the group boundary
// =============================================================================

/// After `/stop`, `act()`'s group loop must not keep walking. Before the
/// checkpoint every remaining group still logged a `ToolCallRequested`,
/// registered in-flight, dispatched (taking an instant cancellation error) and
/// logged a `ToolError` — phantom failures the next prompt reads as calls that
/// ran and failed, and `tool_summaries` counts as real. The pending `tool_use`
/// blocks still need results, so the checkpoint closes them out explicitly
/// instead of letting a doomed dispatch supply the pairing.
#[tokio::test]
async fn group_boundary_stops_and_closes_out_when_the_run_is_cancelled() {
    // No user message in the seed and `last_prompt_seq` left at 0, so
    // `has_unanswered_user_message` is false — the steer checkpoint cannot fire
    // and anything observed here belongs to the cancel checkpoint.
    let session = MockSession::new(vec![turn_started_event()]);
    let tools = ScriptedTools::new_concurrent(
        vec![
            Ok(ok_output(serde_json::json!({"ok": 1}))),
            Ok(ok_output(serde_json::json!({"ok": 2}))),
            Ok(ok_output(serde_json::json!({"ok": 3}))),
        ],
        std::time::Duration::ZERO,
    );

    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        parallel_tool_concurrency: Some(2),
    };
    let harness = AgentHarness::new(deps);

    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();

    let executed = harness
        .act(
            &sample_session_id(),
            uuid::Uuid::new_v4(),
            vec![
                read_call("k1", "a"),
                read_call("k2", "b"),
                read_call("k3", "c"),
            ],
            &mut NoopHarnessCallback,
            0,
            &cancel,
        )
        .await
        .expect("act should return cleanly on a cancelled run");

    assert_eq!(executed, 0, "a cancelled run executes nothing");
    assert_eq!(
        tools.calls().await.len(),
        0,
        "no call may be dispatched after the run is cancelled",
    );

    let events = session.snapshot().await;
    let requested = events
        .iter()
        .filter(|r| matches!(&r.event, SessionEvent::ToolCallRequested { .. }))
        .count();
    assert_eq!(
        requested, 0,
        "a cancelled run must not log tool calls it never made",
    );

    let closed: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolError { call_id, error, .. } if error.contains("cancelled") => {
                Some(call_id.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        closed,
        vec!["k1".to_string(), "k2".to_string(), "k3".to_string()],
        "every pending tool_use block is closed out so the pairing survives",
    );
}

/// A call that queues behind the concurrency cap must be timed from its own
/// first poll, not from batch admission. With `parallel_tool_concurrency = 2`
/// and three calls, the third cannot start until a slot frees — timing it from
/// PASS 0 bills it for that wait, which is exactly what the completion-order
/// loop's comment promises it does not do.
#[tokio::test]
async fn queued_parallel_call_is_timed_from_its_own_start_not_admission() {
    let session = MockSession::new(vec![turn_started_event()]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(DelayByArgTools),
        llm: CapturingProvider::text_only("idle"),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        parallel_tool_concurrency: Some(2),
    };
    let harness = AgentHarness::new(deps);

    let sleep_call = |id: &str, ms: u64| NativeToolCall {
        thought_signature: None,
        id: id.into(),
        name: "sleep_tool".into(),
        arguments: serde_json::json!({ "delay_ms": ms }),
    };

    // Distinct delays keep the three `(name, args)` signatures distinct: two
    // identical calls would trip the within-batch duplicate check, collapse to
    // one whole-batch group and route through the SERIAL loop, where the clock
    // is already correct — the test would pass without exercising anything.
    let mut callback = DoneOrderCallback::default();
    let started = std::time::Instant::now();
    harness
        .act(
            &sample_session_id(),
            uuid::Uuid::new_v4(),
            vec![
                sleep_call("q-hold-a", 200),
                sleep_call("q-hold-b", 210),
                sleep_call("q-queued", 60),
            ],
            &mut callback,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("act should succeed");
    let wall = started.elapsed();

    // Guard against passing for the wrong reason: serially this batch is
    // 200+210+60 = 470ms, and a serial run times every call correctly.
    assert!(
        wall < std::time::Duration::from_millis(400),
        "the batch must actually run in parallel for this test to mean \
         anything (took {wall:?})",
    );

    let queued = callback
        .done
        .iter()
        .find(|(id, _)| id == "q-queued")
        .expect("the queued call reports a done event");
    assert!(
        queued.1 < 150,
        "the queued call slept 60ms but waited ~200ms for a slot; its duration \
         must be its own work (got {}ms)",
        queued.1,
    );
}
