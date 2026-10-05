//! Tests for `AgentHarness::run_turn` — Think phase (Task 8).

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::error::{AlephError, Result as AlephResult};
use crate::harness::tests::harness_ext::AgentHarnessTestExt;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::{AgentHarness, HarnessDeps, HarnessError, NoopHarnessCallback, TurnState};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
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
                thought_signature: None,
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
async fn think_with_no_tool_use_returns_done() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: FixedProvider::text_only("hi"),
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

/// One Think iteration with a thinking block records exactly one
/// `ReasoningEmitted` beside its `TextEmitted{Final}`, carrying the same
/// iteration and the whole block. This is the record `trace.by_runs`
/// replays a folded step from.
#[tokio::test]
async fn a_think_turn_with_thinking_emits_one_reasoning_record() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let (sink, recorded) = super::stability::RecordingTraceSink::new();
    let provider = Arc::new(FixedProvider {
        response: ProviderResponse {
            text: Some("hi".into()),
            thinking: Some("Weighing the two readings of the question.".into()),
            ..Default::default()
        },
    });
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: provider,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: Some(sink as Arc<dyn crate::harness::TraceSink>),
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
    let harness = AgentHarness::new(deps);
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");
    assert_eq!(state, TurnState::Done);

    let events = recorded.lock().unwrap_or_else(|e| e.into_inner());
    let reasoning: Vec<(usize, String)> = events
        .iter()
        .filter_map(|e| match e {
            LoopTraceEvent::ReasoningEmitted { iteration, text } => {
                Some((*iteration, text.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        reasoning,
        vec![(1, "Weighing the two readings of the question.".to_string())],
        "exactly one reasoning record, on iteration 1: {events:?}"
    );
    let text_iteration = events.iter().find_map(|e| match e {
        LoopTraceEvent::TextEmitted { iteration, .. } => Some(*iteration),
        _ => None,
    });
    assert_eq!(
        text_iteration,
        Some(1),
        "the text record shares the iteration"
    );
}

/// A blank thinking block — whitespace only, which also covers `Some("")` —
/// must not produce a record that would later render as "Thought for 0s".
/// Same notion of blank as `is_empty_response` (`trim()`).
#[tokio::test]
async fn an_empty_thinking_block_emits_no_reasoning_event() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let (sink, recorded) = super::stability::RecordingTraceSink::new();
    let provider = Arc::new(FixedProvider {
        response: ProviderResponse {
            text: Some("hi".into()),
            thinking: Some(" \n\n ".into()),
            ..Default::default()
        },
    });
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: provider,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: Some(sink as Arc<dyn crate::harness::TraceSink>),
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
    let harness = AgentHarness::new(deps);
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");
    assert_eq!(state, TurnState::Done);

    let events = recorded.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::ReasoningEmitted { .. })),
        "an empty thinking block must not be recorded: {events:?}"
    );
}

/// A tool-only turn (tool calls, no text) still records its thinking: the
/// dominant step shape the fold is built for. Red if the emit moves inside
/// the `if !text.is_empty()` block that guards `TextEmitted{Final}`.
#[tokio::test]
async fn a_tool_only_turn_with_thinking_still_records_its_reasoning() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let (sink, recorded) = super::stability::RecordingTraceSink::new();
    let provider = Arc::new(FixedProvider {
        response: ProviderResponse {
            text: None,
            thinking: Some("List the directory before editing.".into()),
            tool_calls: vec![NativeToolCall {
                thought_signature: None,
                id: "call-1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({}),
            }],
            ..Default::default()
        },
    });
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: provider,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: Some(sink as Arc<dyn crate::harness::TraceSink>),
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
    let harness = AgentHarness::new(deps);
    let _state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    let events = recorded.lock().unwrap_or_else(|e| e.into_inner());
    // Precondition: the turn really had no text, so no TextEmitted fired.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::TextEmitted { .. })),
        "the fixture must be a tool-only turn: {events:?}"
    );
    let reasoning: Vec<(usize, String)> = events
        .iter()
        .filter_map(|e| match e {
            LoopTraceEvent::ReasoningEmitted { iteration, text } => {
                Some((*iteration, text.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        reasoning,
        vec![(1, "List the directory before editing.".to_string())],
        "a tool-only turn records exactly one reasoning record, on iteration 1: {events:?}"
    );
}

#[tokio::test]
async fn think_llm_error_maps_to_harness_llm() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: Arc::new(ErrProvider),
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
    let harness = AgentHarness::new(deps);

    let err = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect_err("run_turn should propagate LLM error");

    assert!(matches!(err, HarnessError::Llm(_)), "got: {err:?}");
}

/// A transient primary-provider error surfaces as `HarnessError::Llm` — the
/// harness propagates it for the orchestrator to handle. Provider-tier
/// failover, when configured, lives inside `deps.llm` (`FailoverProvider`),
/// not in the harness loop (R10).
#[tokio::test]
async fn primary_transient_error_without_fallback_still_propagates() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: Arc::new(ErrProvider),
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
    let harness = AgentHarness::new(deps);
    let err = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect_err("primary error without fallback must propagate");
    assert!(matches!(err, HarnessError::Llm(_)), "got {err:?}");
}

/// With Task 9's Act phase landed, a tool_call response drives a full Act
/// pass; `Continue` is only returned after the tool executes successfully.
/// The detailed Act-side assertions live in `harness::tests::act` — this
/// test keeps a minimal Think-level sanity check on the Continue path.
/// Task 1 contract: the `HarnessCallback` fires `on_delta` for assistant text
/// and `on_tool_call_start` before each tool dispatch, within a single `run_turn`.
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
        fn on_tool_call_start(&mut self, _id: &str, name: &str, _args: &serde_json::Value) {
            self.tools.push(name.to_string());
        }
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
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    // Turn with one tool_call: expect one on_delta("calling…") + on_tool_call_start("echo").
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(OkTool),
        llm: FixedProvider::with_tool_call("calling…", "echo"),
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
        "on_tool_call_start should fire once per tool dispatch"
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
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: Arc::new(PanicProvider),
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
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    let session = MockSession::new(vec![turn_started_event(), user_message_event("do it")]);
    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools: Arc::new(OkOnceTool),
        llm: FixedProvider::with_tool_call("calling…", "echo"),
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
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    assert_eq!(state, TurnState::Continue);
}

// -- Growth-step fold nudge (Context Fabric, spec 2026-10-01 §1d, Task 4) ----
//
// The nudge is scheduled by `GrowthNudgeTracker` (run-scoped bookkeeping on
// `AgentHarness`) and rendered by `nudges::compact_growth_nudge`; think.rs
// appends it to the transient tail. These tests pin the scheduling contract
// and the transience guarantee — no harness run is needed because the tracker
// is the whole decision.

use crate::harness::agent::think::GrowthNudgeTracker;
use crate::providers::message::UnifiedMessage;
use crate::thinker::nudges::{
    compact_growth_nudge, is_synthetic_reminder, FOLD_NUDGE_GROWTH_TOKENS,
};

/// Crossing the growth threshold since the baseline fires the nudge exactly
/// once: firing rebases, so the same prompt size does not nudge twice.
#[test]
fn nudge_fires_at_growth_threshold() {
    let mut tracker = GrowthNudgeTracker::new();
    // First observation of a run only sets the baseline.
    assert!(tracker
        .consider(10_000, 0, false, FOLD_NUDGE_GROWTH_TOKENS)
        .is_none());
    // Below the threshold: silence.
    assert!(tracker
        .consider(
            10_000 + FOLD_NUDGE_GROWTH_TOKENS - 1,
            0,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_none());
    // At the threshold: fire, and the copy must point at the tool and teach
    // what the summary instructions must preserve.
    let nudge = tracker
        .consider(
            10_000 + FOLD_NUDGE_GROWTH_TOKENS,
            0,
            false,
            FOLD_NUDGE_GROWTH_TOKENS,
        )
        .expect("growth at the threshold earns the nudge");
    assert!(nudge.contains("session_compact"), "{nudge}");
    assert!(nudge.contains("instructions"), "{nudge}");
    assert!(
        is_synthetic_reminder(&nudge),
        "harness scaffolding, so the cache layer never breakpoints it"
    );
    // Firing rebased the accounting: no repeat at the same size.
    assert!(tracker
        .consider(
            10_000 + FOLD_NUDGE_GROWTH_TOKENS,
            0,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_none());
}

/// While the compaction circuit breaker is counting ineffective compactions,
/// the nudge is suppressed — urging the model to compact right after
/// compaction proved useless is nagging. Suppression must NOT rebase: the
/// accumulated growth still earns the nudge once the breaker clears.
#[test]
fn nudge_suppressed_when_breaker_tripped() {
    let mut tracker = GrowthNudgeTracker::new();
    assert!(tracker
        .consider(10_000, 0, false, FOLD_NUDGE_GROWTH_TOKENS)
        .is_none());
    assert!(tracker
        .consider(
            10_000 + FOLD_NUDGE_GROWTH_TOKENS,
            0,
            true,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_none());
    assert!(tracker
        .consider(
            10_000 + FOLD_NUDGE_GROWTH_TOKENS + 1,
            0,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_some());
}

/// A fold rebases the accounting even when the tracker did not fire: growth
/// is measured from the last fold (or run start), so a post-fold prompt needs
/// a full new threshold of growth before the next nudge.
#[test]
fn nudge_resets_after_fold() {
    let mut tracker = GrowthNudgeTracker::new();
    assert!(tracker
        .consider(10_000, 0, false, FOLD_NUDGE_GROWTH_TOKENS)
        .is_none());
    assert!(tracker
        .consider(
            10_000 + FOLD_NUDGE_GROWTH_TOKENS,
            0,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_some());
    // A fold lands (the marker is the latest fold's to_seq); the post-fold
    // prompt is small again. Observing the new marker rebases silently.
    assert!(tracker
        .consider(12_000, 40, false, FOLD_NUDGE_GROWTH_TOKENS)
        .is_none());
    assert!(tracker
        .consider(
            12_000 + FOLD_NUDGE_GROWTH_TOKENS - 1,
            40,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_none());
    assert!(tracker
        .consider(
            12_000 + FOLD_NUDGE_GROWTH_TOKENS,
            40,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_some());
}

/// A custom (smaller) threshold from
/// `ContextBudgetConfig::fold_nudge_growth_tokens` fires proportionally
/// earlier: the tracker measures growth against the caller-supplied
/// threshold, and the copy reports that threshold — never the module
/// constant (R9: configurability exposed, spec O2 overruled 2026-10-02).
#[test]
fn nudge_custom_threshold_fires_earlier() {
    let mut tracker = GrowthNudgeTracker::new();
    const CUSTOM: u64 = 5_000;
    assert!(tracker.consider(10_000, 0, false, CUSTOM).is_none());
    assert!(tracker
        .consider(10_000 + CUSTOM - 1, 0, false, CUSTOM)
        .is_none());
    let nudge = tracker
        .consider(10_000 + CUSTOM, 0, false, CUSTOM)
        .expect("the custom step earns the nudge at its own threshold");
    assert!(nudge.contains("session_compact"), "{nudge}");
    assert!(nudge.contains("nudge threshold: 5000"), "{nudge}");
}

/// Cross-run resume: a run whose session already folded seeds the baseline
/// from the estimated post-fold size, so growth earned by earlier runs
/// counts toward THIS run's first nudge instead of being silently forgiven
/// by a fresh baseline. (The estimate errs high, so any inaccuracy delays
/// the first nudge rather than firing it early.)
#[test]
fn nudge_cross_run_seed_continues_growth() {
    let mut tracker = GrowthNudgeTracker::new();
    // The resumed run's estimate: post-fold prompt was 20k; the fold marker
    // is that fold's to_seq.
    tracker.seed_cross_run(20_000, 40);
    // Below the threshold counting inherited growth: silence. Crucially the
    // FIRST observation does not re-baseline — that was the old per-run bug.
    assert!(tracker
        .consider(
            20_000 + FOLD_NUDGE_GROWTH_TOKENS - 1,
            40,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_none());
    // At the threshold: fire on this run's first observation.
    let nudge = tracker
        .consider(
            20_000 + FOLD_NUDGE_GROWTH_TOKENS,
            40,
            false,
            FOLD_NUDGE_GROWTH_TOKENS,
        )
        .expect("inherited growth earns the nudge without a re-baseline");
    assert!(nudge.contains("session_compact"), "{nudge}");
}

/// The seed is one-shot: once any baseline exists (first observation or a
/// prior seed), `seed_cross_run` is a no-op — the in-run ledger is never
/// re-seeded by stale log state.
#[test]
fn nudge_cross_run_seed_is_one_shot() {
    let mut tracker = GrowthNudgeTracker::new();
    assert!(tracker
        .consider(10_000, 0, false, FOLD_NUDGE_GROWTH_TOKENS)
        .is_none());
    // Baseline exists now; a stale seed attempt must not move it.
    tracker.seed_cross_run(999_999, 0);
    assert!(tracker
        .consider(
            10_000 + FOLD_NUDGE_GROWTH_TOKENS,
            0,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_some());
}

/// Seeded ledger, then a fold lands in-run: the marker change rebases onto
/// the post-fold prompt (the `consider` path, not a second seed), and the
/// next nudge needs a full new growth step.
#[test]
fn nudge_cross_run_seed_then_inrun_fold_rebases() {
    let mut tracker = GrowthNudgeTracker::new();
    tracker.seed_cross_run(20_000, 40);
    // A fold lands (new marker 90); the post-fold prompt is 12k — below the
    // seed, yet observing the new marker rebases silently.
    assert!(tracker
        .consider(12_000, 90, false, FOLD_NUDGE_GROWTH_TOKENS)
        .is_none());
    assert!(tracker
        .consider(
            12_000 + FOLD_NUDGE_GROWTH_TOKENS,
            90,
            false,
            FOLD_NUDGE_GROWTH_TOKENS
        )
        .is_some());
}

/// The nudge rides the transient tail: it is pushed onto the in-memory
/// message vector AFTER `build_prompt_with_transient_tail` returns, so the
/// session log never sees it and the persisted prefix of the next prompt is
/// byte-identical whether or not a nudge fired (Review Focus #4).
#[test]
fn nudge_is_transient_not_persisted() {
    let records = vec![
        SessionEventRecord {
            seq: 1,
            event: user_message_event("migrate the vector store"),
            created_at_ms: now_ms(),
        },
        SessionEventRecord {
            seq: 2,
            event: user_message_event("proceed"),
            created_at_ms: now_ms(),
        },
    ];

    let (mut messages, mut transient_tail) =
        crate::harness::agent::prompt::build_prompt_with_transient_tail(&records, 0);
    let persisted_len = messages.len() - transient_tail;

    // The injection path, mirrored exactly as think.rs performs it.
    let nudge = compact_growth_nudge(FOLD_NUDGE_GROWTH_TOKENS + 2_000, FOLD_NUDGE_GROWTH_TOKENS);
    messages.push(UnifiedMessage::user(&nudge));
    transient_tail += 1;

    // (a) Nothing reached the session log: the events are the same records,
    //     and no serialization of them carries the nudge text.
    let log_json = serde_json::to_string(&records).unwrap();
    assert!(
        !log_json.contains(&nudge),
        "a persisted nudge would replay as a user turn forever"
    );

    // (b) The nudge lives strictly inside the transient tail: the persisted
    //     prefix holds only what the log produced.
    assert_eq!(messages.len() - transient_tail, persisted_len);
    let tail = &messages[persisted_len..];
    assert_eq!(tail.len(), 1);
    let tail_text = format!("{:?}", tail[0]);
    assert!(tail_text.contains("session_compact"), "{tail_text}");

    // (c) Rebuilding from the same log next turn reproduces the identical
    //     persisted prefix — the fired nudge leaves no residue.
    let (rebuilt, _) = crate::harness::agent::prompt::build_prompt_with_transient_tail(&records, 0);
    assert_eq!(
        serde_json::to_string(&rebuilt).unwrap(),
        serde_json::to_string(&messages[..persisted_len]).unwrap(),
        "the persisted prefix must be byte-identical with or without a fired nudge"
    );
}
