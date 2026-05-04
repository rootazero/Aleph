//! Stability rescue test suite — covers TraceSink wiring, act() error
//! rescue, per-turn timeout, and StallTracker dispersion.

#![allow(dead_code)] // helpers grow as tasks land

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::error::Result as AlephResult;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::trace_sink::TraceSink;
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, ToolOutput, ToolOutputMetadata, TurnTrigger,
};
use crate::session::in_process::InProcessActorSessionService;
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

/// Captures every `LoopTraceEvent` for assertion.
pub(super) struct RecordingTraceSink {
    pub(super) events: Arc<Mutex<Vec<LoopTraceEvent>>>,
}

impl RecordingTraceSink {
    pub(super) fn new() -> (Arc<Self>, Arc<Mutex<Vec<LoopTraceEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Self {
            events: events.clone(),
        });
        (sink, events)
    }
}

impl TraceSink for RecordingTraceSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
    fn flush(&self) {}
}

/// Provider whose `process` future never resolves. Used for timeout tests.
pub(super) struct HangingProvider;

impl AiProvider for HangingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
    fn name(&self) -> &str {
        "hanging"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider that returns one tool_call (`name`) once, then text-only "done".
pub(super) struct OneShotToolProvider {
    pub(super) name: String,
    pub(super) calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl AiProvider for OneShotToolProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let calls = self.calls.clone();
        let tool = self.name.clone();
        Box::pin(async move {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: format!("c-{n}"),
                        name: tool,
                        arguments: serde_json::json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                })
            } else {
                Ok(ProviderResponse::text_only("done".into()))
            }
        })
    }
    fn name(&self) -> &str {
        "oneshot"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Tool service that always returns `Err(ToolError::Other(...))`.
pub(super) struct AlwaysFailTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for AlwaysFailTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        Err(crate::tools::service::ToolError::Other(format!(
            "forced fail for {name}"
        )))
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
}

/// Tool service that succeeds for tools whose name starts with "ok_" and
/// fails for tools whose name starts with "fail_".
pub(super) struct MixedTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for MixedTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        if name.starts_with("fail_") {
            Err(crate::tools::service::ToolError::Other(format!(
                "mixed tool {name} forced fail"
            )))
        } else {
            Ok(ToolOutput {
                value: serde_json::json!({"name": name}),
                metadata: ToolOutputMetadata::default(),
            })
        }
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
}

/// Tool service whose `execute` blocks forever (for act-phase timeout tests).
pub(super) struct HangingTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for HangingTools {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        std::future::pending().await
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
}

/// Build a fresh attached session with one `TurnStarted` + `UserMessage`
/// pair so `harness.run` has work on first call.
pub(super) async fn fresh_session(
    tag: &str,
) -> (
    Arc<dyn crate::session::service::SessionService>,
    crate::session::service::SessionId,
) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session: Arc<dyn crate::session::service::SessionService> =
        Arc::new(InProcessActorSessionService::new(store));

    let sid = SessionKey::ephemeral(tag);
    session.attach(sid.clone()).await.unwrap();
    let turn = uuid::Uuid::new_v4();
    session
        .emit_event(
            &sid,
            SessionEvent::TurnStarted {
                turn_id: turn,
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
                turn_id: turn,
                content: MessageContent {
                    text: "go".into(),
                    blocks: vec![],
                },
                at: now_ms(),
            },
        )
        .await
        .unwrap();
    (session, sid)
}

/// Minimal `HarnessDeps` builder used by stability tests. All `Option` fields
/// default to `None`. Trace sink is `None` unless the test injects one.
///
/// Tests that need a different LLM/tool/sandbox set construct deps directly.
pub(super) fn minimal_deps(
    session: Arc<dyn crate::session::service::SessionService>,
    tools: Arc<dyn crate::tools::service::ToolService>,
    llm: Arc<dyn AiProvider>,
) -> HarnessDeps {
    HarnessDeps {
        session,
        tools,
        sandbox: Arc::new(crate::sandbox::NoopSandbox),
        llm,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        // Will be filled in by Tasks 2 and 3:
        // consecutive_failure_cap: None,
        // turn_timeout: None,
    }
}
