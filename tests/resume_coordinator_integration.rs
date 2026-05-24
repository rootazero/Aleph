//! Integration tests for the mid-run trajectory resume boot scan.
//!
//! Spec: docs/superpowers/specs/2026-05-21-mid-run-trajectory-resume-design.md §7.

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
        },
        SessionEvent::RunStarted {
            run_id: "run-1".into(),
            at: at + 2,
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
    let sid = SessionKey::main("main");
    let at = now_ms();
    // 3 consecutive RunStarted with no RunFinished == default max_attempts.
    for (i, ev) in [
        SessionEvent::RunStarted {
            run_id: "r1".into(),
            at,
        },
        SessionEvent::RunStarted {
            run_id: "r2".into(),
            at: at + 1,
        },
        SessionEvent::RunStarted {
            run_id: "r3".into(),
            at: at + 2,
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
}
