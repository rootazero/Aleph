//! Tests for `AgentHarness::run_turn` — Think phase (Task 8).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::error::{AlephError, Result as AlephResult};
use crate::harness::{AgentHarness, Harness, HarnessDeps, HarnessError, TurnState};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sandbox::test_util::MockSandbox;
use crate::sandbox::SandboxOutput;
use crate::session::events::{
    now_ms, EventSeq, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
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
}

// -- Mock AiProvider ---------------------------------------------------------

struct FixedProvider {
    response: ProviderResponse,
}

impl FixedProvider {
    fn text_only(text: &str) -> Arc<Self> {
        Arc::new(Self {
            response: ProviderResponse::text_only(text.to_string()),
        })
    }

    fn with_tool_call(text: &str, tool_name: &str) -> Arc<Self> {
        let response = ProviderResponse {
            text: Some(text.to_string()),
            tool_calls: vec![NativeToolCall {
                id: "call-1".to_string(),
                name: tool_name.to_string(),
                arguments: serde_json::json!({}),
            }],
            ..Default::default()
        };
        Arc::new(Self { response })
    }
}

impl AiProvider for FixedProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let response = self.response.clone();
        Box::pin(async move { Ok(response) })
    }

    fn name(&self) -> &str {
        "fixed"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

struct ErrProvider;

impl AiProvider for ErrProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { Err(AlephError::provider("simulated")) })
    }

    fn name(&self) -> &str {
        "err"
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

fn user_message_event(text: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
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
async fn think_with_no_tool_use_returns_done() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: FixedProvider::text_only("hi"),
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id())
        .await
        .expect("run_turn should succeed");

    assert_eq!(state, TurnState::Done);

    let events = session.snapshot().await;
    let assistant_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count();
    assert_eq!(
        assistant_count, 1,
        "exactly one AssistantMessage should be emitted"
    );

    let assistant = events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::AssistantMessage { content, .. } => Some(content.text.clone()),
            _ => None,
        })
        .expect("AssistantMessage present");
    assert_eq!(assistant, "hi");
}

#[tokio::test]
async fn think_llm_error_maps_to_harness_llm() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: Arc::new(ErrProvider),
    };
    let harness = AgentHarness::new(deps);

    let err = harness
        .run_turn(&sample_session_id())
        .await
        .expect_err("run_turn should propagate LLM error");

    assert!(matches!(err, HarnessError::Llm(_)), "got: {err:?}");
}

#[tokio::test]
async fn think_with_tool_use_returns_continue() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: FixedProvider::with_tool_call("calling…", "echo"),
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id())
        .await
        .expect("run_turn should succeed");

    // Until Task 9 lands, a tool_call response short-circuits to Continue
    // without any actual tool execution.
    assert_eq!(state, TurnState::Continue);
}
