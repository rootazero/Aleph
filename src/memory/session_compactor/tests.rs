use super::*;

#[test]
fn extract_depth_d0() {
    assert_eq!(extract_depth("aleph://session/abc/d0/3"), 0);
}

#[test]
fn extract_depth_d1() {
    assert_eq!(extract_depth("aleph://session/abc/d1/0"), 1);
}

#[test]
fn extract_depth_d2() {
    assert_eq!(extract_depth("aleph://session/abc/d2/1"), 2);
}

#[test]
fn extract_depth_missing_returns_zero() {
    assert_eq!(extract_depth("aleph://user/preferences/"), 0);
}

#[test]
fn extract_depth_complex_session_id() {
    assert_eq!(extract_depth("aleph://session/agent:main:main/d1/5"), 1);
}

#[test]
fn compress_result_default_is_zero() {
    let r = CompressResult::default();
    assert_eq!(r.d0_created, 0);
    assert_eq!(r.d1_created, 0);
    assert_eq!(r.d2_created, 0);
}

/// Regression (writer/reader identity coherence): `post_turn_compress` runs
/// inside a `tokio::spawn` fired after the run-loop's `projects::run_context`
/// scope has closed, so it must resolve the project-scoped storage agent id
/// from the project root the caller captured before the spawn — the task-local
/// is always `None` across a spawn boundary. The old task-local read made the
/// writer land on the BASE id while the in-turn readers (`prepare_history`,
/// `memory_search`, `recall_context`) resolved the SCOPED id, leaving
/// project-scoped recall silently empty.
#[tokio::test]
async fn post_turn_compress_scopes_writes_by_the_explicit_project_root() {
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, MessageRole};
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::memory::store::raw_memory::RawMemoryStore;
    use crate::memory::store::MemoryBackend;
    use crate::routing::session_key::SessionKey;
    use crate::sync_primitives::Arc;

    let temp = tempfile::tempdir().unwrap();
    let session_store = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        })
        .unwrap(),
    );
    let agent = AgentInstance::new(
        AgentInstanceConfig {
            agent_id: "alice".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agent"),
            ..Default::default()
        },
        session_store,
    )
    .unwrap();

    // Seed more messages than the fresh tail so a compressible head exists.
    let key = SessionKey::main("alice");
    for i in 0..4 {
        agent
            .add_message(&key, MessageRole::User, &format!("user turn {i}"))
            .await;
        agent
            .add_message(&key, MessageRole::Assistant, &format!("assistant turn {i}"))
            .await;
    }

    let database: MemoryBackend =
        Arc::new(crate::memory::store::sqlite::SqliteMemoryBackend::in_memory().unwrap());
    // No provider — the deterministic fallback summarizer is enough here.
    let compactor = SessionCompactor::new(
        database.clone(),
        SessionCompactorConfig {
            fresh_tail_count: 2,
            ..Default::default()
        },
    )
    .with_project_scoping(true);

    // Deliberately NO `with_project_root` scope around this call — it mirrors
    // the production spawn where the task-local is dead. The explicit root
    // alone must drive the scoping.
    let project_root = temp.path().join("proj");
    let result = compactor
        .post_turn_compress(&agent, &key, Some(&project_root))
        .await
        .unwrap();
    assert!(
        result.d0_created > 0,
        "compressible head must yield d0 summaries"
    );

    let scoped_id =
        crate::memory::project_scope::scoped_or_base("alice", true, Some(&project_root));
    assert_ne!(
        scoped_id, "alice",
        "scoping must compose a project namespace"
    );
    let prefix = format!("aleph://session/{}/", key.to_key_string());
    let under_scoped = database
        .get_raw_by_path_prefix(&prefix, &scoped_id, 50)
        .await
        .unwrap();
    assert!(
        !under_scoped.is_empty(),
        "writes must land under the project-scoped id the in-turn readers resolve"
    );
    let under_base = database
        .get_raw_by_path_prefix(&prefix, "alice", 50)
        .await
        .unwrap();
    assert!(
        under_base.is_empty(),
        "no session write may leak to the base id while a project root is active"
    );
}
