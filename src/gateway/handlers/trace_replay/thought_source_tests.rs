use super::tests::{req, seed_session, session_store};
use super::thought_source::thought_block_id;
use super::*;
use crate::gateway::caller_identity::CALLER_USER;
use crate::resilience::ThoughtProvenance;
use aleph_protocol::thought::{ThoughtBlockId, ThoughtBlockRef};
use tempfile::TempDir;

async fn fixture() -> (
    TempDir,
    Arc<StateDatabase>,
    Arc<dyn SessionStore>,
    SessionKey,
) {
    let temp = TempDir::new().unwrap();
    let db = Arc::new(StateDatabase::new(temp.path().join("state.db")).unwrap());
    let sessions = session_store(&temp);
    let key = SessionKey::main("thought-source");
    seed_session(&sessions, &key, "alice", &["run-a"]).await;
    db.insert_agent_task(&crate::resilience::AgentTask::new(
        "run-a",
        key.to_key_string(),
        "coder",
        "x",
        crate::resilience::RiskLevel::Low,
    ))
    .await
    .unwrap();
    (temp, db, sessions, key)
}

async fn thought(db: &StateDatabase, text: &str, step: u32, provenance: Option<ThoughtProvenance>) {
    let mut trace = crate::resilience::TaskTrace::new(
        "run-a",
        step,
        aleph_protocol::AgentTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: text.into(),
        },
    );
    trace.thought_provenance = provenance;
    db.insert_trace(&trace).await.unwrap();
}

async fn replay(
    db: Arc<StateDatabase>,
    sessions: Arc<dyn SessionStore>,
    key: &SessionKey,
) -> Value {
    CALLER_USER
        .scope(Some("alice".into()), async {
            handle_by_runs(
                req(json!({"session_key": key.to_key_string(), "run_ids": ["run-a"]})),
                db,
                sessions,
            )
            .await
            .result
            .unwrap()["runs"]["run-a"]
                .clone()
        })
        .await
}

async fn source(
    reference: &ThoughtBlockRef,
    db: &StateDatabase,
    sessions: &dyn SessionStore,
) -> Option<String> {
    CALLER_USER
        .scope(
            Some("alice".into()),
            resolve_thought_source(reference, db, sessions),
        )
        .await
}

fn reference(row: &Value) -> ThoughtBlockRef {
    serde_json::from_value(row["thought_block_ref"].clone())
        .expect("eligible replay must carry a reference")
}

#[tokio::test]
async fn thought_source_rejects_foreign_run() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(
        &db,
        "own source",
        0,
        Some(ThoughtProvenance::ProviderResponse),
    )
    .await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    let owned = reference(&rows[0]);
    assert_eq!(
        source(&owned, &db, sessions.as_ref()).await.as_deref(),
        Some("own source")
    );
    let mut foreign = owned.clone();
    foreign.run_id = "foreign-run".into();
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
    let bob = SessionKey::main("bob-thought");
    seed_session(&sessions, &bob, "bob", &["foreign-run"]).await;
    db.insert_agent_task(&crate::resilience::AgentTask::new(
        "foreign-run",
        bob.to_key_string(),
        "coder",
        "x",
        crate::resilience::RiskLevel::Low,
    ))
    .await
    .unwrap();
    let mut trace = crate::resilience::TaskTrace::new(
        "foreign-run",
        0,
        aleph_protocol::AgentTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: "foreign secret".into(),
        },
    );
    trace.thought_provenance = Some(ThoughtProvenance::ProviderResponse);
    db.insert_trace(&trace).await.unwrap();
    let bob_rows = CALLER_USER
        .scope(
            Some("bob".into()),
            handle_by_runs(
                req(json!({"session_key":bob.to_key_string(), "run_ids":["foreign-run"]})),
                db.clone(),
                sessions.clone(),
            ),
        )
        .await
        .result
        .unwrap();
    foreign = reference(&bob_rows["runs"]["foreign-run"][0]);
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
    foreign.session_key = key.to_key_string();
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
    foreign = owned;
    foreign.block_id = ThoughtBlockId("unknown".into());
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
}

#[tokio::test]
async fn thought_source_rejects_unfinished_block() {
    let (_temp, db, sessions, key) = fixture().await;
    use crate::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
    let emitter = CollectingEventEmitter::new();
    // The same stream-only shape is emitted by slash commands and synthetic
    // execution progress. is_complete never attests a provider response.
    for (seq, content, complete) in [
        (1, "unrecorded thought", false),
        (2, "command reasoning", true),
        (3, "Analyzing the request...", true),
    ] {
        emitter
            .emit(crate::gateway::event_emitter::StreamEvent::Reasoning {
                run_id: "run-a".into(),
                seq,
                content: content.into(),
                is_complete: complete,
            })
            .await
            .unwrap();
    }
    assert_eq!(emitter.events().await.len(), 3);
    assert_eq!(replay(db.clone(), sessions.clone(), &key).await, json!([]));
    let guessed = ThoughtBlockRef {
        session_key: key.to_key_string(),
        run_id: "run-a".into(),
        block_id: ThoughtBlockId("stream-only".into()),
    };
    assert!(source(&guessed, &db, sessions.as_ref()).await.is_none());
}

#[tokio::test]
async fn thought_source_rejects_mixed_or_legacy_rows() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(&db, "legacy reasoning", 0, None).await;
    thought(
        &db,
        "unknown provenance",
        1,
        Some(ThoughtProvenance::Unknown),
    )
    .await;
    let mut mixed = crate::resilience::TaskTrace::new(
        "run-a",
        2,
        aleph_protocol::AgentTraceEvent::ToolSummary {
            iteration: 1,
            summary: "tool summary".into(),
        },
    );
    mixed.thought_provenance = Some(ThoughtProvenance::ProviderResponse);
    db.insert_trace(&mixed).await.unwrap();
    thought(&db, "  ", 3, Some(ThoughtProvenance::ProviderResponse)).await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    assert_eq!(rows.as_array().unwrap().len(), 4);
    for row in rows.as_array().unwrap() {
        assert!(row.get("thought_block_ref").is_none());
    }
    for trace in db.get_traces_by_task("run-a").await.unwrap() {
        let guessed = ThoughtBlockRef {
            session_key: key.to_key_string(),
            run_id: "run-a".into(),
            block_id: thought_block_id(&trace),
        };
        assert!(source(&guessed, &db, sessions.as_ref()).await.is_none());
    }
}

#[tokio::test]
async fn two_blocks_in_one_iteration_have_distinct_ids() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(&db, "first", 0, Some(ThoughtProvenance::ProviderResponse)).await;
    thought(&db, "second", 1, Some(ThoughtProvenance::ProviderResponse)).await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    let first = reference(&rows[0]);
    let second = reference(&rows[1]);
    assert_ne!(first.block_id, second.block_id);
    assert_eq!(
        source(&first, &db, sessions.as_ref()).await.as_deref(),
        Some("first")
    );
    assert_eq!(
        source(&second, &db, sessions.as_ref()).await.as_deref(),
        Some("second")
    );
    let reopened = Arc::new(StateDatabase::new(_temp.path().join("state.db")).unwrap());
    assert_eq!(rows, replay(reopened, sessions.clone(), &key).await);
}

#[tokio::test]
async fn thought_source_equals_masked_replay_text() {
    let (_temp, db, sessions, key) = fixture().await;
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    let text = crate::exec::SecretMasker::new().mask(&format!("think {secret} carefully"));
    assert!(!text.contains(secret));
    thought(&db, &text, 0, Some(ThoughtProvenance::ProviderResponse)).await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    let selected_input = source(&reference(&rows[0]), &db, sessions.as_ref())
        .await
        .unwrap();
    assert_eq!(selected_input, rows[0]["text"].as_str().unwrap());
    assert_eq!(selected_input, text);
}

#[tokio::test]
async fn thought_source_keeps_old_and_unknown_provenance_readable() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(&db, "old visible thought", 0, None).await;
    // Simulate a future provenance spelling in the existing stored JSON format.
    db.with_conn(|conn| {
        conn.execute("UPDATE task_traces SET event_json = json_set(event_json, '$.thought_provenance', 'future_provider')", [])
            .map_err(|e| crate::error::AlephError::config(e.to_string()))?;
        Ok(())
    }).await.unwrap();
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    assert_eq!(rows[0]["text"], "old visible thought");
    assert!(rows[0].get("thought_block_ref").is_none());
    let old = serde_json::json!({ "id":1, "task_id":"run-a", "step_index":0,
        "event": {"kind":"reasoning_emitted", "iteration":1, "text":"legacy"}, "timestamp":0 });
    let trace: crate::resilience::TaskTrace = serde_json::from_value(old).unwrap();
    assert!(trace.thought_provenance.is_none());
}
