//! Tests for `AgentHarness::run_turn` — Think phase (Task 8).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::error::{AlephError, Result as AlephResult};
use crate::harness::{
    AgentHarness, Harness, HarnessDeps, HarnessError, NoopHarnessCallback, TurnState,
};
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

    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
        std::sync::Arc::from([])
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
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
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
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
    };
    let harness = AgentHarness::new(deps);

    let err = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect_err("run_turn should propagate LLM error");

    assert!(matches!(err, HarnessError::Llm(_)), "got: {err:?}");
}

/// With Task 9's Act phase landed, a tool_call response drives a full Act
/// pass; `Continue` is only returned after the tool executes successfully.
/// The detailed Act-side assertions live in `harness::tests::act` — this
/// test keeps a minimal Think-level sanity check on the Continue path.
/// Task 1 contract: the `HarnessCallback` fires `on_delta` for assistant text
/// and `on_tool_call` before each tool dispatch, within a single `run_turn`.
/// Covers both text-only Done turns and tool_use Continue turns.
#[tokio::test]
async fn callback_fires_on_delta_and_tool_call() {
    use crate::harness::HarnessCallback;
    use crate::session::events::ToolOutput;

    #[derive(Default)]
    struct CapturingCallback {
        deltas: Vec<String>,
        tools: Vec<String>,
    }

    impl HarnessCallback for CapturingCallback {
        fn on_delta(&mut self, text: &str) {
            self.deltas.push(text.to_string());
        }
        fn on_tool_call(&mut self, name: &str) {
            self.tools.push(name.to_string());
        }
        fn on_complete(&mut self) {}
    }

    struct OkTool;
    #[async_trait]
    impl ToolService for OkTool {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
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

    // Turn with one tool_call: expect one on_delta("calling…") + on_tool_call("echo").
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(OkTool),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: FixedProvider::with_tool_call("calling…", "echo"),
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
    };
    let harness = AgentHarness::new(deps);
    let mut cb = CapturingCallback::default();

    let state = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn should succeed");

    assert_eq!(state, TurnState::Continue);
    assert_eq!(
        cb.deltas,
        vec!["calling…".to_string()],
        "on_delta should fire once per assistant turn with full text"
    );
    assert_eq!(
        cb.tools,
        vec!["echo".to_string()],
        "on_tool_call should fire once per tool dispatch"
    );
}

#[tokio::test]
async fn run_returns_cancelled_when_token_is_pre_cancelled() {
    use tokio_util::sync::CancellationToken;

    // LLM that would panic if called — proves run() never entered Think.
    struct PanicProvider;
    impl AiProvider for PanicProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move { panic!("LLM must not be called after cancel") })
        }
        fn name(&self) -> &str {
            "panic"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    let session = MockSession::new(vec![turn_started_event(), user_message_event("hi")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: Arc::new(PanicProvider),
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
    };
    let harness = AgentHarness::new(deps);

    let cancel = CancellationToken::new();
    cancel.cancel();

    let err = harness
        .run(&sample_session_id(), &mut NoopHarnessCallback, &cancel)
        .await
        .expect_err("pre-cancelled run should error");

    assert!(
        matches!(err, HarnessError::Cancelled),
        "expected Cancelled, got {err:?}",
    );
}

#[tokio::test]
async fn think_tool_use_after_act_returns_continue() {
    use crate::session::events::ToolOutput;

    // Tool service that succeeds once for the single expected call.
    struct OkOnceTool;
    #[async_trait]
    impl ToolService for OkOnceTool {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
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

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(OkOnceTool),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: FixedProvider::with_tool_call("calling…", "echo"),
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
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
}
