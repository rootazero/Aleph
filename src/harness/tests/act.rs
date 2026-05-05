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
}

impl ScriptedTools {
    fn new(outcomes: Vec<Result<ToolOutput, ToolError>>) -> Arc<Self> {
        Arc::new(Self {
            log: Mutex::new(Vec::new()),
            outcomes: Mutex::new(outcomes),
        })
    }

    async fn calls(&self) -> Vec<(String, serde_json::Value)> {
        self.log.lock().await.clone()
    }
}

#[async_trait]
impl ToolService for ScriptedTools {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        self.log
            .lock()
            .await
            .push((name.to_string(), input.clone()));
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

    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
        std::sync::Arc::from([])
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
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: Some(0),
        signal: None,
        truncated: false,
        duration_ms: 0,
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
            id: "c1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a.txt"}),
        },
        NativeToolCall {
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
        skill_prefetcher: None,
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
        turn_timeout: None,
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
        skill_prefetcher: None,
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
        turn_timeout: None,
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
        skill_prefetcher: None,
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
        turn_timeout: None,
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

    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
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
        skill_prefetcher: None,
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
        turn_timeout: None,
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
        },
        NativeToolCall {
            id: "c2".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"cmd": "ls"}),
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
            } => {
                assert_eq!(id, calls[i].id, "id must round-trip for block {i}");
                assert_eq!(name, calls[i].name, "name must round-trip for block {i}");
                assert_eq!(
                    arguments, calls[i].arguments,
                    "arguments must round-trip for block {i}"
                );
            }
            other => panic!("expected ContentBlock::ToolCall, got {other:?}"),
        }
    }
}
