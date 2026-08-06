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
    // Keep `temp` alive for the test by leaking it — the dirs must outlive
    // the registry. Acceptable in a test binary.
    std::mem::forget(temp);
    registry
}

fn store() -> Arc<dyn SessionEventStore> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    Arc::new(SqliteEventStore::new(conn))
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
            // The db file must outlive every test in the binary.
            std::mem::forget(dir);
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
    assert_eq!(synthetic_errors[0].1, "interrupted by server restart");

    // `execute` was called exactly once, carrying the resume signal.
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, sid.to_key_string());
    assert_eq!(calls[0].1.get("resume").map(String::as_str), Some("true"));
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
        },
        SessionEvent::RunStarted {
            run_id: "r2".into(),
            at: at + 1,
            project_root: None,
        },
        SessionEvent::RunStarted {
            run_id: "r3".into(),
            at: at + 2,
            project_root: None,
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
