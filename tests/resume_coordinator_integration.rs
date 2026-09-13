//! Integration tests for the mid-run trajectory resume boot scan.
//!
//! Spec: docs/superpowers/specs/2026-05-21-mid-run-trajectory-resume-design.md §7.

// test-only tuple return type reads clearer inline.
#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use alephcore::gateway::agent_instance::AgentInstance;
use alephcore::gateway::agent_instance::AgentRegistry;
use alephcore::gateway::event_emitter::EventEmitter;
use alephcore::gateway::execution_adapter::ExecutionAdapter;
use alephcore::gateway::execution_engine::{ExecutionError, RunRequest, RunStatus};
use alephcore::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::ResumeCoordinator;
use alephcore::routing::session_key::SessionKey;
use alephcore::session::events::{
    now_ms, Durability, EventSeq, Retire, RunOutcome, SessionEvent, TurnId,
};
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::session::SessionError;
use alephcore::ResumeConfig;

/// Mock `ExecutionAdapter` that records every `execute` call's
/// `(session_key, metadata, model_override)` so the test can assert resume
/// signalling.
///
/// The third element is ④'s carrier: the crashed run's model pin rides on
/// `RunRequest.model_override` and NOT on metadata, precisely because the
/// override governs this run only and never writes back to the session row.
/// Asserting it from the metadata map would therefore pass on a resume that
/// pinned nothing.
struct RecordingAdapter {
    calls: Arc<Mutex<Vec<Call>>>,
}

type Call = (
    String,
    HashMap<String, String>,
    Option<alephcore::gateway::model_override::ModelOverride>,
);

impl RecordingAdapter {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ExecutionAdapter for RecordingAdapter {
    async fn execute(
        &self,
        request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        self.calls.lock().await.push((
            request.session_key.to_key_string(),
            request.metadata.clone(),
            request.model_override.clone(),
        ));
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

/// Build an `AgentRegistry` containing one agent whose id matches the
/// `SessionKey` under test, so `retrigger`'s `registry.get(agent_id)`
/// resolves.
async fn registry_with_agent(agent_id: &str) -> Arc<AgentRegistry> {
    use alephcore::gateway::agent_instance::AgentInstanceConfig;
    use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};

    let temp = tempfile::tempdir().unwrap();
    let sm = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        })
        .expect("session manager"),
    );
    let cfg = AgentInstanceConfig {
        agent_id: agent_id.to_string(),
        workspace: temp.path().join("ws"),
        agent_dir: temp.path().join("agents").join(agent_id),
        ..Default::default()
    };
    // `AgentRegistry::register` takes `AgentInstance` BY VALUE (not `Arc`)
    // and is `async` (verified: agent_instance.rs:551). `get` then returns
    // `Arc<AgentInstance>`.
    let agent = AgentInstance::new(cfg, sm).unwrap();
    let registry = Arc::new(AgentRegistry::new());
    registry.register(agent).await;
    // The dirs must outlive the registry, which outlives this frame. Registered
    // for removal at process exit instead of abandoned: this helper is called
    // once per test, so `mem::forget` here left 14 trees behind every run.
    let _ = alephcore::utils::scratch::keep_until_exit(temp);
    registry
}

fn store() -> Arc<dyn SessionEventStore> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    Arc::new(SqliteEventStore::new(conn))
}

/// A real `SessionStore` in its own directory, so parallel tests in this
/// binary cannot see each other's session rows.
///
/// The coordinator reads the resumed session's persisted owner/scope from
/// here. A test that seeds no row gets the legacy/pre-P1 shape — no row, no
/// scope stamp, resume behaves exactly as it did before P1.
fn sessions() -> Arc<dyn SessionStore> {
    // A `OnceLock<TempDir>` would never drop — statics don't — so the root is
    // registered for removal at process exit instead.
    static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let base = ROOT
        .get_or_init(|| {
            alephcore::utils::scratch::keep_until_exit(tempfile::tempdir().expect("tempdir"))
        })
        .join(format!("sessions-{n}"));
    std::fs::create_dir_all(&base).expect("session dir");
    Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: base,
            ..Default::default()
        })
        .expect("file session store"),
    )
}

/// Process-global goal store shared by every test in this binary —
/// `goal::init_global` is a first-set-wins `OnceCell`, so tests must share
/// one store and distinguish themselves by unique session keys.
fn shared_goal_store() -> Arc<alephcore::goal::GoalStore> {
    static STORE: std::sync::OnceLock<Arc<alephcore::goal::GoalStore>> = std::sync::OnceLock::new();
    STORE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let store =
                Arc::new(alephcore::goal::GoalStore::open(&dir.path().join("goals.db")).unwrap());
            // The db file must outlive every test in the binary — but not the
            // binary itself.
            let _ = alephcore::utils::scratch::keep_until_exit(dir);
            alephcore::goal::init_global(store.clone());
            store
        })
        .clone()
}

/// Seed a complete interrupted run: user message, a turn, a dangling tool
/// call, then a trailing `RunStarted` with no `RunFinished`.
///
/// The marker carries NO envelope, which is both the pre-④ shape every older
/// log has and the shape the coordinator must keep resuming unchanged.
async fn seed_interrupted_run(store: &Arc<dyn SessionEventStore>, sid: &SessionKey) {
    seed_interrupted_run_with_envelope(store, sid, None).await;
}

/// The same crash, with ④'s knob envelope frozen onto the `RunStarted` marker
/// the way `harness_bridge::runner_impl` writes it for every run since.
async fn seed_interrupted_run_with_envelope(
    store: &Arc<dyn SessionEventStore>,
    sid: &SessionKey,
    envelope: Option<alephcore::session::events::RunEnvelopeSnapshot>,
) {
    let tid = TurnId::new_v4();
    let at = now_ms();
    let events: Vec<SessionEvent> = vec![
        SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: alephcore::session::events::TurnTrigger::UserMessage,
            at,
        },
        SessionEvent::UserMessage {
            turn_id: tid,
            content: alephcore::session::events::MessageContent {
                text: "do a long task".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: at + 1,
            synthetic: false,
            author_user_id: None,
        },
        SessionEvent::RunStarted {
            run_id: "run-1".into(),
            at: at + 2,
            project_root: None,
            envelope,
        },
        SessionEvent::ToolCallRequested {
            turn_id: tid,
            call_id: "dangling-1".into(),
            name: "bash_exec".into(),
            input: serde_json::json!({"cmd": "sleep 999"}),
            at: at + 3,
        },
        // <-- process dies here: no ToolResult, no RunFinished.
    ];
    for (i, ev) in events.into_iter().enumerate() {
        store
            .append(sid, (i as u64) + 1, &ev, now_ms())
            .await
            .unwrap();
    }
}

/// I2: a resumed run in a project room must reach the engine carrying the
/// ROOM's scope, not just its folder.
///
/// Driven through the whole production path — a real session row stamped by
/// `get_or_create` under the room's ambient scope, the real boot scan, and the
/// metadata the `ExecutionAdapter` actually receives — because the defect was
/// precisely that `retrigger` built its metadata without ever consulting the
/// row. `run_loop::with_request_scope` reads this map and nothing else, and
/// `scope_from_metadata` is fail-closed: an unstamped resume runs unscoped and
/// writes the room's memory to the base partition, which is org-tier and shared
/// with every user.
#[tokio::test]
async fn a_resumed_room_run_reaches_the_engine_with_the_rooms_scope() {
    let store = store();
    let sid = SessionKey::main("resume-scope");
    seed_interrupted_run(&store, &sid).await;

    // The durable row the coordinator has to rehydrate from, written the way
    // production writes it: `get_or_create` stamps whatever scope is ambient.
    let sessions = sessions();
    alephcore::scope::with_scope(
        Some(alephcore::scope::ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: alephcore::scope::ScopeId::Project("p-standup".to_string()),
        }),
        sessions.get_or_create(&sid),
    )
    .await
    .expect("session row");

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions,
        test_bus(),
    );
    assert_eq!(coordinator.resume_interrupted_runs().await.resumed, 1);

    let calls = calls.lock().await;
    let (_key, metadata, _model) = calls.first().expect("the resumed run reached the adapter");
    // Assert through the consumer, not the raw keys: this is the exact call
    // `with_request_scope` makes on the way into the run.
    let scope = alephcore::scope::scope_from_metadata(metadata)
        .expect("a resumed room run must carry a scope");
    assert_eq!(
        scope.scope,
        alephcore::scope::ScopeId::Project("p-standup".to_string()),
        "the resumed run must stay in the room, not fall back to the org partition"
    );
    assert_eq!(scope.owner_user_id, "u-alice");
}

#[tokio::test]
async fn interrupted_run_is_repaired_and_retriggered() {
    let store = store();
    let sid = SessionKey::main("main");
    seed_interrupted_run(&store, &sid).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;

    assert_eq!(report.scanned, 1);
    assert_eq!(report.resumed, 1);
    assert_eq!(report.abandoned, 0);
    assert_eq!(report.skipped, 0);

    // The crash boundary was repaired: a synthetic ToolError for the
    // dangling call was appended to the log.
    let all = store.load_all_events(&sid).await.unwrap();
    let synthetic_errors: Vec<_> = all
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolError { call_id, error, .. } => {
                Some((call_id.clone(), error.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(synthetic_errors.len(), 1);
    assert_eq!(synthetic_errors[0].0, "dangling-1");
    // The repair reports an unknown outcome, not a failure: `sleep 999` may
    // have run, and a text that reads as "it failed" invites the model to run
    // it again. It must also name the tool so the model knows what to check.
    let repair = &synthetic_errors[0].1;
    assert!(
        repair.contains("OUTCOME UNKNOWN"),
        "expected an unknown-outcome repair, got: {repair}"
    );
    assert!(
        repair.contains("bash_exec"),
        "repair must name the dispatched tool, got: {repair}"
    );

    // `execute` was called exactly once, carrying the resume signal.
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, sid.to_key_string());
    assert_eq!(calls[0].1.get("resume").map(String::as_str), Some("true"));
}

/// ④ end to end: the knobs frozen on the crashed run's `RunStarted` reach the
/// dispatched request, and each one reaches it on the carrier that cannot make
/// the resume louder than the run it is replaying.
///
/// Every unit test underneath this builds `RunStartFacts` by hand, so all of
/// them stay green if `retrigger` drops the plan on the floor. This asserts at
/// the consumer end — the `RunRequest` an `ExecutionAdapter` actually receives.
///
/// Two carriers, deliberately different:
/// * the model pin rides `RunRequest.model_override`, which governs this run
///   only. On metadata it would be a session-level fact and the crashed run's
///   model would outlive the resume.
/// * the exec tier rides `RESUME_TIER_CEILING_KEY`, NOT the request-rung
///   `exec_tier` key. The request rung outranks session and global, so a `full`
///   snapshot arriving there would RAISE a conversation the operator had since
///   tightened. The ceiling is composed through `most_restrictive` after all
///   three rungs resolve, so it can only tighten — and this test pins the
///   negative half by name.
#[tokio::test]
async fn a_resume_replays_the_crashed_runs_envelope_on_carriers_that_cannot_raise_it() {
    use alephcore::agents::thinking::{ThinkLevel, THINK_LEVEL_SESSION_KEY};
    use alephcore::gateway::execution_engine::RESUME_TIER_CEILING_KEY;
    use alephcore::memory::session_memory_mode::{MemoryMode, MEMORY_MODE_SESSION_KEY};
    use alephcore::orchestrator::{ExecTier, SessionMode, EXEC_TIER_SESSION_KEY, MODE_SESSION_KEY};

    let store = store();
    let sid = SessionKey::main("envelope-replay");
    seed_interrupted_run_with_envelope(
        &store,
        &sid,
        Some(alephcore::session::events::RunEnvelopeSnapshot {
            exec_tier: Some(ExecTier::Ask.id().to_string()),
            session_mode: Some(SessionMode::Code.id().to_string()),
            think_level: Some(ThinkLevel::High.id().to_string()),
            memory_mode: Some(MemoryMode::Off.id().to_string()),
            model: Some("aleph-test-model".to_string()),
            model_provider: Some("openai".to_string()),
            allowed_tools: None,
            btw: None,
        }),
    )
    .await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;
    assert_eq!(report.resumed, 1);
    assert_eq!(
        report.unsnapshotted, 0,
        "this marker carries an envelope; `unsnapshotted` counts the ones that do not"
    );

    let calls = calls.lock().await;
    let (_key, metadata, model_override) =
        calls.first().expect("the resumed run reached the adapter");

    assert_eq!(
        metadata.get(MODE_SESSION_KEY).map(String::as_str),
        Some(SessionMode::Code.id())
    );
    assert_eq!(
        metadata.get(THINK_LEVEL_SESSION_KEY).map(String::as_str),
        Some(ThinkLevel::High.id())
    );
    assert_eq!(
        metadata.get(MEMORY_MODE_SESSION_KEY).map(String::as_str),
        Some(MemoryMode::Off.id())
    );
    assert_eq!(
        metadata.get(RESUME_TIER_CEILING_KEY).map(String::as_str),
        Some(ExecTier::Ask.id()),
        "the snapshot tier must arrive as a ceiling"
    );
    assert_eq!(
        metadata.get(EXEC_TIER_SESSION_KEY),
        None,
        "and never on the request rung, which outranks session and global"
    );

    match model_override {
        Some(alephcore::gateway::model_override::ModelOverride::Qualified { provider, model }) => {
            assert_eq!(provider, "openai");
            assert_eq!(model, "aleph-test-model");
        }
        other => panic!("expected the snapshot's qualified pin, got {other:?}"),
    }
}

/// The on-demand face does the same work as the boot scan, on one session.
#[tokio::test]
async fn on_demand_resume_repairs_and_retriggers_the_named_session() {
    let store = store();
    let target = SessionKey::main("main");
    let bystander = SessionKey::main("other");
    seed_interrupted_run(&store, &target).await;
    seed_interrupted_run(&store, &bystander).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(target.agent_id()).await;

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator
        .resume_session(&target)
        .await
        .expect("resume ok");

    assert_eq!(report.scanned, 1, "only the named session is scanned");
    assert_eq!(report.resumed, 1);

    // Named-session scope: the equally-interrupted bystander is untouched. A
    // per-session verb that quietly resumed the whole database would be a very
    // expensive surprise on a machine with hundreds of sessions.
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, target.to_key_string());
    assert_eq!(calls[0].1.get("resume").map(String::as_str), Some("true"));

    // Same derivation as boot: the boundary repair ran here too.
    let all = store.load_all_events(&target).await.unwrap();
    assert!(
        all.iter().any(|r| matches!(
            &r.event,
            SessionEvent::ToolError { error, .. } if error.contains("OUTCOME UNKNOWN")
        )),
        "on-demand resume must repair the crash boundary, not just re-trigger"
    );
    let bystander_events = store.load_all_events(&bystander).await.unwrap();
    assert!(
        !bystander_events
            .iter()
            .any(|r| matches!(&r.event, SessionEvent::ToolError { .. })),
        "the bystander session must not be repaired by a resume aimed elsewhere"
    );
}

/// Two resumes of one session must never both repair its crash boundary.
///
/// `repair_boundary` is a read-then-append, so two winners append the same
/// synthetic `ToolError` twice and the session ends up with one `call_id`
/// answered by two `tool_result`s — which the provider rejects on every
/// subsequent turn. The boot scan never exposed this (sequential loop); the
/// on-demand face does, including against the boot scan itself.
///
/// The assertion is the invariant, not the lock: **exactly one** repair event,
/// whichever way the two futures interleave. If they serialize instead of
/// racing, the second one's `repairs_for(&reduce_run(..))` sees the first
/// repair and produces nothing — so this holds either way, with no sleep and
/// no ordering assumption.
#[tokio::test]
async fn concurrent_resumes_of_one_session_repair_the_boundary_once() {
    let store = store();
    let sid = SessionKey::main("main");
    seed_interrupted_run(&store, &sid).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = Arc::new(ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    ));

    let (a, b) = tokio::join!(
        {
            let c = coordinator.clone();
            let sid = sid.clone();
            async move { c.resume_session(&sid).await.expect("resume ok") }
        },
        {
            let c = coordinator.clone();
            let sid = sid.clone();
            async move { c.resume_session(&sid).await.expect("resume ok") }
        }
    );

    let repairs = store
        .load_all_events(&sid)
        .await
        .unwrap()
        .iter()
        .filter(|r| matches!(&r.event, SessionEvent::ToolError { .. }))
        .count();
    assert_eq!(
        repairs, 1,
        "the crash boundary must be repaired exactly once, got {repairs} \
         (reports: {a:?} / {b:?})"
    );
}

/// A clean session answers "nothing to resume" rather than erroring or
/// re-running its last completed turn.
#[tokio::test]
async fn on_demand_resume_of_a_finished_session_is_a_no_op() {
    let store = store();
    let sid = SessionKey::main("main");
    let at = now_ms();
    for (i, ev) in [
        SessionEvent::RunStarted {
            run_id: "run-1".into(),
            at,
            project_root: None,
            envelope: None,
        },
        SessionEvent::RunFinished {
            run_id: "run-1".into(),
            outcome: RunOutcome::Completed,
            at: at + 1,
        },
    ]
    .into_iter()
    .enumerate()
    {
        store
            .append(&sid, (i as u64) + 1, &ev, now_ms())
            .await
            .unwrap();
    }

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );

    let report = coordinator.resume_session(&sid).await.expect("resume ok");
    assert_eq!(report.scanned, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.resumed, 0);
    assert!(
        calls.lock().await.is_empty(),
        "must not re-run a finished run"
    );
}

/// A session with no run markers at all is an answer, not an error — and it is
/// distinguishable from "already finished" by `scanned == 0`.
#[tokio::test]
async fn on_demand_resume_of_an_unknown_session_reports_no_runs() {
    let store = store();
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let sid = SessionKey::main("never-ran");
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );

    let report = coordinator.resume_session(&sid).await.expect("resume ok");
    assert_eq!(report, Default::default(), "zero report, not an error");
    assert!(calls.lock().await.is_empty());
}

/// `[resume] enabled = false` switches off the *automatic* scan. An explicit
/// request is a decision the operator has already made, and silently ignoring
/// it is the kind of no-op that reads as a broken feature.
#[tokio::test]
async fn on_demand_resume_works_when_the_boot_scan_is_disabled() {
    let store = store();
    let sid = SessionKey::main("main");
    seed_interrupted_run(&store, &sid).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig {
            enabled: false,
            ..ResumeConfig::default()
        },
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );

    // The scan stays off...
    assert_eq!(
        coordinator.resume_interrupted_runs().await,
        Default::default()
    );
    assert!(calls.lock().await.is_empty());

    // ...and the explicit verb still works.
    let report = coordinator.resume_session(&sid).await.expect("resume ok");
    assert_eq!(report.resumed, 1);
    assert_eq!(calls.lock().await.len(), 1);
}

#[tokio::test]
async fn disabled_config_never_triggers_execute() {
    let store = store();
    let sid = SessionKey::main("main");
    seed_interrupted_run(&store, &sid).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    // `resume_interrupted_runs` self-guards on `config.enabled`: even
    // when called directly it must scan nothing and trigger nothing.
    let cfg = ResumeConfig {
        enabled: false,
        ..ResumeConfig::default()
    };
    let coordinator = ResumeCoordinator::new(
        store.clone(),
        cfg,
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;

    assert_eq!(report, alephcore::gateway::ResumeReport::default());
    assert!(
        calls.lock().await.is_empty(),
        "disabled coordinator must never call execute"
    );
}

/// Seed the state the coordinator itself produces after `n` resumes that each
/// crashed before the resumed run's `RunStarted`: one open run, then `n`
/// `ResumeAttempted` stamps naming it. This is the shape production writes;
/// three bare `RunStarted` (the old fixture) is one it never does, and under
/// the intent-side ratchet that shape reads `attempts: 0` and would retrigger.
async fn seed_crash_looped_run(store: &Arc<dyn SessionEventStore>, sid: &SessionKey, n: u32) {
    let at = now_ms();
    store
        .append(
            sid,
            1,
            &SessionEvent::RunStarted {
                run_id: "r1".into(),
                at,
                project_root: None,
                envelope: None,
            },
            at,
        )
        .await
        .unwrap();
    for attempt in 1..=n {
        store
            .append(
                sid,
                1 + u64::from(attempt),
                &SessionEvent::ResumeAttempted { target: 1, attempt },
                at + i64::from(attempt),
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn crash_loop_cap_abandons_instead_of_retriggering() {
    let store = store();
    // Unique agent/session key: the goal store is process-global in this
    // test binary, so each abandon-path test owns its own session.
    let sid = SessionKey::main("cap-agent");
    // One RunStarted + 3 intent stamps == default max_attempts.
    seed_crash_looped_run(&store, &sid, 3).await;

    // Active goal in the session — its crash recovery hangs entirely on the
    // coordinator's retrigger→post_run chain, so abandoning must block it
    // honestly instead of leaving it lying "Active" in goal(list) forever.
    // Active-pursuit goal: its crash recovery hangs on the coordinator's
    // retrigger→post_run chain. A passive goal (seeded below) must NOT be
    // collateral-blocked.
    let goals = shared_goal_store();
    goals
        .put(
            &alephcore::goal::Goal::new(&sid.to_key_string(), "keep shipping", 0, now_ms() as u64)
                .with_pursuit(alephcore::goal::PursuitMode::Active { max_iterations: 5 }),
        )
        .unwrap();

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;

    assert_eq!(report.scanned, 1);
    assert_eq!(report.resumed, 0);
    assert_eq!(report.abandoned, 1);
    assert!(
        calls.lock().await.is_empty(),
        "capped run must not re-trigger"
    );

    // An `Abandoned` marker was appended so the run is not re-scanned.
    let all = store.load_all_events(&sid).await.unwrap();
    let abandoned = all.iter().any(|r| {
        matches!(
            &r.event,
            SessionEvent::RunFinished {
                outcome: RunOutcome::Abandoned,
                ..
            }
        )
    });
    assert!(abandoned, "expected a RunFinished{{Abandoned}} marker");

    // The Active-pursuit goal was honestly terminated with a note naming the cause.
    let goal = goals
        .get(&sid.to_key_string())
        .unwrap()
        .expect("goal row survives");
    assert_eq!(goal.status, alephcore::goal::GoalStatus::Blocked);
    assert!(
        goal.note.as_deref().unwrap_or("").contains("abandoned"),
        "blocked note must name the abandon: {:?}",
        goal.note
    );

    // A PASSIVE goal in a different session must NOT be collateral-blocked by
    // an unrelated abandon (its recovery never depended on the coordinator).
    let passive_sid = SessionKey::main("cap-passive");
    goals
        .put(&alephcore::goal::Goal::new(
            &passive_sid.to_key_string(),
            "interactive only",
            0,
            now_ms() as u64,
        ))
        .unwrap();
    let passive_store: Arc<dyn SessionEventStore> = {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        Arc::new(SqliteEventStore::new(conn))
    };
    seed_crash_looped_run(&passive_store, &passive_sid, 3).await;
    let coordinator2 = ResumeCoordinator::new(
        passive_store.clone(),
        ResumeConfig::default(),
        Arc::new(RecordingAdapter::new()) as Arc<dyn ExecutionAdapter>,
        registry_with_agent(passive_sid.agent_id()).await,
        sessions(),
        test_bus(),
    );
    coordinator2.resume_interrupted_runs().await;
    assert_eq!(
        goals
            .get(&passive_sid.to_key_string())
            .unwrap()
            .unwrap()
            .status,
        alephcore::goal::GoalStatus::Active,
        "a passive goal must survive an unrelated abandon untouched"
    );
}

/// §5.1 / §5.6: three boots whose retrigger never reaches `RunStarted` — the
/// adapter records the call and writes nothing, i.e. a crash in admit / hook /
/// seed. Under the old counter `trailing_starts` never moved and this looped
/// forever; the intent stamp counts the ATTEMPT, not the run's own marker.
#[tokio::test]
async fn a_retrigger_that_never_starts_is_capped_by_the_intent_stamp() {
    let store = store();
    let sid = SessionKey::main("ratchet-agent");
    seed_interrupted_run(&store, &sid).await;
    let cfg = ResumeConfig {
        max_attempts: 2,
        ..ResumeConfig::default()
    };
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    // Each boot is a fresh coordinator over the SAME log. The coordinator is
    // built outside the future so the future owns it outright.
    let boot = || {
        let c = ResumeCoordinator::new(
            store.clone(),
            cfg.clone(),
            adapter.clone() as Arc<dyn ExecutionAdapter>,
            registry.clone(),
            sessions(),
            test_bus(),
        );
        async move { c.resume_interrupted_runs().await }
    };
    let r1 = boot().await;
    assert_eq!((r1.resumed, r1.abandoned), (1, 0));
    let r2 = boot().await;
    assert_eq!((r2.resumed, r2.abandoned), (1, 0));
    let r3 = boot().await;
    assert_eq!(
        (r3.resumed, r3.abandoned),
        (0, 1),
        "attempts == max_attempts: abandon, no retrigger"
    );
    let r4 = boot().await;
    assert_eq!((r4.resumed, r4.abandoned, r4.skipped), (0, 0, 1));
    assert_eq!(
        calls.lock().await.len(),
        2,
        "exactly two retriggers were ever dispatched"
    );
    let stamps: Vec<(u64, u32)> = store
        .load_all_events(&sid)
        .await
        .unwrap()
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ResumeAttempted { target, attempt } => Some((*target, *attempt)),
            _ => None,
        })
        .collect();
    assert_eq!(
        stamps,
        vec![(3, 1), (3, 2)],
        "each stamp names the RunStarted (seq 3) and its ordinal"
    );
}

/// An adapter that, at the moment `execute` is invoked, reads the session's
/// log and records how many `ResumeAttempted` stamps are ALREADY durable.
///
/// The ratchet test above counts boots and stamps after the fact, which a
/// stamp written AFTER the retrigger passes just as well (the mock adapter
/// returns at once, so the stamp lands either way — measured: moving the stamp
/// below `retrigger` left that test green). Only an observer inside `execute`
/// can tell "written before" from "written after".
struct StampWitnessAdapter {
    store: Arc<dyn SessionEventStore>,
    /// Stamp count visible in the log at each `execute` call, in call order.
    seen: Arc<Mutex<Vec<usize>>>,
}

#[async_trait]
impl ExecutionAdapter for StampWitnessAdapter {
    async fn execute(
        &self,
        request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        let stamps = self
            .store
            .load_all_events(&request.session_key)
            .await
            .map_err(|e| ExecutionError::Failed(e.to_string()))?
            .iter()
            .filter(|r| matches!(r.event, SessionEvent::ResumeAttempted { .. }))
            .count();
        self.seen.lock().await.push(stamps);
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

/// §5.1's ORDER, asserted where it is observable: when the engine is handed
/// the resumed run, this boot's stamp is already in the log. A crash inside
/// `execute` (admit / hook / seed — before the run's own `RunStarted`) then
/// still counts on the next boot.
#[tokio::test]
async fn the_intent_stamp_is_durable_before_the_engine_is_handed_the_run() {
    let store = store();
    let sid = SessionKey::main("stamp-order-agent");
    seed_interrupted_run(&store, &sid).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let adapter = Arc::new(StampWitnessAdapter {
        store: store.clone(),
        seen: seen.clone(),
    });
    let registry = registry_with_agent(sid.agent_id()).await;
    for _ in 0..2 {
        let c = ResumeCoordinator::new(
            store.clone(),
            ResumeConfig::default(),
            adapter.clone() as Arc<dyn ExecutionAdapter>,
            registry.clone(),
            sessions(),
            test_bus(),
        );
        let r = c.resume_interrupted_runs().await;
        assert_eq!(r.resumed, 1);
    }
    assert_eq!(
        *seen.lock().await,
        vec![1, 2],
        "at each execute, this boot's own stamp was already durable"
    );
}

/// Recency-filter abandon (candidate older than `max_age_secs`) takes the
/// same honest-termination path: marker + goal block, no re-trigger.
#[tokio::test]
async fn too_old_candidate_abandons_and_blocks_the_goal() {
    let store = store();
    let sid = SessionKey::main("old-agent");
    let old_at = now_ms() - 2 * 86_400 * 1000; // 2 days > default max_age 1 day
    store
        .append(
            &sid,
            1,
            &SessionEvent::RunStarted {
                run_id: "r-old".into(),
                at: old_at,
                project_root: None,
                envelope: None,
            },
            old_at,
        )
        .await
        .unwrap();

    let goals = shared_goal_store();
    goals
        .put(
            &alephcore::goal::Goal::new(&sid.to_key_string(), "stale pursuit", 0, now_ms() as u64)
                .with_pursuit(alephcore::goal::PursuitMode::Active { max_iterations: 5 }),
        )
        .unwrap();

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;

    assert_eq!(report.abandoned, 1);
    assert_eq!(report.resumed, 0);
    assert!(calls.lock().await.is_empty(), "too-old must not re-trigger");

    let goal = goals
        .get(&sid.to_key_string())
        .unwrap()
        .expect("goal row survives");
    assert_eq!(goal.status, alephcore::goal::GoalStatus::Blocked);
    assert!(
        goal.note.as_deref().unwrap_or("").contains("too old"),
        "blocked note must carry the reason: {:?}",
        goal.note
    );
}

/// `max_age_secs` is measured from the interruption, not from the last
/// ATTEMPT to resume it. Since §5.1 every boot writes a `ResumeAttempted`
/// stamp — the newest marker AND the newest in-scope event — and a stamp that
/// counted as "alive" would let each boot's own attempt resurrect a run the
/// operator's window had already ruled out. Here the run is two days old and
/// the stamp is a moment old: still too old.
#[tokio::test]
async fn a_fresh_stamp_does_not_resurrect_a_run_interrupted_too_long_ago() {
    let store = store();
    let sid = SessionKey::main("stale-stamped-agent");
    let old_at = now_ms() - 2 * 86_400 * 1000; // 2 days > default max_age 1 day
    store
        .append(
            &sid,
            1,
            &SessionEvent::RunStarted {
                run_id: "r-old".into(),
                at: old_at,
                project_root: None,
                envelope: None,
            },
            old_at,
        )
        .await
        .unwrap();
    store
        .append(
            &sid,
            2,
            &SessionEvent::ResumeAttempted {
                target: 1,
                attempt: 1,
            },
            now_ms(),
        )
        .await
        .unwrap();

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;

    assert_eq!((report.abandoned, report.resumed), (1, 0));
    assert!(
        calls.lock().await.is_empty(),
        "a fresh stamp must not make a two-day-old run resumable"
    );
}

/// The one thing a [`FaultingStore`] refuses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// Any batch carrying a `ResumeAttempted` — the intent stamp (§5.1).
    StampAppend,
    /// Every `load_events_range` — the unanswered tail read (§5.2).
    TailRead,
}

/// A store that refuses exactly one thing and nothing else: every other call
/// reaches the real `SqliteEventStore` underneath, so the rest of the log
/// stays readable and writable and the refusal is the only fault in play.
struct FaultingStore {
    inner: Arc<dyn SessionEventStore>,
    fault: Fault,
}

#[async_trait]
impl SessionEventStore for FaultingStore {
    async fn append_batch(
        &self,
        session_id: &SessionKey,
        first_seq: EventSeq,
        events: &[(SessionEvent, i64)],
        retire: Option<Retire>,
        durability: Durability,
    ) -> Result<(), SessionError> {
        if self.fault == Fault::StampAppend
            && events
                .iter()
                .any(|(e, _)| matches!(e, SessionEvent::ResumeAttempted { .. }))
        {
            return Err(SessionError::Storage("disk full: stamp refused".into()));
        }
        self.inner
            .append_batch(session_id, first_seq, events, retire, durability)
            .await
    }
    async fn load_all_events(
        &self,
        session_id: &SessionKey,
    ) -> Result<Vec<alephcore::session::SessionEventRecord>, SessionError> {
        self.inner.load_all_events(session_id).await
    }
    async fn load_events_range(
        &self,
        session_id: &SessionKey,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<alephcore::session::SessionEventRecord>, SessionError> {
        if self.fault == Fault::TailRead {
            return Err(SessionError::Storage("i/o error: tail read refused".into()));
        }
        self.inner.load_events_range(session_id, from, to).await
    }
    async fn load_head_seq(&self, session_id: &SessionKey) -> Result<EventSeq, SessionError> {
        self.inner.load_head_seq(session_id).await
    }
    async fn retire_from(
        &self,
        session_id: &SessionKey,
        from_seq: EventSeq,
    ) -> Result<usize, SessionError> {
        self.inner.retire_from(session_id, from_seq).await
    }
    async fn is_retired(
        &self,
        session_id: &SessionKey,
        seq: EventSeq,
    ) -> Result<bool, SessionError> {
        self.inner.is_retired(session_id, seq).await
    }
    async fn load_run_markers(
        &self,
    ) -> Result<Vec<(SessionKey, alephcore::session::store::MarkerSlice)>, SessionError> {
        self.inner.load_run_markers().await
    }
    async fn load_rows(
        &self,
        session_id: &SessionKey,
    ) -> Result<Vec<alephcore::session::store::DecodedRow>, SessionError> {
        self.inner.load_rows(session_id).await
    }
    async fn retire_record(
        &self,
        session_id: &SessionKey,
        seq: EventSeq,
    ) -> Result<bool, SessionError> {
        self.inner.retire_record(session_id, seq).await
    }
}

/// §5.1's closed side: a stamp that does not land REFUSES the resume. The
/// alternative — warn and retrigger anyway — is a resume the next boot cannot
/// count, i.e. the unbounded loop the stamp exists to close. Pinned at the
/// effect: the refusal is filed under its own word, the adapter is never
/// called, and nothing pretends the run was abandoned either.
#[tokio::test]
async fn a_stamp_that_does_not_land_refuses_the_resume_without_retriggering() {
    let inner = store();
    let sid = SessionKey::main("stamp-refused-agent");
    seed_interrupted_run(&inner, &sid).await;
    let store: Arc<dyn SessionEventStore> = Arc::new(FaultingStore {
        inner: inner.clone(),
        fault: Fault::StampAppend,
    });

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = ResumeCoordinator::new(
        store,
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;

    assert_eq!(
        (report.scanned, report.resumed, report.abandoned),
        (1, 0, 0)
    );
    assert_eq!(report.refused.len(), 1, "{:?}", report.refused);
    let (refused_sid, refusal) = &report.refused[0];
    assert_eq!(refused_sid, &sid);
    assert!(
        matches!(
            refusal,
            alephcore::gateway::ResumeRefusal::IntentStampFailed(_)
        ),
        "filed under its own word, not as a retrigger or repair failure: {refusal:?}"
    );
    assert_eq!(refusal.reason(), "intent_stamp_failed");
    assert!(
        calls.lock().await.is_empty(),
        "a resume without its stamp must not be dispatched"
    );

    let all = inner.load_all_events(&sid).await.unwrap();
    assert!(
        all.iter().any(|r| matches!(&r.event, SessionEvent::ToolError { call_id, .. } if call_id == "dangling-1")),
        "the boundary repair before the stamp still landed"
    );
    assert!(
        !all.iter().any(|r| matches!(
            &r.event,
            SessionEvent::RunFinished {
                outcome: RunOutcome::Abandoned,
                ..
            }
        )),
        "a refused stamp is not an abandonment: the run stays resumable"
    );
    assert!(
        !all.iter()
            .any(|r| matches!(r.event, SessionEvent::ResumeAttempted { .. })),
        "the refused stamp is not in the log"
    );
}

/// A Chat-tier channel's policy is a pair of RESTRICTIVE inputs, and both fail
/// OPEN when missing: `role_is_operator(None) == true`, and an absent channel
/// `ToolPermissionsConfig` merges no deny layer. The boot coordinator used to
/// build a resumed run's metadata from an empty `HashMap`, so a killed daemon
/// resurrected a guest-tier Telegram run as an unwatched **operator** with no
/// deny layer. Re-derive both from the process-global channel-config snapshot.
///
/// NOTE: `set_channel_config_snapshot` is a set-once process global — this is
/// the only test in this binary that publishes one, and the unattended sibling
/// below never reads it (no origin route), so there is no cross-test race.
#[tokio::test]
async fn resumed_channel_run_reinherits_the_channels_guest_clamp_and_deny_layer() {
    use alephcore::gateway::channel_policy::set_channel_config_snapshot;
    use alephcore::gateway::execution_engine::{CHANNEL_TOOL_PERMISSIONS_KEY, UNATTENDED_KEY};
    use alephcore::gateway::inbound_router::{ChannelConfig, ChannelPolicyConfig};
    use alephcore::routing::session_key::DmScope;

    let store = store();
    let sid = SessionKey::dm("main", "telegram", "peer-1", DmScope::PerChannelPeer);
    seed_interrupted_run(&store, &sid).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    // The session was born on Telegram — the same stamp `execute` writes on a
    // session's first inbound message, and the seam `origin_route` reads back.
    let agent = registry
        .get(sid.agent_id())
        .await
        .expect("agent registered");
    agent.ensure_session(&sid).await;
    agent
        .set_session_source_channel(&sid, "telegram", Some("chat-42"))
        .await;

    // The channel's live policy, parsed from the same flat config block boot
    // reads: Chat tier (the default) plus a deny layer. Published to the global
    // snapshot exactly as `initialize_inbound_router` does at boot.
    let policy: ChannelPolicyConfig = serde_json::from_value(serde_json::json!({
        "permission_level": "chat",
        "tool_permissions": { "default": "allow", "overrides": { "bash_exec": "deny" } }
    }))
    .unwrap();
    let mut channel_configs = HashMap::new();
    channel_configs.insert(
        "telegram".to_string(),
        ChannelConfig {
            permission_level: policy.permission_level,
            tool_permissions: policy.tool_permissions,
            ..Default::default()
        },
    );
    set_channel_config_snapshot(channel_configs);

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    let report = coordinator.resume_interrupted_runs().await;
    assert_eq!(report.resumed, 1);

    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    let metadata = &calls[0].1;

    assert_eq!(
        metadata.get("caller_role").map(String::as_str),
        Some("guest"),
        "a resumed Chat-tier channel run must not come back as an operator"
    );
    let perms = metadata
        .get(CHANNEL_TOOL_PERMISSIONS_KEY)
        .expect("the channel deny layer must survive the restart");
    assert!(perms.contains("bash_exec"), "{perms}");
    // The origin route is what makes an approval deliverable, so the run stays
    // attended (the human on the other end of Telegram can answer it).
    assert_eq!(
        metadata.get("channel_id").map(String::as_str),
        Some("telegram")
    );
    assert_eq!(
        metadata.get("conversation_id").map(String::as_str),
        Some("chat-42")
    );
    assert!(!metadata.contains_key(UNATTENDED_KEY));
}

/// The other half of the same rule: a session with no routable origin (the
/// Panel's `gui:chat`, or an origin conversation that was never captured) has
/// nowhere to deliver an approval card that a boot scan's re-trigger raises.
/// Mark it `unattended` so confirm-gated tools fail CLOSED instead of publishing
/// into the void and parking on the 120 s approval timeout.
#[tokio::test]
async fn resumed_run_with_no_routable_origin_is_marked_unattended() {
    use alephcore::gateway::execution_engine::UNATTENDED_KEY;

    let store = store();
    let sid = SessionKey::main("main");
    seed_interrupted_run(&store, &sid).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    );
    assert_eq!(coordinator.resume_interrupted_runs().await.resumed, 1);

    let calls = calls.lock().await;
    let metadata = &calls[0].1;
    assert_eq!(
        metadata.get(UNATTENDED_KEY).map(String::as_str),
        Some("true")
    );
    // No origin channel ⇒ no channel clamp to re-derive; the Panel's own
    // operator semantics are unchanged.
    assert!(!metadata.contains_key("caller_role"));
}

/// Adapter that publishes one frame through whatever emitter it is handed,
/// standing in for the real engine's `RunAccepted`.
struct EmittingAdapter;

#[async_trait]
impl ExecutionAdapter for EmittingAdapter {
    async fn execute(
        &self,
        request: RunRequest,
        _agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        emitter
            .emit(alephcore::gateway::StreamEvent::RunAccepted {
                run_id: request.run_id.clone(),
                session_key: request.session_key.to_key_string(),
                accepted_at: "0".to_string(),
            })
            .await
            .expect("emit");
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

/// The event bus is a mandatory constructor input (no `Option` escape hatch
/// that could re-introduce the collect-and-drop shape); tests that don't care
/// about frames use this throwaway bus with no subscribers.
fn test_bus() -> Arc<alephcore::gateway::event_bus::GatewayEventBus> {
    Arc::new(alephcore::gateway::event_bus::GatewayEventBus::new())
}

/// A crash-recovered run must be visible to, and stoppable from, the UIs.
///
/// Asserted at the CONSUMER end (a bus subscriber), not by inspecting which
/// emitter was constructed. `RunAccepted` is load-bearing twice over: it seeds
/// `event_visibility::EventVisibilityIndex`, which fail-closed-drops every
/// later frame of a run it never saw accepted, and it is the only carrier of
/// the `run_id` that `chat.abort` / `agent.cancel` require. The
/// pre-mandatory-bus shape (a bare `CollectingEventEmitter`) lit the sidebar
/// up (the run
/// registry broadcasts `RunningSetChanged` regardless) while the transcript
/// stayed empty and no UI could stop the run.
#[tokio::test]
async fn a_resumed_run_reaches_the_gateway_bus() {
    let store = store();
    let sid = SessionKey::main("main");
    seed_interrupted_run(&store, &sid).await;

    let bus = Arc::new(alephcore::gateway::event_bus::GatewayEventBus::new());
    let mut rx = bus.subscribe_typed();

    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        Arc::new(EmittingAdapter) as Arc<dyn ExecutionAdapter>,
        registry_with_agent(sid.agent_id()).await,
        sessions(),
        bus,
    );
    assert_eq!(coordinator.resume_interrupted_runs().await.resumed, 1);

    let mut saw_accepted = None;
    while let Ok(frame) = rx.try_recv() {
        if let alephcore::gateway::events::frame::GatewayEventFrame::RunAccepted {
            run_id,
            session_key,
            ..
        } = frame
        {
            saw_accepted = Some((run_id, session_key));
            break;
        }
    }
    let (run_id, session_key) = saw_accepted
        .expect("the resumed run must publish RunAccepted on the bus, not into a collector");
    assert!(
        !run_id.is_empty(),
        "chat.abort has no other way to address this run"
    );
    assert_eq!(session_key, sid.to_key_string());
}

/// An adapter that reports one session as having a run in flight right now —
/// the shape the real engine presents while the owning scheduler is mid-turn on
/// a session the boot scan is walking past.
struct RunningAdapter {
    running: String,
}

#[async_trait]
impl ExecutionAdapter for RunningAdapter {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        panic!("a session with its own scheduler must never be re-dispatched here");
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        Err(ExecutionError::RunNotFound(run_id.to_string()))
    }

    async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
        None
    }

    async fn active_run_count(&self) -> usize {
        1
    }

    fn running_sessions(&self) -> Vec<String> {
        vec![self.running.clone()]
    }
}

/// The two facts the delegated arm may have written, counted off the log so
/// both halves of this pair of tests read the same pair.
async fn repair_marks(store: &Arc<dyn SessionEventStore>, sid: &SessionKey) -> (usize, usize) {
    let all = store.load_all_events(sid).await.expect("load");
    let errors = all
        .iter()
        .filter(|r| {
            matches!(&r.event, SessionEvent::ToolError { call_id, .. } if call_id == "dangling-1")
        })
        .count();
    let closers = all
        .iter()
        .filter(|r| {
            matches!(
                &r.event,
                SessionEvent::RunFinished { run_id, outcome, .. }
                    if run_id == "run-1" && *outcome == RunOutcome::Abandoned
            )
        })
        .count();
    (errors, closers)
}

/// C9: handing recovery back to the scheduler that owns a session is not the
/// same as handing back its LOG. The dispatcher / cron / heartbeat decides
/// whether the work is redone; only a reader of the log can answer the calls
/// the crash left dangling and close the marker it left open.
///
/// Asserted as effects on the log (判据 #4): the old arm called a closer that
/// minted a `delegated-<uuid>` matching no `RunStarted`, and repaired nothing
/// at all — so the next attempt's replay silently dropped the orphan `tool_use`
/// and every later boot re-classified the session `Interrupted`.
#[tokio::test]
async fn a_delegated_session_is_repaired_and_its_own_marker_closed() {
    let store = store();
    let sid = SessionKey::task("main", "cron", "daily-summary");
    seed_interrupted_run(&store, &sid).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;

    let report = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    )
    .resume_interrupted_runs()
    .await;

    assert_eq!(report.delegated, 1, "{report:?}");
    assert_eq!(
        report.resumed, 0,
        "the owning scheduler re-dispatches, not us"
    );
    assert_eq!(report.busy, 0);
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let (errors, closers) = repair_marks(&store, &sid).await;
    assert_eq!(errors, 1, "the dangling call was answered before hand-back");
    assert_eq!(
        closers, 1,
        "the marker was closed with the open run's OWN id, so no later boot re-classifies it"
    );
    assert!(
        calls.lock().await.is_empty(),
        "the resume scan must not dispatch a session it delegated"
    );
}

/// The same hand-back while the owning scheduler has a live turn on the
/// session: both writes are appends, and a `RunFinished` landing in the middle
/// of a running turn makes that turn's real finish read as `FinishWithoutStart`
/// forever. `busy` is the honest count — nothing was handed back and nothing
/// was written.
#[tokio::test]
async fn a_delegated_session_the_engine_is_running_is_left_alone() {
    let store = store();
    let sid = SessionKey::task("main", "cron", "hourly-digest");
    seed_interrupted_run(&store, &sid).await;
    let before = store.load_all_events(&sid).await.expect("load").len();

    let adapter = Arc::new(RunningAdapter {
        running: sid.to_key_string(),
    });
    let registry = registry_with_agent(sid.agent_id()).await;

    let report = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    )
    .resume_interrupted_runs()
    .await;

    assert_eq!(report.busy, 1, "{report:?}");
    assert_eq!(report.delegated, 0, "nothing was handed back");
    assert_eq!(
        (0, 0),
        repair_marks(&store, &sid).await,
        "a live turn's log gets neither the repair nor the closer"
    );
    assert_eq!(
        before,
        store.load_all_events(&sid).await.expect("load").len(),
        "not one append while somebody else is writing"
    );
}

// ---- §5.2 Unanswered: the seed→RunStarted window ---------------------------

/// A `UserMessage` with no marker around it, exactly as `seed_session` writes
/// it before the run's own `RunStarted`.
fn seeded_user(tid: TurnId, text: &str, at: i64) -> SessionEvent {
    SessionEvent::UserMessage {
        turn_id: tid,
        content: alephcore::session::events::MessageContent {
            text: text.into(),
            blocks: vec![],
            thinking: None,
            thinking_signature: None,
        },
        at,
        synthetic: false,
        author_user_id: None,
    }
}

/// Every `ResumeAttempted` stamp in the log, in seq order, as `(target,
/// attempt)`.
async fn stamps(store: &Arc<dyn SessionEventStore>, sid: &SessionKey) -> Vec<(EventSeq, u32)> {
    store
        .load_all_events(sid)
        .await
        .expect("load")
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ResumeAttempted { target, attempt } => Some((*target, *attempt)),
            _ => None,
        })
        .collect()
}

/// §5.2: a seeded message no run ever answered, in a session with no run
/// marker at all. The marker scan cannot see it; the activity window can —
/// the session row `execute()` creates before seeding is what puts it there.
/// The stamp names the message, the retrigger carries `resume`, and nothing
/// is repaired because nothing dangled.
#[tokio::test]
async fn an_unanswered_seed_is_stamped_and_retriggered_without_repair() {
    let store = store();
    let sid = SessionKey::main("unanswered-agent");
    let tid = TurnId::new_v4();
    let at = now_ms();
    store
        .append(
            &sid,
            1,
            &SessionEvent::TurnStarted {
                turn_id: tid,
                trigger: alephcore::session::events::TurnTrigger::UserMessage,
                at,
            },
            at,
        )
        .await
        .unwrap();
    store
        .append(&sid, 2, &seeded_user(tid, "hello?", at + 1), at + 1)
        .await
        .unwrap();
    let sessions = sessions();
    // The row `execute()` creates before seeding — what puts it in the window.
    sessions.get_or_create(&sid).await.unwrap();
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let c = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry_with_agent(sid.agent_id()).await,
        sessions,
        test_bus(),
    );
    let r = c.resume_interrupted_runs().await;
    assert_eq!(
        (r.scanned, r.resumed, r.unsnapshotted),
        (1, 1, 0),
        "not counted as unsnapshotted: there was no RunStarted to snapshot ({r:?})"
    );
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.get("resume").map(String::as_str), Some("true"));
    let all = store.load_all_events(&sid).await.unwrap();
    assert!(matches!(
        all.last().map(|r| &r.event),
        Some(SessionEvent::ResumeAttempted {
            target: 2,
            attempt: 1
        })
    ));
    assert!(
        !all.iter()
            .any(|r| matches!(r.event, SessionEvent::ToolError { .. })),
        "no boundary repair: nothing dangled"
    );
}

/// The shape the SECOND boot sees: `[.., RunFinished, UserMessage,
/// ResumeAttempted]`. The stamp is the newest marker, so a read that started
/// past the last marker would never see the message again — the seed would
/// hide behind its own stamp forever. The read starts past the last
/// `RunFinished` instead, and the ratchet climbs: `[1, 2]`, both naming the
/// message.
#[tokio::test]
async fn an_unanswered_seed_behind_its_own_stamp_is_seen_on_the_next_boot() {
    let store = store();
    let sid = SessionKey::main("unanswered-stamped-agent");
    let tid = TurnId::new_v4();
    let at = now_ms();
    let events: Vec<SessionEvent> = vec![
        SessionEvent::RunStarted {
            run_id: "run-0".into(),
            at,
            project_root: None,
            envelope: None,
        },
        SessionEvent::RunFinished {
            run_id: "run-0".into(),
            outcome: RunOutcome::Completed,
            at: at + 1,
        },
        SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: alephcore::session::events::TurnTrigger::UserMessage,
            at: at + 2,
        },
        seeded_user(tid, "still there?", at + 3),
    ];
    for (i, ev) in events.iter().enumerate() {
        store
            .append(&sid, i as EventSeq + 1, ev, at + i as i64)
            .await
            .unwrap();
    }
    let sessions = sessions();
    sessions.get_or_create(&sid).await.unwrap();
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    let boot = || {
        ResumeCoordinator::new(
            store.clone(),
            ResumeConfig::default(),
            adapter.clone() as Arc<dyn ExecutionAdapter>,
            registry.clone(),
            sessions.clone(),
            test_bus(),
        )
    };

    let first = boot().resume_interrupted_runs().await;
    assert_eq!((first.scanned, first.resumed), (1, 1), "{first:?}");
    assert_eq!(stamps(&store, &sid).await, vec![(4, 1)]);

    let second = boot().resume_interrupted_runs().await;
    assert_eq!(
        (second.scanned, second.resumed, second.skipped),
        (1, 1, 0),
        "the stamped seed is still unanswered, not `skipped`: {second:?}"
    );
    assert_eq!(stamps(&store, &sid).await, vec![(4, 1), (4, 2)]);
    assert_eq!(calls.lock().await.len(), 2);
}

/// The cap reads the unanswered ratchet the same way `Interrupted` reads its
/// own: `max_attempts` stamps already spent ⇒ abandoned, not retriggered, and
/// the closer pairs with no `RunStarted` (a `FinishWithoutStart` by design).
#[tokio::test]
async fn an_unanswered_seed_is_capped_by_its_own_stamps() {
    let store = store();
    let sid = SessionKey::main("unanswered-capped-agent");
    let tid = TurnId::new_v4();
    let at = now_ms();
    store
        .append(&sid, 1, &seeded_user(tid, "hello?", at), at)
        .await
        .unwrap();
    let max_attempts = ResumeConfig::default().max_attempts;
    for attempt in 1..=max_attempts {
        store
            .append(
                &sid,
                1 + EventSeq::from(attempt),
                &SessionEvent::ResumeAttempted { target: 1, attempt },
                at + i64::from(attempt),
            )
            .await
            .unwrap();
    }
    let sessions = sessions();
    sessions.get_or_create(&sid).await.unwrap();
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let r = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry_with_agent(sid.agent_id()).await,
        sessions,
        test_bus(),
    )
    .resume_interrupted_runs()
    .await;
    assert_eq!((r.scanned, r.resumed, r.abandoned), (1, 0, 1), "{r:?}");
    assert!(
        calls.lock().await.is_empty(),
        "capped: nothing re-triggered"
    );
    let all = store.load_all_events(&sid).await.unwrap();
    assert!(
        matches!(
            all.last().map(|r| &r.event),
            Some(SessionEvent::RunFinished {
                outcome: RunOutcome::Abandoned,
                ..
            })
        ),
        "the abandon closer lands last: {:?}",
        all.last()
    );
}

/// §5.2's closed side: a tail that cannot be read is "I cannot tell whether
/// the last message was answered", filed under its own word — not a repair
/// failure (none was attempted), not `skipped` (nothing was decided), and
/// nothing is stamped or dispatched on a question with no answer.
#[tokio::test]
async fn a_tail_that_cannot_be_read_refuses_without_stamping_or_retriggering() {
    let inner = store();
    let sid = SessionKey::main("tail-refused-agent");
    let tid = TurnId::new_v4();
    let at = now_ms();
    inner
        .append(
            &sid,
            1,
            &SessionEvent::TurnStarted {
                turn_id: tid,
                trigger: alephcore::session::events::TurnTrigger::UserMessage,
                at,
            },
            at,
        )
        .await
        .unwrap();
    inner
        .append(&sid, 2, &seeded_user(tid, "hello?", at + 1), at + 1)
        .await
        .unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(FaultingStore {
        inner: inner.clone(),
        fault: Fault::TailRead,
    });
    let sessions = sessions();
    sessions.get_or_create(&sid).await.unwrap();
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let report = ResumeCoordinator::new(
        store,
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry_with_agent(sid.agent_id()).await,
        sessions,
        test_bus(),
    )
    .resume_interrupted_runs()
    .await;

    assert_eq!(
        (
            report.scanned,
            report.resumed,
            report.skipped,
            report.abandoned
        ),
        (1, 0, 0, 0),
        "{report:?}"
    );
    assert_eq!(report.refused.len(), 1, "{:?}", report.refused);
    let (refused_sid, refusal) = &report.refused[0];
    assert_eq!(refused_sid, &sid);
    assert!(
        matches!(
            refusal,
            alephcore::gateway::ResumeRefusal::TailReadFailed(_)
        ),
        "filed under its own word, not as a repair failure: {refusal:?}"
    );
    assert_eq!(refusal.reason(), "tail_read_failed");
    assert!(
        calls.lock().await.is_empty(),
        "an unanswerable question must not be dispatched"
    );
    assert!(stamps(&inner, &sid).await.is_empty(), "nothing was stamped");
    assert_eq!(
        inner.load_all_events(&sid).await.unwrap().len(),
        2,
        "not one append on a log nobody could read"
    );
}

/// Criterion #8 at the resume face, with the real store: a session whose
/// marker row this build cannot decode is refused under its own kind — filed
/// as `log_inconsistent` with the undecodable-record contradiction, never
/// read as "no markers" and never as clean — while its neighbour, interrupted
/// and decodable, is resumed exactly as before. The whole scan used to fail
/// on the first such row, refusing every session at once.
#[tokio::test]
async fn an_undecodable_marker_row_refuses_only_its_own_session_at_the_resume_face() {
    use alephcore::session::reduction::LogContradiction;

    // File-backed so a second connection can write the row RAW — past
    // `encode_row`, exactly as another build would have left it on disk.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let bad = SessionKey::main("undecodable-marker-agent");
    let good = SessionKey::main("decodable-neighbour-agent");
    seed_interrupted_run(&store, &bad).await;
    seed_interrupted_run(&store, &good).await;
    // A marker row with an outcome word this build's `RunOutcome` does not
    // know: a `run_finished` written by a newer build.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO session_events (session_id, seq, event_type, payload_json, created_at) \
             VALUES (?1, 99, 'run_finished', ?2, 1)",
            rusqlite::params![
                serde_json::to_string(&bad).unwrap(),
                r#"{"type":"run_finished","run_id":"run-1","outcome":"from_the_future","at":1}"#
            ],
        )
        .unwrap();

    let sessions = sessions();
    for sid in [&bad, &good] {
        sessions.get_or_create(sid).await.unwrap();
    }
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let report = ResumeCoordinator::new(
        store,
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry_with_agent(good.agent_id()).await,
        sessions,
        test_bus(),
    )
    .resume_interrupted_runs()
    .await;

    assert_eq!(
        (report.scanned, report.resumed, report.skipped),
        (2, 1, 0),
        "{report:?}"
    );
    assert_eq!(
        report.refused,
        vec![(
            bad.clone(),
            alephcore::gateway::ResumeRefusal::LogInconsistent(
                LogContradiction::UndecodableRecord { seq: 99 }
            )
        )],
        "refused under its own kind, naming the row"
    );
    assert_eq!(report.refused[0].1.reason(), "log_inconsistent");
    let dispatched: Vec<String> = calls.lock().await.iter().map(|c| c.0.clone()).collect();
    assert_eq!(
        dispatched,
        vec![good.to_key_string()],
        "the neighbour resumed; the refused session was never dispatched"
    );
}

/// The verb's second face (criterion #9): `agent.resume` / `aleph-server
/// resume` on a session with no run marker asks the same §5.2 question and
/// acts on it — it used to return the zero report ("nothing to resume") for
/// exactly the session whose user is waiting.
#[tokio::test]
async fn an_on_demand_resume_of_a_marker_less_unanswered_seed_stamps_and_retriggers() {
    let store = store();
    let sid = SessionKey::main("unanswered-on-demand-agent");
    let tid = TurnId::new_v4();
    let at = now_ms();
    store
        .append(
            &sid,
            1,
            &SessionEvent::TurnStarted {
                turn_id: tid,
                trigger: alephcore::session::events::TurnTrigger::UserMessage,
                at,
            },
            at,
        )
        .await
        .unwrap();
    store
        .append(&sid, 2, &seeded_user(tid, "hello?", at + 1), at + 1)
        .await
        .unwrap();
    let sessions = sessions();
    sessions.get_or_create(&sid).await.unwrap();
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let coordinator = ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry_with_agent(sid.agent_id()).await,
        sessions,
        test_bus(),
    );

    let report = coordinator
        .resume_session(&sid)
        .await
        .expect("markers readable");
    assert_eq!(
        (report.scanned, report.resumed, report.skipped),
        (1, 1, 0),
        "{report:?}"
    );
    assert_eq!(
        stamps(&store, &sid).await,
        vec![(2, 1)],
        "the stamp names the message"
    );
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, sid.to_key_string());
    assert_eq!(calls[0].1.get("resume").map(String::as_str), Some("true"));
}
