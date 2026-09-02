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
use alephcore::session::events::{now_ms, RunOutcome, SessionEvent, TurnId};
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::ResumeConfig;

/// Mock `ExecutionAdapter` that records every `execute` call's
/// `(session_key, metadata)` so the test can assert resume signalling.
struct RecordingAdapter {
    calls: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
}

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
async fn seed_interrupted_run(store: &Arc<dyn SessionEventStore>, sid: &SessionKey) {
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
            envelope: None,
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
    let (_key, metadata) = calls.first().expect("the resumed run reached the adapter");
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

#[tokio::test]
async fn crash_loop_cap_abandons_instead_of_retriggering() {
    let store = store();
    // Unique agent/session key: the goal store is process-global in this
    // test binary, so each abandon-path test owns its own session.
    let sid = SessionKey::main("cap-agent");
    let at = now_ms();
    // 3 consecutive RunStarted with no RunFinished == default max_attempts.
    for (i, ev) in [
        SessionEvent::RunStarted {
            run_id: "r1".into(),
            at,
            project_root: None,
            envelope: None,
        },
        SessionEvent::RunStarted {
            run_id: "r2".into(),
            at: at + 1,
            project_root: None,
            envelope: None,
        },
        SessionEvent::RunStarted {
            run_id: "r3".into(),
            at: at + 2,
            project_root: None,
            envelope: None,
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
    passive_store
        .append(
            &passive_sid,
            1,
            &SessionEvent::RunStarted {
                run_id: "r-p1".into(),
                at,
                project_root: None,
                envelope: None,
            },
            at,
        )
        .await
        .unwrap();
    for (i, run) in ["r-p2", "r-p3"].iter().enumerate() {
        passive_store
            .append(
                &passive_sid,
                (i as u64) + 2,
                &SessionEvent::RunStarted {
                    run_id: (*run).into(),
                    at: at + 1 + i as i64,
                    project_root: None,
                    envelope: None,
                },
                at,
            )
            .await
            .unwrap();
    }
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
