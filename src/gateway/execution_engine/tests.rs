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

/// Install (or join) the process-wide goal store every goal test in this binary
/// shares, and return whichever store actually became global — that is the one
/// `confirm_fire` / `block_goal_on_failure` / the wake sweep all read.
///
/// The directory is anchored to the PROCESS, not to the installing test. The
/// global is a set-once `OnceCell`, so a `TempDir` dropped at the end of the
/// winning test would delete the database out from under every later test that
/// reads it — and, since cargo runs tests in parallel, out from under a
/// concurrent one. That was harmless while exactly one test used the global; it
/// stops being harmless the moment a second one does.
fn goal_store_global() -> Arc<crate::goal::GoalStore> {
    // `OnceLock<TempDir>` never drops (statics don't), so the directory is
    // registered for removal at process exit instead of abandoned.
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let dir = DIR.get_or_init(|| {
        crate::utils::scratch::keep_until_exit(tempfile::tempdir().expect("goal store dir"))
    });
    crate::goal::set_global_for_test(Arc::new(
        crate::goal::GoalStore::open(&dir.join("goals.db")).expect("goal store"),
    ));
    crate::goal::global().expect("a goal store is installed")
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
/// run's `metadata["channel_id"]` is surfaced as `origin_channel`, and that the
/// RUN that caused the update is surfaced as `origin_run_id` — the latter is
/// what a client compares against the run ids its own `chat.send` returned, so
/// dropping it at the publish site turns "my own update" into "somebody else's"
/// (a transcript clobber) and "somebody else's" into nothing at all (a room
/// peer's message never appears). `origin_channel` cannot answer that question:
/// every Panel connection sends the same `gui:chat` literal.
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
            origin_run_id,
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
            assert_eq!(
                origin_run_id.as_deref(),
                Some("run-new-session"),
                "the frame must name the run that caused it — a client cannot \
                 tell its own update from a peer's without it"
            );
        }
        other => panic!("expected SessionUpdated frame, got {other:?}"),
    }
}

/// The Panel case, which is the whole reason `origin_run_id` exists.
///
/// A Panel `chat.send` hardcodes `"channel": "gui:chat"` (`api/chat.rs`), so
/// EVERY Panel connection — a second tab of the same user, a second member of a
/// project room — stamps the identical `origin_channel`. A client that reads
/// that literal as "this is my own update" is answering an identity question
/// with a class answer, and is wrong for every connection but the one that sent.
/// So the frame must still carry something only the sender holds: the run id its
/// own `chat.send` response returned.
///
/// Asserted together on purpose — `origin_channel == "gui:chat"` is the premise
/// that makes `origin_run_id` load-bearing, and a test that pinned only the run
/// id would go green if the channel literal quietly became per-connection.
#[tokio::test]
async fn panel_originated_session_update_names_its_run_not_just_gui_chat() {
    let temp = tempfile::tempdir().unwrap();
    let sm = test_session_manager(&temp);
    let config = AgentInstanceConfig {
        agent_id: "test-panel-origin".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test-panel-origin"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
    let mut typed_rx = bus.subscribe_typed();
    let engine =
        SimpleExecutionEngine::new(ExecutionEngineConfig::default()).with_event_bus(bus.clone());

    let mut metadata = HashMap::new();
    metadata.insert("channel_id".to_string(), "gui:chat".to_string());
    let request = RunRequest {
        run_id: "run-from-tab-a".to_string(),
        input: "hello from a Panel tab".to_string(),
        session_key: SessionKey::main("test-panel-origin"),
        timeout_secs: Some(5),
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };

    engine.execute(request, agent, emitter).await.unwrap();

    let frame = typed_rx
        .try_recv()
        .expect("first message should publish a SessionUpdated frame on the bus");
    match frame {
        crate::gateway::events::GatewayEventFrame::SessionUpdated {
            origin_channel,
            origin_run_id,
            ..
        } => {
            assert_eq!(
                origin_channel.as_deref(),
                Some("gui:chat"),
                "the premise: every Panel connection stamps this same literal"
            );
            assert_eq!(
                origin_run_id.as_deref(),
                Some("run-from-tab-a"),
                "…so the run id is the only thing in this frame that tells the \
                 sending tab apart from every other Panel watching the session"
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
/// An engine whose parser cell was never filled must resolve nothing, rather
/// than reaching for a second, weaker derivation. Guessing `direct_tool` from
/// a bare registry lookup is exactly the drift the convergence removed.
#[tokio::test]
async fn an_absent_parser_resolves_nothing_rather_than_guessing() {
    let engine = test_engine();
    let mut md = std::collections::HashMap::new();
    engine.stamp_slash_mode("/help", &mut md).await;
    assert!(
        md.is_empty(),
        "no parser means no answer; a fallback derivation would be a second \
         definition of what `/foo` means"
    );
}

/// Idempotent: the inbound router stamps before the engine ever sees the
/// request, and re-resolving would let a later, differently-derived answer
/// overwrite the router's.
#[tokio::test]
async fn stamp_slash_mode_never_overwrites_an_existing_stamp() {
    let engine = test_engine();
    let mut md = std::collections::HashMap::new();
    md.insert(
        crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY.to_string(),
        "already-resolved".to_string(),
    );
    engine.stamp_slash_mode("/help", &mut md).await;
    assert_eq!(
        md[crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY],
        "already-resolved"
    );
}

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

/// Round-7 regression: a run that has claimed its session slot but is still
/// parked on the per-agent concurrency semaphore must be reachable by cancel.
///
/// `try_claim` withdraws the run's busy-queue ticket (`mark_admitted`), so once
/// it succeeds the message is no longer "waiting" anywhere the queue can see.
/// If `active_runs` is only populated after `concurrency.acquire().await` — an
/// unbounded wait — the run exists in NO registry any stop path consults:
/// `cancel` / `cancel_session` read `active_runs` and miss, and
/// `cancel_queued_run` / `purge` find no ticket. Stop answers "nothing is
/// running" and then the cancelled turn executes anyway, whenever a permit
/// frees up. Asserts the effect at the consumer — `cancel` reaches it — not
/// that some registration function was called.
#[tokio::test]
async fn a_run_waiting_for_a_concurrency_permit_can_still_be_cancelled() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "capped-agent").await;
    // One permit for the whole agent, so the second admission must park.
    let engine = Arc::new(test_engine_with_config(ExecutionEngineConfig {
        max_runs_global: 8,
        max_runs_per_agent: 1,
        ..Default::default()
    }));

    let session_a = SessionKey::peer("capped-agent", "conv-a");
    let session_b = SessionKey::peer("capped-agent", "conv-b");

    let req_a = gate_test_request(&session_a, "run-a");
    let (tx_a, _rx_a) = mpsc::channel::<()>(1);
    let _slot_a = match engine
        .admit_run(&req_a, "run-a", &agent, tx_a)
        .await
        .expect("run A must be admitted")
    {
        GateOutcome::Admitted(slot) => slot,
        GateOutcome::HandledInline => panic!("an idle session must not be handled inline"),
    };

    // Run B is on a different session (its own slot is free) but the agent's
    // only permit is held by A, so this call parks inside `admit_run`.
    let engine_b = Arc::clone(&engine);
    let agent_b = Arc::clone(&agent);
    let req_b = gate_test_request(&session_b, "run-b");
    let (tx_b, mut rx_b) = mpsc::channel::<()>(1);
    let parked = tokio::spawn(async move {
        engine_b
            .admit_run(&req_b, "run-b", &agent_b, tx_b)
            .await
            .map(|outcome| matches!(outcome, GateOutcome::Admitted(_)))
    });

    // Let it get as far as the semaphore.
    let claimed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if engine.get_status("run-b").await.is_some() {
                return true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        claimed.unwrap_or(false),
        "a run holding a session claim must be visible to the stop paths while it \
         waits for a permit — it has already left the busy queue's wait lane"
    );

    // The consumer that matters: Stop must actually reach it.
    engine
        .cancel("run-b")
        .await
        .expect("cancelling a permit-waiting run must not report RunNotFound");
    assert!(
        rx_b.recv().await.is_some(),
        "the cancel must reach the run's own channel, so the loop sees it the \
         moment the permit lands instead of executing a turn the user stopped"
    );

    // Release A so the parked admission can finish and the task can join.
    drop(_slot_a);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), parked).await;
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

/// Task 9 regression: `cancel_session` on a LEADER session must also cancel
/// an in-flight DELEGATED CHILD run registered in the process-global
/// `BackgroundAgentTracker` under the leader's `root_session` — otherwise a
/// `/stop` / Panel `chat.abort` on the leader leaves the delegated member
/// DETACHED (still running, still burning tokens).
///
/// This exercises the REAL cancel chain, not just the tracker's own API
/// (Task 8's test only proved shape-parity): the member run is admitted
/// through the actual engine gate (`admit_run`, the same seam `execute()`
/// uses), which registers it in `active_runs` with a REAL `cancel_tx` under
/// a known run_id — exactly the state a genuine delegated child run would be
/// in. That id is then registered in the tracker under the leader's
/// `root_session` (mirroring what `delegate.rs` now does at spawn time —
/// Task 8). Calling `engine.cancel_session(&leader_key)` must walk the
/// tracker, find the child, and fire `engine.cancel(child)` on it — observed
/// here by actually receiving the signal on the member run's own `cancel_tx`
/// channel, the same channel a real `execute()` loop selects on to tear down
/// its `AgentLoop`. No orchestrator is wired in this harness (matching every
/// other gate test in this file), so this is the deepest real assertion
/// available without spinning one up — it is the exact mechanism, not a
/// restatement of the tracker's own `running_runs_of_session` unit test.
#[tokio::test]
async fn cancel_session_cancels_in_flight_delegated_child() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "delegate-agent").await;
    let engine = test_engine();

    let leader_key = SessionKey::main("leader-cancel-it");
    let member_key = SessionKey::task("delegate-agent", "team", "task-cancel");
    let member_run_id = "member-run-cancel-1";

    // Start a REAL member run through the engine's admission gate. This
    // registers `member_run_id` in `active_runs` with a real `cancel_tx`,
    // just as `execute()` does for a genuine delegated child run.
    let member_req = gate_test_request(&member_key, member_run_id);
    let (member_tx, mut member_rx) = mpsc::channel::<()>(1);
    let member_outcome = engine
        .admit_run(&member_req, member_run_id, &agent, member_tx)
        .await
        .expect("member run must be admitted");
    assert!(matches!(member_outcome, GateOutcome::Admitted(_)));

    // Register that SAME run_id in the tracker under the leader's root
    // session — mirrors the delegate registration wired in Task 8.
    let _reg = crate::agents::background_tracker::RunningRegistration::register(
        crate::sync_primitives::Arc::clone(
            &crate::agents::background_tracker::BackgroundAgentTracker::global(),
        ),
        member_run_id.to_string(),
        tokio_util::sync::CancellationToken::new(),
        "delegated member".to_string(),
        crate::agents::background_tracker::SpawnMeta {
            parent_id: None,
            depth: 1,
            root_session: leader_key.to_key_string(),
            model: None,
        },
    );

    // The leader has no own active run — `cancel_session` must still walk
    // and cancel the delegated child (the child walk runs regardless of
    // whether an own-run target existed).
    let cancelled = engine
        .cancel_session(&leader_key)
        .await
        .expect("cancel_session must not error");
    assert_eq!(
        cancelled, None,
        "leader has no own run; the returned id must stay None (contract unchanged)"
    );

    // The member's REAL per-run cancel token must have fired. Receiving here
    // proves `cancel_session` walked the tracker, found the child under the
    // leader's root_session, and invoked the real engine `cancel()` on it —
    // closing the detached-member leak.
    member_rx
        .try_recv()
        .expect("cancel_session must have fired the member run's real cancel_tx");

    // Keep the member's RunSlot alive until here, mirroring the neighbouring
    // busy-path tests.
    drop(member_outcome);
}

/// Companion to `cancel_session_cancels_in_flight_delegated_child` (which covers
/// the leader having NO own run): a `cancel_session` on a leader that IS running
/// its own run AND owns an in-flight delegated child must cancel BOTH rails at
/// once — the leader's own run (returned as `Some(run_id)`) and the tracked
/// child (its real per-run cancel token fires). Locks the two cancel paths
/// composing so a future change can't quietly drop one.
#[tokio::test]
async fn cancel_session_cancels_own_run_and_in_flight_delegated_child() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "delegate-agent").await;
    let engine = test_engine();

    let leader_key = SessionKey::peer("delegate-agent", "leader-own-and-child");
    let leader_run_id = "leader-run-own-1";
    let member_key = SessionKey::task("delegate-agent", "team", "task-own-child");
    let member_run_id = "member-run-own-child-1";

    // Leader's OWN run — admitted on the leader session with a real cancel_tx.
    let leader_req = gate_test_request(&leader_key, leader_run_id);
    let (leader_tx, mut leader_rx) = mpsc::channel::<()>(1);
    let leader_outcome = engine
        .admit_run(&leader_req, leader_run_id, &agent, leader_tx)
        .await
        .expect("leader run must be admitted");
    assert!(matches!(leader_outcome, GateOutcome::Admitted(_)));

    // Delegated member run — admitted on its own task session with a real
    // cancel_tx, then registered in the tracker under the leader's root_session
    // (mirrors the delegate registration wired in Item 1 / Task 8).
    let member_req = gate_test_request(&member_key, member_run_id);
    let (member_tx, mut member_rx) = mpsc::channel::<()>(1);
    let member_outcome = engine
        .admit_run(&member_req, member_run_id, &agent, member_tx)
        .await
        .expect("member run must be admitted");
    assert!(matches!(member_outcome, GateOutcome::Admitted(_)));
    let _reg = crate::agents::background_tracker::RunningRegistration::register(
        crate::sync_primitives::Arc::clone(
            &crate::agents::background_tracker::BackgroundAgentTracker::global(),
        ),
        member_run_id.to_string(),
        tokio_util::sync::CancellationToken::new(),
        "delegated member".to_string(),
        crate::agents::background_tracker::SpawnMeta {
            parent_id: None,
            depth: 1,
            root_session: leader_key.to_key_string(),
            model: None,
        },
    );

    // Cancel the leader session: own run + delegated child must both be cancelled.
    let cancelled = engine
        .cancel_session(&leader_key)
        .await
        .expect("cancel_session must not error");
    assert_eq!(
        cancelled,
        Some(leader_run_id.to_string()),
        "cancel_session must return the leader's own cancelled run id"
    );

    leader_rx
        .try_recv()
        .expect("the leader's own run cancel_tx must have fired");
    member_rx
        .try_recv()
        .expect("the delegated child's cancel_tx must have fired (child walk)");

    drop(leader_outcome);
    drop(member_outcome);
}

/// Item 1 follow-up (demote → real cancel): a busy-input `Interrupt` that lands
/// on a session driving an in-flight sub-agent fan-out must now CANCEL the
/// running sibling AND its delegated children (then restart via the busy queue),
/// NOT demote-to-queue. Before the simplification the fan-out signal force-
/// demoted the Interrupt so nothing was cancelled; after it, the Interrupt fires
/// a real `cancel_session` — the same mechanism `/stop` uses. Exercises the new
/// semantic end-to-end through the actual `admit_run` gate.
#[tokio::test]
async fn interrupt_on_fanout_session_cancels_parent_and_children() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "delegate-agent").await;
    let engine = test_engine();

    let leader_key = SessionKey::peer("delegate-agent", "leader-interrupt-fanout");
    let leader_run_id = "leader-run-interrupt-1";
    let member_key = SessionKey::task("delegate-agent", "team", "task-interrupt");
    let member_run_id = "member-run-interrupt-1";

    // Leader's own run holds the leader session claim.
    let leader_req = gate_test_request(&leader_key, leader_run_id);
    let (leader_tx, mut leader_rx) = mpsc::channel::<()>(1);
    let leader_outcome = engine
        .admit_run(&leader_req, leader_run_id, &agent, leader_tx)
        .await
        .expect("leader run must be admitted");
    assert!(matches!(leader_outcome, GateOutcome::Admitted(_)));

    // In-flight delegated child: admitted (real cancel_tx) + tracker-registered
    // under the leader's root_session. This is the "active fan-out" signal that
    // used to force the Interrupt to demote-to-queue.
    let member_req = gate_test_request(&member_key, member_run_id);
    let (member_tx, mut member_rx) = mpsc::channel::<()>(1);
    let member_outcome = engine
        .admit_run(&member_req, member_run_id, &agent, member_tx)
        .await
        .expect("member run must be admitted");
    assert!(matches!(member_outcome, GateOutcome::Admitted(_)));
    let _reg = crate::agents::background_tracker::RunningRegistration::register(
        crate::sync_primitives::Arc::clone(
            &crate::agents::background_tracker::BackgroundAgentTracker::global(),
        ),
        member_run_id.to_string(),
        tokio_util::sync::CancellationToken::new(),
        "delegated member".to_string(),
        crate::agents::background_tracker::SpawnMeta {
            parent_id: None,
            depth: 1,
            root_session: leader_key.to_key_string(),
            model: None,
        },
    );

    // A second message on the leader session with Interrupt busy-input mode and
    // real steering content ("hello" from gate_test_request). It loses
    // `try_claim` → busy path → Interrupt branch.
    let mut interrupt_req = gate_test_request(&leader_key, "interrupt-run");
    interrupt_req.metadata.insert(
        BUSY_INPUT_MODE_KEY.to_string(),
        BusyInputMode::Interrupt.as_wire().to_string(),
    );
    let (int_tx, _int_rx) = mpsc::channel::<()>(1);
    let outcome = engine
        .admit_run(&interrupt_req, "interrupt-run", &agent, int_tx)
        .await;
    assert!(
        matches!(outcome, Err(ExecutionError::AgentBusy(_))),
        "an Interrupt on a busy session returns AgentBusy (message restarts via the busy queue)"
    );

    // Real cancel, not demote: BOTH the leader's own run and the delegated child
    // must have received their real per-run cancel signals.
    leader_rx
        .try_recv()
        .expect("Interrupt must cancel the leader's own run (demote → real cancel)");
    member_rx
        .try_recv()
        .expect("Interrupt must cancel the in-flight delegated child (no detached leak)");

    drop(leader_outcome);
    drop(member_outcome);
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

// =============================================================================
// Task 7 (INV-ISO): concurrent runs preserve agent memory isolation +
// non-interleaved transcripts
// =============================================================================
//
// Task 6 moved run admission from a single per-agent flag to a per-session
// claim, so two sessions of the SAME agent can now execute concurrently (see
// `two_sessions_same_agent_run_in_parallel` above). INV-ISO is the redline
// that parallelism must not violate: one run must never see another run's
// identity or memory. The isolation mechanism itself was NOT touched by
// Task 6:
//   - `TURN_CONTEXT` (`src/tools/turn_context.rs`) — a tokio `task_local!`
//     carrying each run's `SessionKey` — is scoped fresh per task by
//     `ScopedToolService::execute_with_cancel`, the single production
//     tool-dispatch chokepoint.
//   - `NoteManageTool::resolve_agent_id` (`src/builtin_tools/note_manage.rs`)
//     resolves the storage partition from that task-local (falling back to
//     `DEFAULT_AGENT_ID` outside a scoped turn), and both the on-disk layout
//     (`memory_dir/{agent_id}/{category}/{file}.md`) and the SQLite index
//     (`notes_index` PRIMARY KEY `(agent_id, path)`) key off it.
//
// The two tests below drive REAL concurrent `tokio::spawn` tasks through
// that exact production mechanism — the same `TURN_CONTEXT.scope(...)` idiom
// the dispatch chokepoint uses — to prove it holds. They are expected to
// PASS today; a FAIL means a real isolation regression (not a test bug) —
// see `.superpowers/sdd/task-7-brief.md`.

/// The `TurnContext` a real tool dispatch would scope around a call
/// originating from `session_key` — mirrors the value
/// `ScopedToolService::execute_with_cancel` sets in production
/// (`src/tools/scoped/mod.rs`).
fn inv_iso_turn(session_key: SessionKey) -> crate::tools::turn_context::TurnContext {
    crate::tools::turn_context::TurnContext {
        session_key,
        run_id: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        caller_role: None,
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
    }
}

/// `NoteManageArgs` for a `create` call. `agent_id` is deliberately left
/// `None` so the partition is resolved purely from the active
/// `TURN_CONTEXT` — the exact path under test.
fn inv_iso_create_args(
    filename: &str,
    content: &str,
) -> crate::builtin_tools::note_manage::NoteManageArgs {
    use crate::builtin_tools::note_manage::{NoteManageAction, NoteManageArgs};
    NoteManageArgs {
        action: NoteManageAction::Create,
        category: Some("learning".to_string()),
        filename: Some(filename.to_string()),
        title: Some(filename.to_string()),
        content: Some(content.to_string()),
        facts: None,
        links: None,
        tags: None,
        query: None,
        limit: None,
        new_title: None,
        relations: None,
        agent_id: None,
    }
}

/// `NoteManageArgs` for a `list` call scoped to `category`, `agent_id` again
/// left to `TURN_CONTEXT` resolution — used to read a partition's contents
/// back black-box, through the same `resolve_agent_id` path the write used.
fn inv_iso_list_args(category: &str) -> crate::builtin_tools::note_manage::NoteManageArgs {
    use crate::builtin_tools::note_manage::{NoteManageAction, NoteManageArgs};
    NoteManageArgs {
        action: NoteManageAction::List,
        category: Some(category.to_string()),
        filename: None,
        title: None,
        content: None,
        facts: None,
        links: None,
        tags: None,
        query: None,
        limit: None,
        new_title: None,
        relations: None,
        agent_id: None,
    }
}

/// `NoteManageArgs` for a `query` call (FTS fallback, no embedder wired).
fn inv_iso_query_args(query_text: &str) -> crate::builtin_tools::note_manage::NoteManageArgs {
    use crate::builtin_tools::note_manage::{NoteManageAction, NoteManageArgs};
    NoteManageArgs {
        action: NoteManageAction::Query,
        category: None,
        filename: None,
        title: None,
        content: None,
        facts: None,
        links: None,
        tags: None,
        query: Some(query_text.to_string()),
        limit: None,
        new_title: None,
        relations: None,
        agent_id: None,
    }
}

/// INV-ISO: two concurrent runs on DIFFERENT agents ("agent-a" / "agent-b")
/// must not cross-write memory. Both writes deliberately target the SAME
/// category+filename — if isolation ever broke (e.g. `resolve_agent_id`
/// started reading a shared/global "current agent" instead of the
/// per-task `TURN_CONTEXT`), the two concurrent writes would race for the
/// SAME file/index row, surfacing as a lost write or corrupted content
/// instead of hiding behind distinct names.
#[tokio::test]
async fn concurrent_runs_different_agents_do_not_cross_write_memory() {
    use crate::builtin_tools::note_manage::NoteManageTool;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::tools::turn_context::TURN_CONTEXT;
    use crate::tools::AlephTool;

    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    // One shared `NoteManageTool` instance (`Clone` is cheap — it wraps only
    // `Arc`s) — mirrors how the real tool registry hands the SAME tool
    // instance to every concurrent run; only `TURN_CONTEXT` differs per run.
    let tool = NoteManageTool::new(dir.path().join("note"), backend);
    let tool_a = tool.clone();
    let tool_b = tool.clone();

    let handle_a = tokio::spawn(async move {
        TURN_CONTEXT
            .scope(inv_iso_turn(SessionKey::main("agent-a")), async move {
                AlephTool::call(
                    &tool_a,
                    inv_iso_create_args("shared-name", "agent A's private content"),
                )
                .await
            })
            .await
    });
    let handle_b = tokio::spawn(async move {
        TURN_CONTEXT
            .scope(inv_iso_turn(SessionKey::main("agent-b")), async move {
                AlephTool::call(
                    &tool_b,
                    inv_iso_create_args("shared-name", "agent B's private content"),
                )
                .await
            })
            .await
    });

    let (result_a, result_b) = tokio::join!(handle_a, handle_b);
    let result_a = result_a
        .expect("agent-a task must not panic")
        .expect("agent-a create must succeed");
    let result_b = result_b
        .expect("agent-b task must not panic")
        .expect("agent-b create must succeed");
    assert!(result_a.success);
    assert!(result_b.success);

    // Black-box partition proof #1 (metadata): each agent's own `list` —
    // which resolves its `agent_id` from `TURN_CONTEXT` exactly like the
    // write did — must see EXACTLY its own note.
    let list_a = TURN_CONTEXT
        .scope(inv_iso_turn(SessionKey::main("agent-a")), async {
            AlephTool::call(&tool, inv_iso_list_args("learning")).await
        })
        .await
        .expect("agent-a list must succeed");
    let list_b = TURN_CONTEXT
        .scope(inv_iso_turn(SessionKey::main("agent-b")), async {
            AlephTool::call(&tool, inv_iso_list_args("learning")).await
        })
        .await
        .expect("agent-b list must succeed");
    let notes_a = list_a.notes.expect("agent-a list must return entries");
    let notes_b = list_b.notes.expect("agent-b list must return entries");
    assert_eq!(
        notes_a.len(),
        1,
        "agent-a partition must contain exactly its own note, got {notes_a:?}"
    );
    assert_eq!(
        notes_b.len(),
        1,
        "agent-b partition must contain exactly its own note, got {notes_b:?}"
    );

    // Black-box partition proof #2 (content): both notes' bodies share the
    // word "private" by design, so a search that ignored (or misresolved)
    // the agent partition would surface the OTHER agent's body too.
    let found_a = TURN_CONTEXT
        .scope(inv_iso_turn(SessionKey::main("agent-a")), async {
            AlephTool::call(&tool, inv_iso_query_args("private")).await
        })
        .await
        .expect("agent-a query must succeed");
    let body_a = found_a.content.expect("agent-a query must return content");
    assert!(body_a.contains("agent A's private content"));
    assert!(
        !body_a.contains("agent B's private content"),
        "agent A's query result must not contain agent B's content — cross-write, got:\n{body_a}"
    );
}

/// INV-ISO: two concurrent runs of the SAME agent ("agent-a"), different
/// conversations (conv-1 / conv-2), must not interleave each other's
/// `session_events` transcript, and both must land their memory write in
/// the shared agent-a partition without loss or deadlock.
///
/// Drives `InProcessActorSessionService` (`src/session/in_process.rs`) — the
/// production `SessionService` impl, which spawns one tokio actor task per
/// distinct `SessionKey` — concurrently for two sessions of one agent,
/// interleaved with real `note_manage` writes under each session's own
/// `TURN_CONTEXT`. See `second_message_same_session_takes_busy_path` above
/// for the exact hazard this guards against (INV-SEQ / audit 4.2): two runs
/// interleaving writes into the same transcript.
#[tokio::test]
async fn concurrent_runs_same_agent_do_not_interleave_transcript() {
    use crate::builtin_tools::note_manage::NoteManageTool;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::session::events::{now_ms, SessionEvent};
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};
    use crate::session::{InProcessActorSessionService, SessionEventStore, SessionService};
    use crate::tools::turn_context::TURN_CONTEXT;
    use crate::tools::AlephTool;

    // Real per-session actor backend — spawns ONE tokio task per distinct
    // `SessionKey` and persists to SQLite, the actual production transcript
    // path (not a stub).
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let event_store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session_svc = Arc::new(InProcessActorSessionService::new(event_store));

    // Real shared note backend for agent "A"'s single vault.
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    let tool = NoteManageTool::new(dir.path().join("note"), backend);

    let sid_1 = SessionKey::peer("agent-a", "conv-1");
    let sid_2 = SessionKey::peer("agent-a", "conv-2");
    assert_eq!(sid_1.agent_id(), "agent-a");
    assert_eq!(sid_2.agent_id(), "agent-a");
    assert_ne!(
        sid_1, sid_2,
        "the two conversations must be genuinely distinct sessions"
    );

    const EVENTS_PER_CONV: usize = 25;

    let svc_1 = session_svc.clone();
    let sid_1_task = sid_1.clone();
    let tool_1 = tool.clone();
    let handle_1 = tokio::spawn(async move {
        let turn = inv_iso_turn(sid_1_task.clone());
        TURN_CONTEXT
            .scope(turn, async move {
                for i in 0..EVENTS_PER_CONV {
                    svc_1
                        .emit_event(
                            &sid_1_task,
                            SessionEvent::SystemMessage {
                                turn_id: uuid::Uuid::new_v4(),
                                content: format!("conv1-{i}"),
                                at: now_ms(),
                            },
                        )
                        .await
                        .expect("conv-1 emit_event must succeed");
                    tokio::task::yield_now().await;
                }
                AlephTool::call(
                    &tool_1,
                    inv_iso_create_args("conv1-note", "written by conv-1"),
                )
                .await
            })
            .await
    });

    let svc_2 = session_svc.clone();
    let sid_2_task = sid_2.clone();
    let tool_2 = tool.clone();
    let handle_2 = tokio::spawn(async move {
        let turn = inv_iso_turn(sid_2_task.clone());
        TURN_CONTEXT
            .scope(turn, async move {
                for i in 0..EVENTS_PER_CONV {
                    svc_2
                        .emit_event(
                            &sid_2_task,
                            SessionEvent::SystemMessage {
                                turn_id: uuid::Uuid::new_v4(),
                                content: format!("conv2-{i}"),
                                at: now_ms(),
                            },
                        )
                        .await
                        .expect("conv-2 emit_event must succeed");
                    tokio::task::yield_now().await;
                }
                AlephTool::call(
                    &tool_2,
                    inv_iso_create_args("conv2-note", "written by conv-2"),
                )
                .await
            })
            .await
    });

    let (result_1, result_2) = tokio::join!(handle_1, handle_2);
    let result_1 = result_1
        .expect("conv-1 task must not panic")
        .expect("conv-1 note write must succeed — no loss, no deadlock");
    let result_2 = result_2
        .expect("conv-2 task must not panic")
        .expect("conv-2 note write must succeed — no loss, no deadlock");
    assert!(result_1.success);
    assert!(result_2.success);

    // --- Transcript non-interleave: each conversation's read-back must
    //     contain EXACTLY its own events, monotonic, none of the other's. ---
    let events_1 = session_svc.get_events(&sid_1, None, None).await.unwrap();
    let events_2 = session_svc.get_events(&sid_2, None, None).await.unwrap();

    assert_eq!(
        events_1.len(),
        EVENTS_PER_CONV,
        "conv-1 must not lose or gain events"
    );
    assert_eq!(
        events_2.len(),
        EVENTS_PER_CONV,
        "conv-2 must not lose or gain events"
    );

    for (i, rec) in events_1.iter().enumerate() {
        assert_eq!(
            rec.seq,
            (i + 1) as u64,
            "conv-1 seq must be monotonic starting at 1"
        );
        match &rec.event {
            SessionEvent::SystemMessage { content, .. } => {
                assert_eq!(
                    content,
                    &format!("conv1-{i}"),
                    "conv-1's transcript must contain ONLY conv-1's own events, in order — a \
                     mismatch here means conv-2's events leaked in (interleave regression)"
                );
            }
            other => panic!("unexpected event kind in conv-1 transcript: {other:?}"),
        }
    }
    for (i, rec) in events_2.iter().enumerate() {
        assert_eq!(
            rec.seq,
            (i + 1) as u64,
            "conv-2 seq must be monotonic starting at 1"
        );
        match &rec.event {
            SessionEvent::SystemMessage { content, .. } => {
                assert_eq!(
                    content,
                    &format!("conv2-{i}"),
                    "conv-2's transcript must contain ONLY conv-2's own events, in order"
                );
            }
            other => panic!("unexpected event kind in conv-2 transcript: {other:?}"),
        }
    }

    // --- Both notes land in the SHARED agent-a partition: no loss, no
    //     deadlock (both spawned tasks above already completed). ---
    let list_agent_a = TURN_CONTEXT
        .scope(inv_iso_turn(SessionKey::main("agent-a")), async {
            AlephTool::call(&tool, inv_iso_list_args("learning")).await
        })
        .await
        .expect("agent-a list must succeed");
    let filenames: Vec<String> = list_agent_a
        .notes
        .expect("agent-a list must return entries")
        .into_iter()
        .map(|n| n.filename)
        .collect();
    assert!(
        filenames.contains(&"conv1-note".to_string()),
        "conv-1's note must be present in agent-a's partition, got {filenames:?}"
    );
    assert!(
        filenames.contains(&"conv2-note".to_string()),
        "conv-2's note must be present in agent-a's partition, got {filenames:?}"
    );
}

// =============================================================================
// F1 — the slash-command fast path (L0) must not dispatch a gated tool
// =============================================================================
//
// `execute_direct_tool` calls `ToolRegistry::execute_tool` directly: no
// `ScopedToolService`, so no exec tier, no `[policies.tool_permissions]`, no
// operator gate and no approval card. The gate it now consults can only DECLINE
// (it has no approval transport), so a gated call must return `Fallthrough` and
// leave the registry untouched — the full agent loop then re-evaluates it with
// the real gates. These assert on the CALL COUNTER, because "did the tool run?"
// is the only question that matters here.

/// Counts `execute_tool` calls. `get_tool` answering `None` is faithful to the
/// gate under test: it never consults the registry for metadata.
struct CountingToolRegistry {
    calls: AtomicUsize,
}

impl CountingToolRegistry {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl crate::executor::ToolRegistry for CountingToolRegistry {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(serde_json::json!({"_display": "ran"})) })
    }
}

fn slash_engine(
    registry: Arc<CountingToolRegistry>,
) -> ExecutionEngine<crate::thinker::SingleProviderRegistry, CountingToolRegistry> {
    ExecutionEngine::new(
        ExecutionEngineConfig::default(),
        Arc::new(crate::thinker::SingleProviderRegistry::new(
            crate::providers::create_mock_provider(),
        )),
        registry,
        Vec::new(),
        None,
    )
}

/// A run carrying a resolved `/tool args` slash command, from `caller_role`.
/// `None` = the Panel / CLI / loopback operator (no channel role stamped).
fn slash_request(session_key: &SessionKey, caller_role: Option<&str>) -> RunRequest {
    let mut req = gate_test_request(session_key, "slash-run");
    if let Some(role) = caller_role {
        req.metadata
            .insert("caller_role".to_string(), role.to_string());
    }
    req
}

fn slash_mode(tool_id: &str, args: &str) -> String {
    serde_json::json!({"type": "direct_tool", "tool_id": tool_id, "args": args}).to_string()
}

/// The headline: a chat-tier channel (Telegram, `caller_role = "guest"`) sends
/// `/bash rm -rf ~`. Before the gate this reached `execute_tool` unchecked.
#[tokio::test]
async fn guest_slash_command_for_a_dangerous_tool_never_reaches_the_registry() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "slash-guest").await;
    let registry = Arc::new(CountingToolRegistry::new());
    let engine = slash_engine(Arc::clone(&registry));
    let emitter = Arc::new(TestEmitter::new());
    let session = SessionKey::main("slash-guest");

    for tool in ["bash", "code_exec", "file_write", "self_config"] {
        let request = slash_request(&session, Some("guest"));
        let err = engine
            .execute_slash_command_fast_path(
                "slash-run",
                &slash_mode(tool, "--cmd 'rm -rf ~'"),
                &request,
                Arc::clone(&agent),
                Arc::clone(&emitter),
            )
            .await
            .expect_err("a guest must not fast-path a dangerous tool");
        assert!(
            matches!(err, ExecutionError::Fallthrough { .. }),
            "`/{tool}` must fall through to the gated agent loop, got {err:?}"
        );
    }
    assert_eq!(
        registry.calls(),
        0,
        "no gated slash command may reach the raw registry"
    );
}

/// The operator gate (`method_authz`): a chat-tier channel cannot reconfigure
/// Aleph through a slash command either. `agent_delete` also declares
/// `requires_confirmation`, so it is gated for EVERY caller — see below.
#[tokio::test]
async fn guest_slash_command_for_an_operator_tool_falls_through() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "slash-operator-gate").await;
    let registry = Arc::new(CountingToolRegistry::new());
    let engine = slash_engine(Arc::clone(&registry));
    let emitter = Arc::new(TestEmitter::new());
    let session = SessionKey::main("slash-operator-gate");
    let request = slash_request(&session, Some("guest"));

    let err = engine
        .execute_slash_command_fast_path(
            "slash-run",
            &slash_mode("cron_manage", "--action delete --id nightly"),
            &request,
            agent,
            emitter,
        )
        .await
        .expect_err("cron_manage is an operator tool");
    assert!(matches!(err, ExecutionError::Fallthrough { .. }), "{err:?}");
    assert_eq!(registry.calls(), 0);
}

/// A tool that DECLARES `requires_confirmation` is gated at every tier and for
/// every caller — including the Panel operator on the default `Auto` tier. The
/// loop raises a card for it; the fast path cannot, so it must decline.
#[tokio::test]
async fn confirmation_gated_tool_falls_through_even_for_an_operator() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "slash-confirm").await;
    let registry = Arc::new(CountingToolRegistry::new());
    let engine = slash_engine(Arc::clone(&registry));
    let emitter = Arc::new(TestEmitter::new());
    let session = SessionKey::main("slash-confirm");

    for tool in ["agent_delete", "vault_store", "team_disband"] {
        let request = slash_request(&session, None);
        let err = engine
            .execute_slash_command_fast_path(
                "slash-run",
                &slash_mode(tool, "explore"),
                &request,
                Arc::clone(&agent),
                Arc::clone(&emitter),
            )
            .await
            .expect_err("a confirm-gated tool must not fast-path");
        assert!(matches!(err, ExecutionError::Fallthrough { .. }), "{err:?}");
    }
    assert_eq!(
        registry.calls(),
        0,
        "the fast path has no approval transport — it must decline, not skip the card"
    );
}

/// The tier's argument filter: `file_ops` hides `delete` behind the same tool
/// name as `list`, so the name-keyed rules cannot see it. Under the default
/// `Auto` tier the delete falls through and the list still fast-paths.
#[tokio::test]
async fn auto_tier_destructive_file_ops_argument_falls_through_but_a_read_does_not() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "slash-fileops").await;
    let registry = Arc::new(CountingToolRegistry::new());
    let engine = slash_engine(Arc::clone(&registry));
    let emitter = Arc::new(TestEmitter::new());
    let session = SessionKey::main("slash-fileops");

    let request = slash_request(&session, None);
    let err = engine
        .execute_slash_command_fast_path(
            "slash-run",
            &slash_mode("file_ops", "--operation delete --path /home/u/Documents"),
            &request,
            Arc::clone(&agent),
            Arc::clone(&emitter),
        )
        .await
        .expect_err("a destructive file_ops call asks under Auto");
    assert!(matches!(err, ExecutionError::Fallthrough { .. }), "{err:?}");
    assert_eq!(registry.calls(), 0);

    let request = slash_request(&session, None);
    engine
        .execute_slash_command_fast_path(
            "slash-run",
            &slash_mode("file_ops", "--operation list --path /tmp"),
            &request,
            agent,
            emitter,
        )
        .await
        .expect("a read-shaped file_ops call still takes the fast path");
    assert_eq!(
        registry.calls(),
        1,
        "the gate must not over-block: Auto allows a `list`"
    );
}

/// No regression for the Panel / CLI operator on the default tier: an ungated
/// tool still takes the fast path, with no LLM turn burned.
#[tokio::test]
async fn operator_slash_command_for_an_ungated_tool_still_fast_paths() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "slash-allow").await;
    let registry = Arc::new(CountingToolRegistry::new());
    let engine = slash_engine(Arc::clone(&registry));
    let emitter = Arc::new(TestEmitter::new());
    let session = SessionKey::main("slash-allow");

    // `search` is a declared pure read; `bash` is dangerous but the hard floor
    // is scoped to untrusted surfaces — a loopback operator is never restricted
    // by it, so `/bash` keeps its deterministic fast path.
    for tool in ["search", "bash"] {
        let request = slash_request(&session, None);
        let out = engine
            .execute_slash_command_fast_path(
                "slash-run",
                &slash_mode(tool, "hello"),
                &request,
                Arc::clone(&agent),
                Arc::clone(&emitter),
            )
            .await
            .unwrap_or_else(|e| panic!("`/{tool}` must fast-path for an operator, got {e:?}"));
        assert_eq!(out, "ran");
    }
    assert_eq!(registry.calls(), 2);
}

/// Captures the `RunRequest` a continuation is actually dispatched with, so the
/// inheritance contract is asserted on the real request the engine would run —
/// not on a reconstruction of it.
struct RecordingAdapter {
    requests: Arc<RwLock<Vec<RunRequest>>>,
}

impl RecordingAdapter {
    fn new() -> Self {
        Self {
            requests: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// The continuation is dispatched from a detached `tokio::spawn`, so poll
    /// (bounded) instead of racing it.
    async fn await_one(&self) -> Option<RunRequest> {
        for _ in 0..200 {
            if let Some(req) = self.requests.write().await.pop() {
                return Some(req);
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        None
    }
}

#[async_trait]
impl crate::gateway::execution_adapter::ExecutionAdapter for RecordingAdapter {
    async fn execute(
        &self,
        request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        self.requests.write().await.push(request);
        Ok(())
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        Err(ExecutionError::RunNotFound(run_id.to_string()))
    }

    async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
        None
    }

    async fn active_run_count(&self) -> usize {
        0
    }
}

/// A goal continuation spawned from a project-mode run must land in the SAME
/// project. `spawn_continuation_run` used to hardcode `workspace_override: None`
/// while every other sub-run producer (steering, team dispatch, session.send,
/// resume) inherited it — so an unattended continuation silently dropped the
/// project's CLAUDE.md / AGENTS.md and project skills, and its
/// workspace_directive told the model its cwd was the agent workspace. The root
/// is unrecoverable once lost: neither `goal` nor `looping` persists it.
#[tokio::test]
async fn goal_continuation_inherits_the_originating_runs_project_root() {
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::goal::{ContinuationDecision, Goal, PursuitMode};

    let temp = tempfile::tempdir().unwrap();
    let store = goal_store_global();

    let session = SessionKey::main("b10-continuation");
    let session_str = session.to_key_string();
    let project = temp.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();

    // Claim the continuation exactly as the post-run hook does: the claim is
    // what stamps the pending wake the fire-time gate checks.
    let now = 1_000u64;
    store
        .put(
            &Goal::new(&session_str, "keep going", 0, now)
                .with_pursuit(PursuitMode::Active { max_iterations: 5 }),
        )
        .unwrap();
    let ContinuationDecision::Fire {
        wake_ms, prompt, ..
    } = store
        .try_claim_continuation(&session_str, None, now, false, None)
        .unwrap()
    else {
        panic!("an Active goal with runway must claim a continuation");
    };

    let sessions: Arc<dyn crate::gateway::session_store::SessionStore> =
        test_session_manager(&temp);
    let agent = AgentInstance::new(
        AgentInstanceConfig {
            agent_id: session.agent_id().to_string(),
            workspace: temp.path().join("agent-workspace"),
            agent_dir: temp.path().join("agents"),
            ..Default::default()
        },
        Arc::clone(&sessions),
    )
    .unwrap();
    let registry = Arc::new(crate::gateway::agent_instance::AgentRegistry::new());
    registry.register(agent).await;

    let adapter = Arc::new(RecordingAdapter::new());
    super::execute::spawn_continuation_run(
        registry,
        Arc::clone(&adapter) as Arc<dyn ExecutionAdapter>,
        session.clone(),
        session_str,
        prompt,
        HashMap::new(),
        Some(project.clone()),
        None,
        // The real store, not `None`: this argument is what the agent-miss
        // branch resolves an origin from, and a test that passes `None` cannot
        // tell a wired handle from an unwired one.
        Some(sessions),
        None,
        super::execute::ContinuationKind::Goal { wake_ms },
    );

    let request = adapter
        .await_one()
        .await
        .expect("the claimed continuation must reach the execution adapter");
    assert_eq!(
        request.workspace_override,
        Some(project),
        "the continuation must run inside the project the goal was set in"
    );
}

// The goal store is process-global and `sweep_once` scans EVERY row in it, so
// two tests that each park a goal into the sweep's claim shape will consume one
// another's barrier: the sweep clears it and claims the continuation FIRST, and
// only then discovers the agent belongs to the other test's registry — by which
// point the wake is already spent and unrecoverable. Serialize the sweeping
// tests rather than hoping their windows miss.
static GOAL_SWEEP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A transient provider failure parks the pursuit and pushes "retrying in ~N" —
/// and something has to make that retry happen.
///
/// Nothing did. The park is written from the FAILURE arm of `adapter.execute`,
/// while `post_run` — the only producer that turns a timer barrier into an armed
/// `tokio` sleep — runs on the `Ok` arm alone. The park also clears
/// `pending_continuation_ms`, so no in-flight timer survives it, and the
/// periodic sweep filtered `waiting_on_task.is_some()`, so timer barriers were
/// invisible to it. The goal then sat `Active`-and-parked until someone typed in
/// that session or the daemon restarted; for an unattended pursuit neither is
/// coming, and the push the user is holding is a lie.
///
/// So this asserts the WAKE — a real run reaching the execution adapter with the
/// resume prompt — not the parked row. A test that stopped at "parked, weld
/// preserved, not Blocked" passes with the bug fully present.
#[tokio::test]
async fn a_transiently_parked_goal_is_actually_woken() {
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::goal::{ContinuationDecision, Goal, GoalStatus, PursuitMode};

    let _sweep_guard = GOAL_SWEEP_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let store = goal_store_global();

    let session = SessionKey::main("b10-transient-park");
    let session_str = session.to_key_string();
    // Real wall clock: the park, the sweep and `confirm_fire` all read
    // `now_ms()`, so the row has to live in that coordinate system.
    let now = super::goal_continuation::now_ms();
    store
        .put(
            &Goal::new(&session_str, "keep going", 0, now)
                .with_pursuit(PursuitMode::Active { max_iterations: 5 }),
        )
        .unwrap();
    // Claim one continuation exactly as the post-run hook does — the run that is
    // about to fail is a claimed one, and its marker is what the park clears.
    let ContinuationDecision::Fire { .. } = store
        .try_claim_continuation(&session_str, None, now, false, None)
        .unwrap()
    else {
        panic!("an Active goal with runway must claim a continuation");
    };

    // That run fails with a 429 carrying a 1-second Retry-After — the shortest
    // park `bound_transient_park_delay_ms` allows, so the sweep below runs
    // against a genuinely elapsed barrier instead of a hand-written row.
    super::goal_continuation::block_goal_on_failure(
        &session_str,
        &ExecutionError::Failed(
            "provider x: Rate limit error: 429 rate limited; retry after 1 seconds".to_string(),
        ),
        None,
    )
    .await;
    let parked = store.get(&session_str).unwrap().unwrap();
    assert_eq!(
        parked.status,
        GoalStatus::Active,
        "a transient failure parks, it does not judge"
    );
    assert!(
        parked.waiting_until_ms.is_some(),
        "the park is a timer barrier"
    );
    assert_eq!(
        parked.pending_continuation_ms, None,
        "the failed run's marker is cleared — nothing is in flight to wake this"
    );

    let sessions: Arc<dyn crate::gateway::session_store::SessionStore> =
        test_session_manager(&temp);
    let agent = AgentInstance::new(
        AgentInstanceConfig {
            agent_id: session.agent_id().to_string(),
            workspace: temp.path().join("agent-workspace"),
            agent_dir: temp.path().join("agents"),
            ..Default::default()
        },
        Arc::clone(&sessions),
    )
    .unwrap();
    let registry = Arc::new(crate::gateway::agent_instance::AgentRegistry::new());
    registry.register(agent).await;
    let adapter = Arc::new(RecordingAdapter::new());
    let wake = Arc::new(super::goal_wait::GoalWakeService::new(
        ContinuationDeps {
            registry,
            adapter: Arc::clone(&adapter) as Arc<dyn ExecutionAdapter>,
            gate: None,
            event_bus: None,
        },
        None,
        Some(sessions),
    ));

    // Let the park elapse, then run exactly one sweep — the tick loop's body.
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    wake.sweep_once().await;

    let request = adapter
        .await_one()
        .await
        .expect("the parked pursuit must be woken, not left Active-and-parked forever");
    assert!(
        request.input.contains("Resuming your standing goal"),
        "the wake must say WHY it fired (R9); got: {}",
        request.input
    );
    let woken = store.get(&session_str).unwrap().unwrap();
    assert!(
        !woken.has_wait_barrier(),
        "the wake consumes the barrier it fired on"
    );
    assert_eq!(
        woken.continuations_used, 2,
        "the wake claims through the normal pipeline, so it spends an iteration \
         like every other autonomous step"
    );
}

/// G11 — a spend-ceiling denial must `Blocked` a pursuit, not park it on the
/// wait barrier the sweep above wakes: the ceiling does not lift until the
/// period resets, so a park here would retry the same denial forever,
/// costing no iteration each time (see `ReceiptKind::is_transient`'s doc).
#[tokio::test]
async fn a_spend_exhausted_failure_blocks_the_goal_instead_of_parking_it() {
    use crate::goal::{ContinuationDecision, Goal, GoalStatus, PursuitMode};

    let _sweep_guard = GOAL_SWEEP_LOCK.lock().await;
    let store = goal_store_global();

    let session = SessionKey::main("g11-spend-exhausted-blocks");
    let session_str = session.to_key_string();
    let now = super::goal_continuation::now_ms();
    store
        .put(
            &Goal::new(&session_str, "keep going", 0, now)
                .with_pursuit(PursuitMode::Active { max_iterations: 5 }),
        )
        .unwrap();
    let ContinuationDecision::Fire { .. } = store
        .try_claim_continuation(&session_str, None, now, false, None)
        .unwrap()
    else {
        panic!("an Active goal with runway must claim a continuation");
    };

    super::goal_continuation::block_goal_on_failure(
        &session_str,
        &ExecutionError::SpendExhausted {
            limit: crate::spend::Limit::Total,
        },
        None,
    )
    .await;

    let blocked = store.get(&session_str).unwrap().unwrap();
    assert_eq!(
        blocked.status,
        GoalStatus::Blocked,
        "a spend denial must block the pursuit, not park it for retry"
    );
    assert!(
        blocked.waiting_until_ms.is_none(),
        "a blocked goal carries no timer barrier — nothing should auto-wake it"
    );
}

/// The sweep that fixed the un-woken transient park (above) claims an elapsed
/// timer barrier carrying no pending marker. A FIRED timer wake reads as exactly
/// that shape: `confirm_fire`'s `Proceed` arm consumed the marker, and the
/// barrier it fired on stayed behind until the next claim — for the whole
/// duration of the woken run, which for an autonomous turn routinely outlasts
/// the 60s sweep interval.
///
/// So the sweep spawned a SECOND continuation for a goal that was already
/// running: one more iteration off the R5 cap, an `AgentBusy` collision plus its
/// re-arm retries, and a "Resuming your standing goal — the wait elapsed" prompt
/// for a pursuit that is not parked.
///
/// This asserts the sweep claims NOTHING while that run is in flight — the
/// defect's observable — not merely that the row lost a field. The registered
/// agent is load-bearing: an unregistered one makes `spawn_wake_run` drop every
/// wake, and the assertion would then hold with the defect fully present.
#[tokio::test]
async fn a_fired_timer_wake_leaves_nothing_for_the_sweep_to_claim() {
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::goal::{ContinuationDecision, FireDecision, Goal, PursuitMode};

    let _sweep_guard = GOAL_SWEEP_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let store = goal_store_global();

    let session = SessionKey::main("b10-fired-wake");
    let session_str = session.to_key_string();
    // Real wall clock: the claim, `confirm_fire` and the sweep all read
    // `now_ms()`, so the row has to live in that coordinate system.
    let now = super::goal_continuation::now_ms();
    // The designed park — `goal(update, wait_minutes=…)` — one second out, so
    // the claim below arms it while it is still in the future (the claim's lazy
    // self-clear only drops an already-elapsed barrier).
    store
        .put(
            &Goal::new(&session_str, "keep going", 0, now)
                .with_pursuit(PursuitMode::Active { max_iterations: 5 })
                .with_wait_until(now + 1_000, Some("cooldown".into()), now),
        )
        .unwrap();
    let ContinuationDecision::Fire { wake_ms, .. } = store
        .try_claim_continuation(&session_str, None, now, false, None)
        .unwrap()
    else {
        panic!("a parked timer barrier must claim its wake");
    };

    let sessions: Arc<dyn crate::gateway::session_store::SessionStore> =
        test_session_manager(&temp);
    let agent = AgentInstance::new(
        AgentInstanceConfig {
            agent_id: session.agent_id().to_string(),
            workspace: temp.path().join("agent-workspace"),
            agent_dir: temp.path().join("agents"),
            ..Default::default()
        },
        Arc::clone(&sessions),
    )
    .unwrap();
    let registry = Arc::new(crate::gateway::agent_instance::AgentRegistry::new());
    registry.register(agent).await;
    let adapter = Arc::new(RecordingAdapter::new());
    let wake = Arc::new(super::goal_wait::GoalWakeService::new(
        ContinuationDeps {
            registry,
            adapter: Arc::clone(&adapter) as Arc<dyn ExecutionAdapter>,
            gate: None,
            event_bus: None,
        },
        None,
        Some(sessions),
    ));

    // The timer elapses and fires: this is the instant the woken run starts.
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert_eq!(
        store
            .confirm_fire(&session_str, wake_ms, super::goal_continuation::now_ms())
            .unwrap(),
        FireDecision::Proceed,
        "the armed timer's own fire must be confirmed"
    );
    let spent = store.get(&session_str).unwrap().unwrap().continuations_used;

    // A sweep tick lands while that woken run is still executing.
    wake.sweep_once().await;

    assert!(
        adapter.await_one().await.is_none(),
        "the sweep must not dispatch a second continuation for a goal whose wake \
         is already running"
    );
    assert_eq!(
        store.get(&session_str).unwrap().unwrap().continuations_used,
        spent,
        "and it must not spend another iteration off the pursuit cap"
    );
}

/// Regression pin: `execute()` announces the turn's end on **both** of its
/// terminal arms.
///
/// Until 2026-08-10 only the `Ok` arm published `SessionUpdated`. A run that
/// failed, timed out or was cancelled therefore ended in silence — while the
/// transcript HAD moved: the harness appends the user message before dispatch
/// and the error receipt is persisted after. Every surface that re-hydrates on
/// that frame (a second Panel tab, another member of a project room, the
/// sidebar row's `updated_at`) kept rendering the state from before the failed
/// turn until the viewer manually reselected the session.
///
/// A source-level pin because the real `ExecutionEngine::execute` needs a live
/// orchestrator to reach either arm, so there is no unit-level harness that
/// distinguishes "announced once" from "announced twice"; without this,
/// deleting the `Err` arm's call leaves every test green again. The needle is
/// the shared helper (`Engine::announce_turn_end`) rather than
/// `publish_session_updated`, because the whole point of the helper is that the
/// three arguments — including the `metadata["channel_id"]` lookup — are
/// derived in exactly one place.
///
/// CRLF-safe: this repository is checked out with CRLF on Windows, so the file
/// is normalised before any `\n`-anchored splitting. See the "source-level
/// guards" criterion in CLAUDE.md §10 — a separator with a character in front
/// of its `\n` matches nothing on a CRLF checkout, and the resulting "whole
/// file" prefix then happily matches literals inside this very test.
#[test]
fn execute_announces_the_turn_end_on_both_terminal_arms() {
    let src = include_str!("execute.rs").replace('\r', "");
    let production = src
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one element");
    assert!(
        production.len() > 1000,
        "the production prefix must be the bulk of execute.rs, not an empty \
         slice — a mis-split would make every assertion below vacuous"
    );
    let announcements = production
        .matches("self.announce_turn_end(&request)")
        .count();
    assert_eq!(
        announcements, 2,
        "execute() has two terminal arms (Ok and Err) and both must announce \
         the turn's end; found {announcements} call site(s)"
    );

    // …and specifically that one of them is on the failure path. Counting
    // alone would pass if somebody duplicated the call inside the success arm.
    let err_arm = production
        .find("failed to emit RunError stream event")
        .expect("the Err arm still emits a RunError stream event");
    assert!(
        production[err_arm..].contains("self.announce_turn_end(&request)"),
        "the announcement must also happen after the failure receipt is \
         emitted — a failed or cancelled turn moved the transcript too"
    );
}
