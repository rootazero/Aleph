use super::{handle_owned_reembed, ReembedState};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::JsonRpcRequest;
use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::memory::store::SqliteMemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::{Arc, AtomicBool, Ordering};
use std::future::Future;
use std::time::Duration;
use tokio::sync::Notify;

#[tokio::test]
async fn reembed_stop_observes_migration_and_forwarder() {
    let root = tempfile::TempDir::new().unwrap();
    let metadata = root.path().join("metadata.db");
    let note_dir = root.path().join("notes");
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: metadata.clone(),
            note_memory_dir: note_dir.clone(),
        },
        Arc::new(crate::capability::ownership::OwnershipTree::new()),
    );
    runtime.start().await.unwrap();
    let operation = runtime.operation().unwrap();
    let indexer = NoteIndexer::new(note_dir.clone(), operation.backend().clone());
    operation
        .run(async move {
            indexer
                .write_note(
                    "default",
                    "reference",
                    &KnowledgeNote {
                        title: "owned-reembed".into(),
                        category: "reference".into(),
                        facts: vec!["A migration must finish its actual vector write.".into()],
                        ..KnowledgeNote::default()
                    },
                )
                .await?;
            Ok(())
        })
        .await
        .unwrap();
    let migration = Arc::new((Notify::new(), Notify::new(), AtomicBool::new(false)));
    let progress = Arc::new((Notify::new(), Notify::new(), AtomicBool::new(false)));
    let mut state = ReembedState::new();
    state.migration_gate = Some(migration.clone());
    state.progress_gate = Some(progress.clone());
    let state = Arc::new(state);
    let events = Arc::new(GatewayEventBus::new());
    let mut notifications = events.subscribe();
    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(MockEmbeddingProvider::new(1024, "owned-lifecycle"));
    let expected_signature =
        crate::memory::embedding_signature::provider_signature(embedder.as_ref());
    let response = handle_owned_reembed(
        JsonRpcRequest::new("memory.reembed", None, Some(serde_json::json!(1))),
        operation,
        note_dir,
        embedder,
        events,
        state.clone(),
    )
    .await;
    assert!(response.is_success(), "{:?}", response.error);
    tokio::time::timeout(Duration::from_secs(10), migration.0.notified())
        .await
        .unwrap();
    assert!(
        migration.2.load(Ordering::SeqCst),
        "the migration must carry admission"
    );
    {
        let mut stop = Box::pin(runtime.stop());
        assert!(stop
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
            .is_pending());
        assert_eq!(runtime.status().state, MemoryState::Draining);
    }
    migration.1.notify_one();
    tokio::time::timeout(Duration::from_secs(10), progress.0.notified())
        .await
        .unwrap();
    assert!(
        progress.2.load(Ordering::SeqCst),
        "the forwarder must carry the same admission"
    );
    let completion = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let message: serde_json::Value =
                serde_json::from_str(&notifications.recv().await.unwrap()).unwrap();
            if message["topic"] == "memory.reembed.completed" {
                break message;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(completion["data"]["facts_updated"], 1);
    assert_eq!(completion["data"]["errors"], serde_json::json!([]));
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.running.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    {
        let mut retry = Box::pin(runtime.stop());
        assert!(
            retry
                .as_mut()
                .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
                .is_pending(),
            "a finished migration must not detach its blocked progress forwarder"
        );
    }
    progress.1.notify_one();
    let stopped = tokio::time::timeout(Duration::from_secs(10), runtime.stop())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.state, MemoryState::Off);
    let forwarded: serde_json::Value = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(10), notifications.recv())
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(forwarded["topic"], "memory.reembed.progress");
    assert_eq!(forwarded["data"]["completed"], 1);
    let check = SqliteMemoryBackend::new(&metadata).unwrap();
    assert!(check
        .stale_vector_paths("default")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        check.get_embedding_signature().unwrap().as_deref(),
        Some(expected_signature.as_str())
    );
    check.close().unwrap();
}
