use crate::capability::ownership::OwnershipTree;
use crate::gateway::router::SessionKey;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::runtime::{
    MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig, MemoryState,
};
use crate::memory::session_search_summary::lookup::retrieve_summary_fact;
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::providers::MockProvider;
use crate::sync_primitives::Arc;

use super::{SessionManager, SessionManagerConfig};

#[tokio::test]
async fn session_close_preserves_raw_off_and_drains_derived_when_running() {
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
    let mut config = crate::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    config.memory.reflection.enabled = false;
    runtime
        .configure_services(MemoryServicesConfig {
            app_config: Arc::new(tokio::sync::RwLock::new(config)),
            raw_store: raw.clone(),
            session_store: Some(sessions.clone()),
            provider: Some(Arc::new(MockProvider::new("A completed session."))),
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: crate::routing::DEFAULT_AGENT_ID.into(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("snapshots"),
        })
        .unwrap();

    let cold = SessionKey::main("cold-agent");
    sessions.get_or_create(&cold).await.unwrap();
    sessions
        .add_message(&cold, "user", "Keep raw chat while Memory is off.")
        .await
        .unwrap();
    sessions.close_session(&cold, None).await.unwrap();
    assert!(
        raw.get_unprocessed_raw_memories("cold-agent", 20)
            .await
            .unwrap()
            .iter()
            .any(|row| matches!(row.source, RawMemorySource::SessionEnd { .. })),
        "cold session close must observe its actual archive before returning"
    );
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert_eq!(sessions.get_history(&cold, None).await.unwrap().len(), 1);

    runtime.start().await.unwrap();
    let agent = crate::routing::DEFAULT_AGENT_ID;
    let warm = SessionKey::main(agent);
    let session = warm.to_key_string();
    sessions.get_or_create(&warm).await.unwrap();
    sessions
        .add_message(
            &warm,
            "user",
            "Retain the admitted summary until it is durable.",
        )
        .await
        .unwrap();
    raw.insert_raw_memory(
        &RawMemory::new(
            "A completed session.".into(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent(agent)
        .with_session(&session)
        .with_path(format!("aleph://session/{session}/d2/0")),
    )
    .await
    .unwrap();
    sessions.close_session(&warm, None).await.unwrap();
    runtime.stop().await.unwrap();
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert_eq!(
        retrieve_summary_fact(&(raw.clone() as Arc<dyn RawMemoryStore>), agent, &session)
            .await
            .unwrap()
            .map(|row| row.content),
        Some("A completed session.".into()),
        "session close must admit and retain its real derived summary before returning"
    );
    assert!(
        root.path().join("snapshots").exists(),
        "resume snapshots must use the bundle-owned path"
    );
    assert_eq!(sessions.get_history(&warm, None).await.unwrap().len(), 1);
    let source = Arc::downgrade(&runtime);
    drop(runtime);
    assert!(
        source.upgrade().is_none(),
        "the injected session store must not retain its Memory source"
    );
}
