//! Public-boundary lifecycle evidence; no test-only runtime getters or fake storage.

use std::sync::Arc;

use alephcore::capability::facade::Reference;
use alephcore::capability::ownership::{OwnershipTree, VisibilityScope};
use alephcore::error::AlephError;
use alephcore::executor::{BuiltinToolConfig, BuiltinToolRegistry, ToolRegistry};
use alephcore::gateway::{SessionManager, SessionManagerConfig};
use alephcore::memory::extensions::MemoryExtensionRegistry;
use alephcore::memory::notes::store::NoteStore;
use alephcore::memory::runtime::{
    MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig, MemoryState,
};
use alephcore::memory::store::markdown::MarkdownSessionStore;
use alephcore::memory::store::raw_memory::RawMemoryStore;
use alephcore::memory::store::sqlite::SqliteMemoryBackend;
use alephcore::routing::session_key::SessionKey;
use serde_json::json;

fn reference() -> Reference {
    Reference {
        id: alephcore::capability::descriptor::CapabilityId {
            namespace: "aleph/resources".into(),
            name: "memory.runtime".into(),
        },
        visibility: VisibilityScope::default(),
    }
}

#[tokio::test]
async fn registered_notes_restart_while_real_raw_sessions_remain_available() {
    let root = tempfile::tempdir().unwrap();
    let raw = Arc::new(
        MarkdownSessionStore::new(&root.path().join("memory"))
            .await
            .unwrap(),
    );
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("memory/metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    let sessions = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: root.path().join("sessions.db"),
            ..Default::default()
        })
        .unwrap()
        .with_raw_memory_writer(raw.clone())
        .with_memory_runtime(&runtime),
    );
    let mut config = alephcore::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    config.memory.project_scoped = false;
    runtime
        .configure_services(MemoryServicesConfig {
            app_config: Arc::new(tokio::sync::RwLock::new(config)),
            raw_store: raw.clone(),
            session_store: Some(sessions.clone()),
            provider: None,
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: alephcore::routing::DEFAULT_AGENT_ID.into(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("snapshots"),
        })
        .unwrap();
    let registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
        memory_runtime: Some(runtime.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert!(!root.path().join("notes").exists());
    for (index, title) in ["public-first", "public-second"].into_iter().enumerate() {
        assert!(registry
            .execute_tool(
                "note_manage",
                json!({
                    "action": "create", "category": "reference", "filename": "rejected-off",
                    "title": "Unavailable", "content": "Unadmitted content must not land."
                })
            )
            .await
            .is_err());
        runtime.start().await.unwrap();
        registry.execute_tool("note_manage", json!({
            "action": "create", "category": "reference", "filename": title,
            "title": "Public lifecycle evidence", "content": "Keep the actual source-owned note."
        })).await.unwrap();
        runtime.stop().await.unwrap();
        let database = SqliteMemoryBackend::new(&root.path().join("memory/metadata.db")).unwrap();
        assert_eq!(database.count_all_notes().await.unwrap(), index as i64 + 1);
        drop(database);
        assert!(!root
            .path()
            .join("notes")
            .join(alephcore::routing::DEFAULT_AGENT_ID)
            .join("reference/rejected-off.md")
            .exists());
        let agent = format!("raw-after-stop-{index}");
        let key = SessionKey::main(&agent);
        sessions.get_or_create(&key).await.unwrap();
        sessions
            .add_message(&key, "user", "Raw archival survives while Memory is Off.")
            .await
            .unwrap();
        sessions.close_session(&key, None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let rows = raw.get_unprocessed_raw_memories(&agent, 10).await.unwrap();
                if rows.iter().any(|row| {
                    row.session_id.as_deref() == Some(key.to_key_string().as_str())
                        && row.content.contains("Raw archival survives")
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("real session-owned archival must remain live after derived stop");
        assert_eq!(sessions.get_history(&key, None).await.unwrap().len(), 1);
    }
    drop(registry);
    let source = Arc::downgrade(&runtime);
    drop(runtime);
    assert!(
        source.upgrade().is_none(),
        "session-store injection must not retain its runtime in a cycle"
    );
}

#[tokio::test]
async fn cancelled_stop_waiter_keeps_the_actual_blocking_effect_owned() {
    let root = tempfile::tempdir().unwrap();
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    runtime.start().await.unwrap();
    let operation = runtime.capture(&reference()).unwrap().admit().unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let effect = root.path().join("blocking-effect.txt");
    let path = effect.clone();
    let mut work = Box::pin(operation.run_blocking(move || {
        let _ = entered.send(());
        wait.recv()
            .map_err(|error| AlephError::other(error.to_string()))?;
        std::fs::write(path, "actual blocking effect")
            .map_err(|error| AlephError::other(error.to_string()))
    }));
    assert!(futures::poll!(&mut work).is_pending());
    started.await.unwrap();
    drop(work);
    drop(operation);
    let mut stop = Box::pin(runtime.stop());
    assert!(futures::poll!(&mut stop).is_pending());
    assert_eq!(runtime.status().state, MemoryState::Draining);
    drop(stop);
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), runtime.stop())
            .await
            .unwrap()
            .unwrap()
            .state,
        MemoryState::Off
    );
    assert_eq!(
        std::fs::read_to_string(effect).unwrap(),
        "actual blocking effect"
    );
}
