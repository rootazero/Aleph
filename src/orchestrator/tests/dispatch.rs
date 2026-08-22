use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::orchestrator::dispatch::{FlowRequest, Orchestrator};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::{FlowRegistry, FlowSet};
use crate::orchestrator::flow_spec::{
    BrainRef, FlowInput, FlowOverrides, FlowSpec, SessionStrategy,
};
use crate::orchestrator::sandbox_factory::build_sandbox_factory;
use crate::sandbox::Sandbox;

struct MockHarness {
    outcome: crate::orchestrator::dispatch::FlowOutcome,
    invocations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl crate::orchestrator::dispatch::HarnessRunner for MockHarness {
    async fn run(
        &self,
        session_key: String,
        _spec: Arc<FlowSpec>,
        _input: FlowInput,
        _sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<crate::orchestrator::dispatch::FlowStreamEvent>,
        _cancel: CancellationToken,
        _tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
        _trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
        _interaction_manifest: Option<crate::thinker::InteractionManifest>,
        _workspace_override: Option<std::path::PathBuf>,
        _max_iterations_override: Option<u32>,
        _transient_context: Option<String>,
        _think_level: Option<crate::agents::thinking::ThinkLevel>,
        _envelope: crate::thinker::TurnEnvelope,
        _turn_model: Option<crate::providers::session_model_handle::SessionModelPref>,
    ) -> Result<crate::orchestrator::dispatch::FlowOutcome, FlowError> {
        self.invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_key);
        let _ = events.send(crate::orchestrator::dispatch::FlowStreamEvent::Delta(
            "hi".into(),
        ));
        let _ = events.send(crate::orchestrator::dispatch::FlowStreamEvent::Complete(
            // rust-doctor-disable-next-line excessive-clone
            self.outcome.clone(),
        ));
        // rust-doctor-disable-next-line excessive-clone
        Ok(self.outcome.clone())
    }
}

fn fake_session_service() -> Arc<dyn crate::session::service::SessionService> {
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

    // rust-doctor-disable-next-line unwrap-in-production
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

fn fixture_orchestrator() -> (Orchestrator, Arc<Mutex<Vec<String>>>) {
    let mut spec_map = FlowSet::new();
    let spec = FlowSpec {
        id: "default-agent".into(),
        description: "t".into(),
        agent: "main".into(),
        brain: BrainRef::Default,
        session_strategy: SessionStrategy::Fresh,
        overrides: FlowOverrides::default(),
    };
    spec_map.insert("default-agent".into(), Arc::new(spec));
    let registry = Arc::new(FlowRegistry::new(spec_map));

    let mut defaults = std::collections::HashMap::new();
    defaults.insert("main".into(), "default-agent".into());

    let session_service = fake_session_service();

    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(crate::sandbox::NoopSandbox) as Arc<dyn Sandbox>)
    }));

    let invocations = Arc::new(Mutex::new(Vec::<String>::new()));
    let harness = Arc::new(MockHarness {
        outcome: crate::orchestrator::dispatch::FlowOutcome {
            final_text: "ok".into(),
            iterations: 1,
            ..Default::default()
        },
        // rust-doctor-disable-next-line excessive-clone
        invocations: invocations.clone(),
    });

    (
        Orchestrator::new(
            registry,
            Arc::new(defaults),
            session_service,
            sandbox_factory,
            harness,
        ),
        invocations,
    )
}

#[tokio::test]
async fn dispatch_happy_path_returns_handle_and_completes() {
    let (orch, invocations) = fixture_orchestrator();
    let handle = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("hello".into()),
            channel: None,
            session_hint: None,
            owner_user_id: None,
            scope_id: None,
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: crate::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("dispatch ok");

    // rust-doctor-disable-next-line unwrap-in-production
    let completion = handle.completion.await.unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let outcome = completion.unwrap();
    assert_eq!(outcome.final_text, "ok");

    // rust-doctor-disable-next-line unwrap-in-production
    let calls = invocations.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].is_empty(), "session key must be non-empty");
}

#[tokio::test]
async fn dispatch_unknown_flow_id_returns_error() {
    let (orch, _) = fixture_orchestrator();
    let err = orch
        .dispatch(FlowRequest {
            flow_id: Some("does-not-exist".into()),
            agent_id: "main".into(),
            input: FlowInput::Prompt("x".into()),
            channel: None,
            session_hint: None,
            owner_user_id: None,
            scope_id: None,
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: crate::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, FlowError::UnknownFlow(ref id) if id == "does-not-exist"));
}

/// An agent absent from the orchestrator routing table is NOT an error: the
/// gateway already resolved its `AgentInstance` before dispatch, so the run is
/// routed through the canonical `default-agent` flow (see `dispatch()` step 1).
/// This is what lets config- and team-created agents — which live only in the
/// gateway registry, not the orchestrator builtins — execute.
#[tokio::test]
async fn dispatch_unregistered_agent_routes_through_default_flow() {
    let (orch, invocations) = fixture_orchestrator();
    let handle = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "ghost".into(),
            input: FlowInput::Prompt("x".into()),
            channel: None,
            session_hint: None,
            owner_user_id: None,
            scope_id: None,
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: crate::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("unregistered agent routes through default-agent flow");

    // rust-doctor-disable-next-line unwrap-in-production
    let completion = handle.completion.await.unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let outcome = completion.unwrap();
    assert_eq!(outcome.final_text, "ok");
    assert_eq!(invocations.lock().unwrap().len(), 1);
}

use crate::orchestrator::resolver::MAX_FLOW_DEPTH;

#[tokio::test]
async fn dispatch_above_max_depth_returns_recursion_error() {
    let (orch, _) = fixture_orchestrator();
    let err = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("x".into()),
            channel: None,
            session_hint: None,
            owner_user_id: None,
            scope_id: None,
            parent_session: None,
            depth: MAX_FLOW_DEPTH + 1,
            tool_service: None,
            trace_sink: None,
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: crate::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, FlowError::RecursionLimit { max } if max == MAX_FLOW_DEPTH));
}

#[tokio::test]
async fn dispatch_rejects_concurrent_same_session_reuse() {
    let mut spec_map = FlowSet::new();
    let spec = FlowSpec {
        id: "default-agent".into(),
        description: "t".into(),
        agent: "main".into(),
        brain: BrainRef::Default,
        session_strategy: SessionStrategy::Reuse,
        overrides: FlowOverrides::default(),
    };
    spec_map.insert("default-agent".into(), Arc::new(spec));
    let registry = Arc::new(FlowRegistry::new(spec_map));
    let mut defaults = std::collections::HashMap::new();
    defaults.insert("main".into(), "default-agent".into());

    let session_service = fake_session_service();
    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(crate::sandbox::NoopSandbox) as Arc<dyn Sandbox>)
    }));

    struct HangingHarness;
    #[async_trait]
    impl crate::orchestrator::dispatch::HarnessRunner for HangingHarness {
        async fn run(
            &self,
            _s: String,
            _sp: Arc<FlowSpec>,
            _i: FlowInput,
            _sb: Arc<dyn Sandbox>,
            _ev: broadcast::Sender<crate::orchestrator::dispatch::FlowStreamEvent>,
            cancel: CancellationToken,
            _tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
            _trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
            _interaction_manifest: Option<crate::thinker::InteractionManifest>,
            _workspace_override: Option<std::path::PathBuf>,
            _max_iterations_override: Option<u32>,
            _transient_context: Option<String>,
            _think_level: Option<crate::agents::thinking::ThinkLevel>,
            _envelope: crate::thinker::TurnEnvelope,
            _turn_model: Option<crate::providers::session_model_handle::SessionModelPref>,
        ) -> Result<crate::orchestrator::dispatch::FlowOutcome, FlowError> {
            cancel.cancelled().await;
            Ok(crate::orchestrator::dispatch::FlowOutcome {
                final_text: "cancelled".into(),
                iterations: 0,
                ..Default::default()
            })
        }
    }

    let orch = Orchestrator::new(
        registry,
        Arc::new(defaults),
        session_service,
        sandbox_factory,
        Arc::new(HangingHarness),
    );

    let mk_req = || FlowRequest {
        flow_id: None,
        agent_id: "main".into(),
        input: FlowInput::Prompt("x".into()),
        channel: None,
        session_hint: Some("shared-session".into()),
        owner_user_id: None,
        scope_id: None,
        parent_session: None,
        depth: 0,
        tool_service: None,
        trace_sink: None,
        interaction_manifest: None,
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        transient_context: None,
        think_level: None,
        envelope: crate::thinker::TurnEnvelope::none(),
        model_directive: None,
    };

    // rust-doctor-disable-next-line unwrap-in-production
    let first = orch.dispatch(mk_req()).await.expect("first ok");
    let err = orch.dispatch(mk_req()).await.unwrap_err();
    assert!(matches!(err, FlowError::SessionConflict(ref k) if k == "shared-session"));
    first.cancel.cancel();
    let _ = first.completion.await;
}

#[tokio::test]
async fn dispatch_releases_session_lock_after_completion() {
    let (orch, _invocations) = fixture_orchestrator();

    let mk_req = || FlowRequest {
        flow_id: None,
        agent_id: "main".into(),
        input: FlowInput::Prompt("x".into()),
        channel: None,
        session_hint: Some("reusable-session".into()),
        owner_user_id: None,
        scope_id: None,
        parent_session: None,
        depth: 0,
        tool_service: None,
        trace_sink: None,
        interaction_manifest: None,
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        transient_context: None,
        think_level: None,
        envelope: crate::thinker::TurnEnvelope::none(),
        model_directive: None,
    };

    // First dispatch — await completion.
    // rust-doctor-disable-next-line unwrap-in-production
    let first = orch.dispatch(mk_req()).await.expect("first ok");
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = first.completion.await.unwrap();

    // The session lock must now be released — a second dispatch with the
    // SAME session_hint must succeed, not return SessionConflict.
    let second = orch
        .dispatch(mk_req())
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("second ok after release");
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = second.completion.await.unwrap();

    assert_eq!(
        _invocations.lock().unwrap().len(),
        2,
        "harness must have run twice — both dispatches completed"
    );
}

// ── Step 11: forwarding tests ────────────────────────────────────────────────

/// Stub harness that records the `tool_service_override` and `trace_sink`
/// passed into `run` so the test can assert they arrived correctly.
struct CapturingHarness {
    received_tool_service: Arc<Mutex<Option<bool>>>,
    received_trace_sink: Arc<Mutex<Option<bool>>>,
}

#[async_trait]
impl crate::orchestrator::dispatch::HarnessRunner for CapturingHarness {
    async fn run(
        &self,
        _session_key: String,
        _spec: Arc<FlowSpec>,
        _input: FlowInput,
        _sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<crate::orchestrator::dispatch::FlowStreamEvent>,
        _cancel: CancellationToken,
        tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
        trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
        _interaction_manifest: Option<crate::thinker::InteractionManifest>,
        _workspace_override: Option<std::path::PathBuf>,
        _max_iterations_override: Option<u32>,
        _transient_context: Option<String>,
        _think_level: Option<crate::agents::thinking::ThinkLevel>,
        _envelope: crate::thinker::TurnEnvelope,
        _turn_model: Option<crate::providers::session_model_handle::SessionModelPref>,
    ) -> Result<crate::orchestrator::dispatch::FlowOutcome, FlowError> {
        *self
            .received_tool_service
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(tool_service_override.is_some());
        *self
            .received_trace_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(trace_sink.is_some());
        let outcome = crate::orchestrator::dispatch::FlowOutcome {
            final_text: "captured".into(),
            iterations: 1,
            ..Default::default()
        };
        let _ = events.send(crate::orchestrator::dispatch::FlowStreamEvent::Complete(
            // rust-doctor-disable-next-line excessive-clone
            outcome.clone(),
        ));
        Ok(outcome)
    }
}

fn fixture_capturing_orchestrator() -> (
    Orchestrator,
    Arc<Mutex<Option<bool>>>,
    Arc<Mutex<Option<bool>>>,
) {
    let mut spec_map = FlowSet::new();
    let spec = FlowSpec {
        id: "default-agent".into(),
        description: "t".into(),
        agent: "main".into(),
        brain: BrainRef::Default,
        session_strategy: SessionStrategy::Fresh,
        overrides: FlowOverrides::default(),
    };
    spec_map.insert("default-agent".into(), Arc::new(spec));
    let registry = Arc::new(FlowRegistry::new(spec_map));

    let mut defaults = std::collections::HashMap::new();
    defaults.insert("main".into(), "default-agent".into());

    let session_service = fake_session_service();
    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(crate::sandbox::NoopSandbox) as Arc<dyn Sandbox>)
    }));

    let received_tool_service = Arc::new(Mutex::new(None::<bool>));
    let received_trace_sink = Arc::new(Mutex::new(None::<bool>));

    let harness = Arc::new(CapturingHarness {
        // rust-doctor-disable-next-line excessive-clone
        received_tool_service: received_tool_service.clone(),
        // rust-doctor-disable-next-line excessive-clone
        received_trace_sink: received_trace_sink.clone(),
    });

    (
        Orchestrator::new(
            registry,
            Arc::new(defaults),
            session_service,
            sandbox_factory,
            harness,
        ),
        received_tool_service,
        received_trace_sink,
    )
}

/// Minimal ToolService stub for forwarding test — list/describe/execute all no-op.
struct StubToolService;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for StubToolService {
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
    async fn execute(
        &self,
        name: &str,
        _args: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
        Err(crate::tools::service::ToolError::NotFound {
            name: name.to_string(),
        })
    }
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
}

#[tokio::test]
async fn dispatch_forwards_tool_service_override() {
    let (orch, received_tool_service, _received_trace_sink) = fixture_capturing_orchestrator();

    let tool_service: std::sync::Arc<dyn crate::tools::service::ToolService> =
        Arc::new(StubToolService);

    let handle = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("test".into()),
            channel: None,
            session_hint: None,
            owner_user_id: None,
            scope_id: None,
            parent_session: None,
            depth: 0,
            tool_service: Some(tool_service),
            trace_sink: None,
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: crate::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("dispatch ok");

    // rust-doctor-disable-next-line unwrap-in-production
    let _ = handle.completion.await.unwrap();

    let got = received_tool_service
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("harness must have been called");
    assert!(got, "tool_service_override must arrive as Some(_)");
}

#[tokio::test]
async fn dispatch_forwards_trace_sink() {
    let (orch, _received_tool_service, received_trace_sink) = fixture_capturing_orchestrator();

    let trace_sink: std::sync::Arc<dyn crate::harness::TraceSink> =
        Arc::new(crate::harness::NoopTraceSink);

    let handle = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("test".into()),
            channel: None,
            session_hint: None,
            owner_user_id: None,
            scope_id: None,
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: Some(trace_sink),
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: crate::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("dispatch ok");

    // rust-doctor-disable-next-line unwrap-in-production
    let _ = handle.completion.await.unwrap();

    let got = received_trace_sink
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("harness must have been called");
    assert!(got, "trace_sink must arrive as Some(_)");
}
