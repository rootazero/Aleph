use crate::capability::ownership::OwnershipTree;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::sync_primitives::Arc;

use super::{MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig, MemoryState};

#[tokio::test]
async fn activation_watcher_indexes_external_edits_and_stops_with_its_source() {
    let root = tempfile::tempdir().unwrap();
    let raw = Arc::new(
        MarkdownSessionStore::new(&root.path().join("memory"))
            .await
            .unwrap(),
    );
    let mut config = crate::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    let notes = root.path().join("notes");
    let runtime = MemoryRuntime::new_with_services(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("memory/metadata.db"),
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
            default_agent_id: crate::routing::DEFAULT_AGENT_ID.into(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("snapshots"),
        },
    );
    for title in ["external-first", "external-second"] {
        runtime.start().await.unwrap();
        // Wait for activation work to finish so only the actual native watcher
        // can discover the file written below.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if runtime.inner.lock().unwrap().admitted == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let agent = crate::routing::DEFAULT_AGENT_ID;
        let file = notes
            .join(agent)
            .join("reference")
            .join(format!("{title}.md"));
        tokio::fs::write(
            &file,
            "---\ncategory: reference\ntags: [lifecycle]\ncreated: 2026-04-01\nupdated: 2026-04-10\n---\n\n- Observe the native external edit before shutdown.\n",
        )
        .await
        .unwrap();
        let operation = runtime.operation().unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            operation.clone().scope(async {
                loop {
                    let indexed = operation.backend().count_all_notes().await.unwrap();
                    if indexed == if title == "external-first" { 1 } else { 2 } {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }),
        )
        .await
        .expect("the source-owned native watcher must index an edit after activation rebuild");
        drop(operation);
        runtime.stop().await.unwrap();
        assert_eq!(runtime.status().state, MemoryState::Off);
        assert!(runtime.inner.lock().unwrap().bundle.is_none());
    }
}
