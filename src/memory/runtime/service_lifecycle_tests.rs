use crate::capability::ownership::OwnershipTree;
use crate::config::types::memory::EmbeddingProviderConfig;
use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
use crate::memory::compression::CompressionTrigger;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::session_search_summary::lookup::retrieve_summary_fact;
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::providers::MockProvider;
use crate::sync_primitives::{Arc, Ordering};

use super::{MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig};

#[tokio::test]
async fn review_open_loops_use_each_activation_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let raw = Arc::new(
        MarkdownSessionStore::new(&root.path().join("memory"))
            .await
            .unwrap(),
    );
    let agent = crate::routing::DEFAULT_AGENT_ID;
    let dir = root.path().join("agents").join(agent);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join("MEMORY.md"), "A durable fact.\n§\n")
        .await
        .unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    tokio::fs::write(
        dir.join("OPEN_LOOPS.md"),
        format!(
            "{}\n- activationatlas pending work\n",
            crate::memory::curated::snapshot::open_loops_capture_header(&today)
        ),
    )
    .await
    .unwrap();
    let mut config = crate::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    config.memory.reflection.enabled = true;
    config.memory.reflection.open_loop_inject_prompt = false;
    let app_config = Arc::new(tokio::sync::RwLock::new(config));
    let runtime = MemoryRuntime::new_with_services(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("memory/metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
        MemoryServicesConfig {
            app_config: app_config.clone(),
            raw_store: raw,
            session_store: None,
            provider: None,
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: agent.into(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("snapshots"),
        },
    );
    let context =
        crate::thinker::memory_context_provider::RuntimeMemoryContext::new(runtime.clone());
    runtime.start().await.unwrap();
    let first = context.build_curated_message(agent, "first").await.unwrap();
    assert!(!format!("{first:?}").contains("activationatlas"));
    app_config
        .write()
        .await
        .memory
        .reflection
        .open_loop_inject_prompt = true;
    let current = context
        .build_curated_message(agent, "current")
        .await
        .unwrap();
    assert!(!format!("{current:?}").contains("activationatlas"));
    runtime.stop().await.unwrap();
    runtime.start().await.unwrap();
    let next = context.build_curated_message(agent, "next").await.unwrap();
    runtime.stop().await.unwrap();
    assert!(
        format!("{next:?}").contains("activationatlas"),
        "the new activation must render its own open-loop policy"
    );
}

#[tokio::test]
async fn activation_builds_session_services_and_preserves_raw_storage() {
    let root = tempfile::tempdir().unwrap();
    let raw = Arc::new(
        MarkdownSessionStore::new(&root.path().join("memory"))
            .await
            .unwrap(),
    );
    let sessions = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: root.path().join("sessions.db"),
            ..Default::default()
        })
        .unwrap(),
    );
    let agent = crate::routing::DEFAULT_AGENT_ID;
    let key = SessionKey::main(agent);
    sessions.get_or_create(&key).await.unwrap();
    sessions
        .add_message(
            &key,
            "user",
            "Keep every admitted task in its resource owner.",
        )
        .await
        .unwrap();
    let session = key.to_key_string();
    raw.insert_raw_memory(
        &RawMemory::new(
            "A real completed conversation.".into(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent(agent)
        .with_session(&session)
        .with_path(format!("aleph://session/{session}/d2/0")),
    )
    .await
    .unwrap();
    let mut config = crate::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    config.memory.reflection.enabled = true;
    config.memory.reflection.min_turns = 0;
    config.memory.reflection.min_user_chars = 0;
    config.memory.reflection.cooldown_minutes = 0;
    config.memory.reflection.open_loop_tracking = false;
    let runtime = MemoryRuntime::new_with_services(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("memory/metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
        MemoryServicesConfig {
            app_config: Arc::new(tokio::sync::RwLock::new(config)),
            raw_store: raw.clone(),
            session_store: Some(sessions.clone()),
            provider: Some(Arc::new(MockProvider::new(
                "Retain admitted tasks through shutdown.",
            ))),
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: agent.into(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("snapshots"),
        },
    );
    runtime.start().await.unwrap();
    let operation = runtime.operation().unwrap();
    let services = operation.services().unwrap();
    let old_services = Arc::downgrade(&services);
    let session_for_work = session.clone();
    operation
        .run(async move {
            if let Some(summarizer) = &services.summarizer {
                summarizer.produce(agent, &session_for_work).await?;
            }
            if let Some(reflector) = &services.reflector {
                reflector.reflect(agent, &session_for_work).await?;
            }
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        retrieve_summary_fact(&(raw.clone() as Arc<dyn RawMemoryStore>), agent, &session)
            .await
            .unwrap()
            .map(|row| row.content),
        Some("A real completed conversation.".into()),
        "the source-owned factory must wire the real session summary producer"
    );
    assert_eq!(
        raw.get_unprocessed_raw_memories(agent, 50)
            .await
            .unwrap()
            .iter()
            .filter(|row| matches!(row.source, RawMemorySource::Reflection))
            .count(),
        1,
        "the same admitted bundle must produce a real reflection row"
    );
    drop(operation);
    runtime.stop().await.unwrap();
    assert!(old_services.upgrade().is_none());
    raw.insert_raw_memory(
        &RawMemory::new(
            "Chat archival remains available.".into(),
            RawMemorySource::Transcript,
        )
        .with_agent(agent)
        .with_session(&session),
    )
    .await
    .unwrap();
    assert_eq!(sessions.get_history(&key, None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn compression_factory_uses_the_same_activation_policy() {
    let root = tempfile::tempdir().unwrap();
    let raw = Arc::new(
        MarkdownSessionStore::new(&root.path().join("memory"))
            .await
            .unwrap(),
    );
    let mut config = crate::Config::default();
    let mut embedder = EmbeddingProviderConfig::ollama();
    embedder.api_base = "http://127.0.0.1:9/v1".into();
    config.memory.embedding.active_provider_id = embedder.id.clone();
    config.memory.embedding.providers = vec![embedder];
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    config.policies.memory.compression.turn_threshold = 3;
    let app_config = Arc::new(tokio::sync::RwLock::new(config));
    let runtime = MemoryRuntime::new_with_services(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("memory/metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
        MemoryServicesConfig {
            app_config: app_config.clone(),
            raw_store: raw,
            session_store: None,
            provider: Some(Arc::new(MockProvider::new("unused"))),
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: crate::routing::DEFAULT_AGENT_ID.into(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("snapshots"),
        },
    );
    for threshold in [3, 5] {
        app_config
            .write()
            .await
            .policies
            .memory
            .compression
            .turn_threshold = threshold;
        runtime.start().await.unwrap();
        let operation = runtime.operation().unwrap();
        let services = operation.services().unwrap();
        let compression = services
            .compression
            .as_ref()
            .expect("activation must construct its compression service");
        let old = Arc::downgrade(compression);
        let scheduler = compression.get_scheduler();
        runtime.record_turn().await;
        assert_eq!(
            scheduler.pending_turns.load(Ordering::Acquire),
            1,
            "the source entry must record this activation's turn"
        );
        scheduler
            .pending_turns
            .store(threshold - 1, Ordering::Release);
        assert!(matches!(
            scheduler.should_trigger_compression(),
            CompressionTrigger::None
        ));
        scheduler.pending_turns.store(threshold, Ordering::Release);
        assert!(
            matches!(scheduler.should_trigger_compression(), CompressionTrigger::TurnThreshold(value) if value == threshold)
        );
        drop(scheduler);
        drop(services);
        drop(operation);
        runtime.stop().await.unwrap();
        assert!(
            old.upgrade().is_none(),
            "the stopped bundle must release its compression service"
        );
    }
}

#[tokio::test]
async fn activation_drains_real_note_maintenance_before_stop_completes() {
    use crate::memory::notes::store::NoteStore;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    let root = tempfile::tempdir().unwrap();
    let raw = Arc::new(
        MarkdownSessionStore::new(&root.path().join("memory"))
            .await
            .unwrap(),
    );
    let agent = crate::routing::DEFAULT_AGENT_ID;
    let notes = root.path().join("notes");
    let category = notes.join(agent).join("reference");
    tokio::fs::create_dir_all(&category).await.unwrap();
    tokio::fs::write(
        category.join("startup.md"),
        "---\ncategory: reference\ntags: [lifecycle]\ncreated: 2026-04-01\nupdated: 2026-04-10\n---\n\n- Retain the real startup work until shutdown completes.\n",
    )
    .await
    .unwrap();
    let mut config = crate::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    let metadata = root.path().join("memory/metadata.db");
    let runtime = MemoryRuntime::new_with_services(
        MemoryRuntimeConfig {
            metadata_path: metadata.clone(),
            note_memory_dir: notes.clone(),
        },
        Arc::new(OwnershipTree::new()),
        MemoryServicesConfig {
            app_config: Arc::new(tokio::sync::RwLock::new(config)),
            raw_store: raw,
            session_store: None,
            provider: None,
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: agent.into(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("snapshots"),
        },
    );
    runtime.start().await.unwrap();
    runtime.stop().await.unwrap();
    let inspected = SqliteMemoryBackend::new(&metadata).unwrap();
    assert_eq!(
        inspected.count_all_notes().await.unwrap(),
        1,
        "stop must observe the actual activation rebuild"
    );
    inspected.close().unwrap();
    let agent_dir = notes.join(agent);
    assert!(agent_dir.join("SCHEMA.md").exists());
    assert!(agent_dir.join("index.md").exists());
    assert!(agent_dir.join("log.md").exists());
}
