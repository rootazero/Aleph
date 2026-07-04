//! Tests for the execution engine module.

use super::deadline::wait_for_deadline;
use super::gate::GateOutcome;
use super::*;
use crate::sync_primitives::{AtomicUsize, Ordering};

use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::SessionState;

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tokio::sync::RwLock;

/// Test event emitter that collects events
struct TestEmitter {
    events: Arc<RwLock<Vec<StreamEvent>>>,
    event_count: AtomicUsize,
    seq_counter: AtomicU64,
}

impl TestEmitter {
    fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            event_count: AtomicUsize::new(0),
            seq_counter: AtomicU64::new(0),
        }
    }

    async fn get_events(&self) -> Vec<StreamEvent> {
        self.events.read().await.clone()
    }
}

#[async_trait]
impl EventEmitter for TestEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.events.write().await.push(event);
        self.event_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

fn test_session_manager(
    temp: &tempfile::TempDir,
) -> Arc<crate::gateway::session_manager::SessionManager> {
    let config = crate::gateway::session_manager::SessionManagerConfig {
        db_path: temp.path().join("test_sessions.db"),
        ..Default::default()
    };
    Arc::new(
        crate::gateway::session_manager::SessionManager::new(config).expect("test session manager"),
    )
}

#[tokio::test]
async fn test_simple_execution_engine_basic() {
    let temp = tempfile::tempdir().unwrap();
    let sm = test_session_manager(&temp);
    let config = AgentInstanceConfig {
        agent_id: "test".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let engine = SimpleExecutionEngine::default();

    let request = RunRequest {
        run_id: "test-run-1".to_string(),
        input: "Hello, world!".to_string(),
        session_key: SessionKey::main("test"),
        timeout_secs: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };

    let result = engine.execute(request, agent, emitter.clone()).await;
    assert!(result.is_ok());

    let events = emitter.get_events().await;
    assert!(!events.is_empty());

    // Check for expected events
    let has_run_accepted = events
        .iter()
        .any(|e| matches!(e, StreamEvent::RunAccepted { .. }));
    let has_run_complete = events
        .iter()
        .any(|e| matches!(e, StreamEvent::RunComplete { .. }));

    assert!(has_run_accepted, "Should have RunAccepted event");
    assert!(has_run_complete, "Should have RunComplete event");
}

/// Regression: the persisted session state must return to Idle after a run
/// completes. Without this, the Panel/session list continues to show the
/// session as "running" even though the assistant response has already been
/// written.
#[tokio::test]
async fn test_session_state_returns_to_idle_after_run() {
    let temp = tempfile::tempdir().unwrap();
    let sm = test_session_manager(&temp);
    let config = AgentInstanceConfig {
        agent_id: "test-idle".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test-idle"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let engine = SimpleExecutionEngine::default();

    let session_key = SessionKey::main("test-idle");
    let request = RunRequest {
        run_id: "test-run-idle".to_string(),
        input: "Hello, world!".to_string(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };

    // Persisting the user message transitions the session to Running.
    let result = engine
        .execute(request, agent.clone(), emitter.clone())
        .await;
    assert!(result.is_ok());

    // After the run finishes, the engine must reset the session state.
    let final_state = agent.session_state(&session_key).await;
    assert_eq!(
        final_state,
        Some(SessionState::Idle),
        "session state should return to Idle after the run completes"
    );
}

/// Regression: a brand-new session must be announced via `SessionUpdated` on the
/// **global event bus** at the moment of creation (first user message). The
/// announcement is bus-published rather than emitter-emitted so it reaches the
/// Panel for channel-originated runs too — `ReplyEmitter` only routes
/// channel-facing events and would otherwise drop it. We also pin that the
/// run's `metadata["channel_id"]` is surfaced as `origin_channel`, which the
/// Panel uses to distinguish external updates from its own runs.
///
/// Exercised against `SimpleExecutionEngine` (the harness-friendly engine); the
/// production path is `ExecutionEngine::<P, R>::execute` (execute.rs), which
/// carries the identical first-message publish.
#[tokio::test]
async fn test_first_message_publishes_session_updated_on_bus() {
    let temp = tempfile::tempdir().unwrap();
    let sm = test_session_manager(&temp);
    let config = AgentInstanceConfig {
        agent_id: "test-new-session".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test-new-session"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
    let mut typed_rx = bus.subscribe_typed();
    let engine =
        SimpleExecutionEngine::new(ExecutionEngineConfig::default()).with_event_bus(bus.clone());

    let mut metadata = HashMap::new();
    metadata.insert("channel_id".to_string(), "telegram".to_string());
    let request = RunRequest {
        run_id: "run-new-session".to_string(),
        input: "first message in a brand-new session".to_string(),
        session_key: SessionKey::main("test-new-session"),
        timeout_secs: Some(5),
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };

    let result = engine.execute(request, agent, emitter.clone()).await;
    assert!(result.is_ok());

    // SimpleExecutionEngine's only session announcement is the first-message
    // one, so a received frame proves creation-time announce (not completion).
    let frame = typed_rx
        .try_recv()
        .expect("first message should publish a SessionUpdated frame on the bus");
    match frame {
        crate::gateway::events::GatewayEventFrame::SessionUpdated {
            session_key,
            origin_channel,
        } => {
            assert_eq!(
                session_key,
                SessionKey::main("test-new-session").to_key_string()
            );
            assert_eq!(
                origin_channel.as_deref(),
                Some("telegram"),
                "the run's channel_id metadata must surface as origin_channel"
            );
        }
        other => panic!("expected SessionUpdated frame, got {other:?}"),
    }
}

#[tokio::test]
async fn test_simple_execution_engine_run() {
    let temp = tempfile::tempdir().unwrap();
    let sm = test_session_manager(&temp);
    let config = AgentInstanceConfig {
        agent_id: "test-simple".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test-simple"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let engine = SimpleExecutionEngine::new(ExecutionEngineConfig {
        default_timeout_secs: 10,
        ..Default::default()
    });

    let request = RunRequest {
        run_id: "run-simple".to_string(),
        input: "Test input".to_string(),
        session_key: SessionKey::main("test-simple"),
        timeout_secs: Some(5),
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };

    // This should succeed and complete quickly
    let result = engine
        .execute(request, agent.clone(), emitter.clone())
        .await;
    assert!(result.is_ok());

    // Verify events were emitted
    let events = emitter.get_events().await;
    let has_reasoning = events
        .iter()
        .any(|e| matches!(e, StreamEvent::Reasoning { .. }));
    let has_response = events
        .iter()
        .any(|e| matches!(e, StreamEvent::ResponseChunk { .. }));

    assert!(has_reasoning, "Should have Reasoning event");
    assert!(has_response, "Should have ResponseChunk event");
}

// =============================================================================
// Task 6: per-session admission gate (production `ExecutionEngine<P, R>`)
// =============================================================================
//
// These exercise `ExecutionEngine::admit_run` directly rather than the full
// `execute()` (which needs a live orchestrator/harness to actually run a
// turn). `admit_run` IS the wired gate under test — Task 3's own unit tests
// already cover `SessionRunRegistry` in isolation; these prove it's correctly
// threaded through the production engine's admission path.

/// Minimal `ToolRegistry` double: `admit_run` never looks up or executes a
/// tool, so an empty registry satisfies `ExecutionEngine<P, R>`'s generic
/// bound without pulling in the real (heavy) `BuiltinToolRegistry`.
struct EmptyToolRegistry;

impl crate::executor::ToolRegistry for EmptyToolRegistry {
    fn get_tool(&self, _name: &str) -> Option<&crate::tool_metadata::UnifiedTool> {
        None
    }

    fn execute_tool(
        &self,
        _tool_name: &str,
        _arguments: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crate::error::Result<serde_json::Value>> + Send + '_>,
    > {
        Box::pin(async { Err(crate::error::AlephError::tool("no tools in test registry")) })
    }
}

/// A production `ExecutionEngine<P, R>` with the lightest real `P`/`R` the
/// generic bounds allow, using an explicit config. `admit_run` never calls
/// into either registry, so these stand in for the real provider/tool stacks
/// without any network or filesystem setup — this is the smallest real unit
/// that exercises the wired gate.
fn test_engine_with_config(
    config: ExecutionEngineConfig,
) -> ExecutionEngine<crate::thinker::SingleProviderRegistry, EmptyToolRegistry> {
    ExecutionEngine::new(
        config,
        Arc::new(crate::thinker::SingleProviderRegistry::new(
            crate::providers::create_mock_provider(),
        )),
        Arc::new(EmptyToolRegistry),
        Vec::new(),
        None,
    )
}

/// Default-config engine for tests that hold at most one permit at a time.
fn test_engine() -> ExecutionEngine<crate::thinker::SingleProviderRegistry, EmptyToolRegistry> {
    test_engine_with_config(ExecutionEngineConfig::default())
}

fn gate_test_request(session_key: &SessionKey, run_id: &str) -> RunRequest {
    RunRequest {
        run_id: run_id.to_string(),
        input: "hello".to_string(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    }
}

async fn gate_test_agent(temp: &tempfile::TempDir, agent_id: &str) -> Arc<AgentInstance> {
    let sm = test_session_manager(temp);
    let config = AgentInstanceConfig {
        agent_id: agent_id.to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join(format!("agents/{agent_id}")),
        ..Default::default()
    };
    Arc::new(AgentInstance::new(config, sm).unwrap())
}

/// Task 6 regression: the OLD gate (`agent.try_start_run`, flipping a single
/// `AgentState` flag shared by the whole `AgentInstance`) rejected a second
/// run of the SAME agent even on a completely different session — the two
/// sessions serialized through one agent-wide Idle/Running flag. The NEW gate
/// claims per-`SessionKey` (`SessionRunRegistry`), so two sessions of the
/// SAME agent must now both be admitted (true parallelism).
#[tokio::test]
async fn two_sessions_same_agent_run_in_parallel() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "parallel-agent").await;
    // This test holds TWO concurrent same-agent permits (run A's `RunSlot` is
    // alive while run B acquires). It therefore needs per-agent cap >= 2 (and
    // global cap >= 2) — set explicitly so the assertion never depends on the
    // default: if the default per-agent cap were ever lowered to 1, run B's
    // `admit_run` would block forever on `acquire().await` and this test would
    // HANG instead of failing cleanly.
    let engine = test_engine_with_config(ExecutionEngineConfig {
        max_runs_global: 8,
        max_runs_per_agent: 3,
        ..Default::default()
    });

    let session_a = SessionKey::peer("parallel-agent", "conv-a");
    let session_b = SessionKey::peer("parallel-agent", "conv-b");
    // Sanity: genuinely the same agent, genuinely different sessions —
    // otherwise this test would prove nothing.
    assert_eq!(session_a.agent_id(), session_b.agent_id());
    assert_ne!(session_a, session_b);

    let req_a = gate_test_request(&session_a, "run-a");
    let (tx_a, _rx_a) = mpsc::channel::<()>(1);
    let outcome_a = engine
        .admit_run(&req_a, "run-a", &agent, tx_a)
        .await
        .expect("run A must be admitted");
    assert!(
        matches!(outcome_a, GateOutcome::Admitted(_)),
        "first run must be admitted"
    );

    // Same `agent` instance held across both calls — its per-agent
    // `AgentState` is still "Running" from run A (the new gate never touches
    // it), so the OLD gate would have rejected this second call outright.
    // The new per-session gate must admit it: it's a DIFFERENT session.
    let req_b = gate_test_request(&session_b, "run-b");
    let (tx_b, _rx_b) = mpsc::channel::<()>(1);
    let outcome_b = engine
        .admit_run(&req_b, "run-b", &agent, tx_b)
        .await
        .expect("run B on a different session of the SAME agent must be admitted");
    assert!(
        matches!(outcome_b, GateOutcome::Admitted(_)),
        "second run on a DIFFERENT session of the SAME agent must also be admitted"
    );
}

/// Task 6 regression: a second message on the SAME session while a run is
/// still active must still take the busy-input path (`try_claim` returns
/// `false`) — the session-scoped claim must not accidentally admit two runs
/// on one session (which would let two runs interleave writes into the same
/// `session_events` transcript — INV-SEQ / audit 4.2). No orchestrator is
/// wired in this harness, so `Steer`'s mid-loop injection cannot succeed and
/// the busy path surfaces as `AgentBusy` — proof `try_claim` rejected the
/// second run (which busy sub-mode fires is steering.rs's own concern, unit
/// tested separately).
#[tokio::test]
async fn second_message_same_session_takes_busy_path() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "busy-agent").await;
    let engine = test_engine();

    let session = SessionKey::peer("busy-agent", "conv-1");

    let req1 = gate_test_request(&session, "run-1");
    let (tx1, _rx1) = mpsc::channel::<()>(1);
    let outcome1 = engine
        .admit_run(&req1, "run-1", &agent, tx1)
        .await
        .expect("first run must be admitted");
    assert!(matches!(outcome1, GateOutcome::Admitted(_)));

    // `outcome1` (and the `RunSlot` it carries) is still alive here — the
    // session claim from run 1 is still held, so this second call on the
    // SAME session must be rejected by `try_claim`.
    let req2 = gate_test_request(&session, "run-2");
    let (tx2, _rx2) = mpsc::channel::<()>(1);
    let outcome2 = engine.admit_run(&req2, "run-2", &agent, tx2).await;
    match outcome2 {
        Err(ExecutionError::AgentBusy(_)) => {}
        Ok(_) => panic!(
            "second run on the SAME session must be rejected by try_claim (busy path), \
             but was Admitted/HandledInline"
        ),
        Err(e) => panic!("expected AgentBusy from the busy path, got a different error: {e}"),
    }

    drop(outcome1);
}

/// Task 6: `RunSlot::drop` must release the session claim (alongside the
/// concurrency permit) — otherwise a completed run would wedge its session
/// forever. Task 3 already covers `SessionRunRegistry::release` in isolation;
/// this proves the RAII wiring end-to-end through the actual `admit_run`
/// gate, catching a regression where `RunSlot` forgot to release on drop or
/// `execute()` forgot to bind/hold it.
#[tokio::test]
async fn run_slot_drop_releases_session_for_reclaim() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "release-agent").await;
    let engine = test_engine();
    let session = SessionKey::peer("release-agent", "conv-1");

    let req1 = gate_test_request(&session, "run-1");
    let (tx1, _rx1) = mpsc::channel::<()>(1);
    let outcome1 = engine
        .admit_run(&req1, "run-1", &agent, tx1)
        .await
        .expect("first run must be admitted");

    // Release the slot the way `execute()` does on run completion.
    drop(outcome1);

    // A fresh run on the SAME session must now be admitted again.
    let req2 = gate_test_request(&session, "run-2");
    let (tx2, _rx2) = mpsc::channel::<()>(1);
    let outcome2 = engine
        .admit_run(&req2, "run-2", &agent, tx2)
        .await
        .expect("run after release must be admitted");
    assert!(matches!(outcome2, GateOutcome::Admitted(_)));
}

// =============================================================================
// Cascade timeout resolution
// =============================================================================

/// Helper: resolve timeout using the same cascade as engine.rs:349-352
fn resolve_timeout(
    request_timeout: Option<u64>,
    agent_timeout: Option<u64>,
    engine_default: u64,
) -> u64 {
    request_timeout.or(agent_timeout).unwrap_or(engine_default)
}

#[test]
fn test_cascade_request_wins_over_agent_and_global() {
    assert_eq!(resolve_timeout(Some(30), Some(600), 172_800), 30);
}

#[test]
fn test_cascade_agent_wins_over_global() {
    assert_eq!(resolve_timeout(None, Some(3600), 172_800), 3600);
}

#[test]
fn test_cascade_falls_through_to_global() {
    assert_eq!(resolve_timeout(None, None, 172_800), 172_800);
}

#[test]
fn test_cascade_request_overrides_even_when_agent_is_none() {
    assert_eq!(resolve_timeout(Some(60), None, 172_800), 60);
}

#[test]
fn test_cascade_matches_engine_default() {
    let engine_config = ExecutionEngineConfig::default();
    assert_eq!(
        resolve_timeout(None, None, engine_config.default_timeout_secs),
        172_800
    );
}

/// Task 5 (audit 1.3): `max_concurrent_runs` is retired in favor of two
/// explicit concurrency caps consumed by `ConcurrencyLimiter` (Task 6).
#[test]
fn engine_config_default_concurrency_caps() {
    let c = ExecutionEngineConfig::default();
    assert_eq!(c.max_runs_global, 8);
    assert_eq!(c.max_runs_per_agent, 3);
}

// =============================================================================
// wait_for_deadline — resettable deadline behavior
// =============================================================================

#[tokio::test]
async fn test_wait_for_deadline_fires_at_deadline() {
    let deadline = Arc::new(tokio::sync::Mutex::new(
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(100),
    ));

    let start = tokio::time::Instant::now();
    wait_for_deadline(deadline).await;
    let elapsed = start.elapsed();

    // Should fire after ~100ms (with some tolerance)
    assert!(
        elapsed >= tokio::time::Duration::from_millis(80),
        "fired too early: {:?}",
        elapsed
    );
    assert!(
        elapsed < tokio::time::Duration::from_millis(500),
        "fired too late: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_wait_for_deadline_extension_delays_firing() {
    // Margins are generous (extension lands ~90ms before the initial deadline,
    // lower bound sits ~100ms above the initial deadline) so a loaded CI runner
    // with coarse (~15ms) timer granularity that starves the extender task does
    // not make this flake — a tight margin fired early on Windows runners.
    let deadline = Arc::new(tokio::sync::Mutex::new(
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(150),
    ));

    let deadline_clone = deadline.clone();

    // Extend the deadline at ~60ms (well before the 150ms initial deadline).
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(60)).await;
        *deadline_clone.lock().await =
            tokio::time::Instant::now() + tokio::time::Duration::from_millis(250);
    });

    let start = tokio::time::Instant::now();
    wait_for_deadline(deadline).await;
    let elapsed = start.elapsed();

    // Should fire after ~310ms (60ms wait + 250ms extended), not at 150ms.
    assert!(
        elapsed >= tokio::time::Duration::from_millis(250),
        "deadline extension was ignored, fired too early: {:?}",
        elapsed
    );
    assert!(
        elapsed < tokio::time::Duration::from_secs(2),
        "fired too late: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_wait_for_deadline_multiple_extensions() {
    // Generous margins (each extension lands >100ms before the deadline it must
    // beat; lower bound sits 100ms above the initial deadline) keep this from
    // flaking when a loaded Windows CI runner starves the extender task — the
    // original 20ms first-extension margin fired early under scheduler pressure.
    let deadline = Arc::new(tokio::sync::Mutex::new(
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(250),
    ));

    let dl = deadline.clone();

    // Extend twice: at ~80ms (before the 250ms initial deadline) and at ~260ms
    // (before the ~380ms first-extended deadline).
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
        *dl.lock().await = tokio::time::Instant::now() + tokio::time::Duration::from_millis(300);

        tokio::time::sleep(tokio::time::Duration::from_millis(180)).await;
        *dl.lock().await = tokio::time::Instant::now() + tokio::time::Duration::from_millis(300);
    });

    let start = tokio::time::Instant::now();
    wait_for_deadline(deadline).await;
    let elapsed = start.elapsed();

    // Should fire after ~560ms (80 + 180 + 300), not at the 250ms initial.
    assert!(
        elapsed >= tokio::time::Duration::from_millis(350),
        "multiple extensions were ignored: {:?}",
        elapsed
    );
    assert!(
        elapsed < tokio::time::Duration::from_secs(2),
        "fired too late: {:?}",
        elapsed
    );
}

// =============================================================================
// Stream callback trace persistence
// =============================================================================

#[tokio::test]
async fn stream_callback_persists_agent_trace_events() {
    use crate::gateway::execution_engine::callback::{StreamCallbackState, TracePersistence};
    use crate::harness::trace::LoopTraceEvent;
    use crate::resilience::{AgentTask, RiskLevel};

    let db = Arc::new(crate::resilience::StateDatabase::in_memory().unwrap());
    db.insert_agent_task(&AgentTask::new(
        "run-1",
        "session-1",
        "coder",
        "persist trace",
        RiskLevel::High,
    ))
    .await
    .unwrap();
    db.update_task_status("run-1", crate::resilience::TaskStatus::Running)
        .await
        .unwrap();

    let shared = Arc::new(StreamCallbackState::new(Some(Arc::new(
        TracePersistence::new(db.clone(), "run-1".to_string()),
    ))));

    // Test persistence directly via StreamCallbackState (post-flip: StreamCallback
    // is dead code; the production path uses GatewayTraceSink/CallbackStateFlushHandle).
    shared.persist_trace(&LoopTraceEvent::TurnStarted { iteration: 1 });
    shared.flush_trace_persistence().await;

    let traces = db.get_traces_by_task("run-1").await.unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].event.kind(), "turn_started");
    assert_eq!(
        traces[0].event,
        aleph_protocol::AgentTraceEvent::TurnStarted { iteration: 1 }
    );
}
