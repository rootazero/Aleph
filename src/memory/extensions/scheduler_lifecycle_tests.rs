use std::task::Poll;

use crate::capability::ownership::OwnershipTree;
use crate::error::AlephError;
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision, ProduceCtx};
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::sync_primitives::{Arc, AtomicBool, Ordering};
use async_trait::async_trait;
use tokio::sync::Notify;

use super::{MemoryExtensionRegistry, MemoryProducerScheduler};

struct GatedProducer {
    entered: Notify,
    release: Notify,
    admitted: AtomicBool,
}

#[async_trait]
impl MemoryExtension for GatedProducer {
    fn name(&self) -> &str {
        "test.memory-owned-producer"
    }

    async fn produce(&self, _ctx: &ProduceCtx) -> Result<Vec<RawMemory>, AlephError> {
        let operation = MemoryOperation::current();
        self.admitted.store(operation.is_some(), Ordering::Release);
        self.entered.notify_one();
        self.release.notified().await;
        let operation = operation.ok_or_else(|| AlephError::other("producer has no admission"))?;
        operation
            .backend()
            .with_conn(|connection| {
                connection
                    .execute_batch(
                        "CREATE TABLE producer_tick_effect(value INTEGER); \
                         INSERT INTO producer_tick_effect VALUES (1)",
                    )
                    .map_err(|error| AlephError::other(error.to_string()))
            })
            .await?;
        Ok(vec![RawMemory::new(
            "source-owned produced memory".into(),
            RawMemorySource::Transcript,
        )
        .with_agent("main")])
    }

    async fn on_capture(
        &self,
        _ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        if MemoryOperation::current().is_none() {
            return Err(AlephError::other("capture lost the producer admission"));
        }
        raw.content.push_str(" | captured");
        Ok(CaptureDecision::Allow)
    }
}

#[tokio::test]
async fn producer_tick_drains_capture_filter_insert() {
    let directory = tempfile::tempdir().unwrap();
    let raw = Arc::new(MarkdownSessionStore::new(directory.path()).await.unwrap());
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: directory.path().join("metadata.db"),
            note_memory_dir: directory.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    runtime.start().await.unwrap();
    let producer = Arc::new(GatedProducer {
        entered: Notify::new(),
        release: Notify::new(),
        admitted: AtomicBool::new(false),
    });
    let registry = Arc::new(MemoryExtensionRegistry::new());
    registry.register(producer.clone()).unwrap();
    let mut scheduler = MemoryProducerScheduler::new(registry, raw.clone());
    scheduler.tick_duration = std::time::Duration::from_millis(1);
    let scheduler = Arc::new(scheduler);
    runtime.retain_worker(
        scheduler
            .clone()
            .spawn_owned(runtime.clone(), runtime.worker_stop().unwrap()),
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        producer.entered.notified(),
    )
    .await
    .unwrap();
    assert!(
        producer.admitted.load(Ordering::Acquire),
        "the complete producer tick must carry its source admission"
    );
    let mut stop = Box::pin(runtime.stop());
    assert!(matches!(futures::poll!(&mut stop), Poll::Pending));
    assert_eq!(runtime.status().state, MemoryState::Draining);
    assert_eq!(raw.count_unprocessed("main").await.unwrap(), 0);
    drop(stop);
    let mut retried = Box::pin(runtime.stop());
    assert!(matches!(futures::poll!(&mut retried), Poll::Pending));
    producer.release.notify_one();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), retried)
            .await
            .unwrap()
            .unwrap()
            .state,
        MemoryState::Off
    );
    assert_eq!(scheduler.tick_counter.load(Ordering::Relaxed), 1);
    let persisted = raw.get_unprocessed_raw_memories("main", 10).await.unwrap();
    assert_eq!(persisted.len(), 1);
    assert!(persisted[0].content.ends_with(" | captured"));
    let connection = rusqlite::Connection::open(directory.path().join("metadata.db")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT SUM(value) FROM producer_tick_effect", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
