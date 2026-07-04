//! Tests for the execution engine module.

use super::deadline::wait_for_deadline;
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
