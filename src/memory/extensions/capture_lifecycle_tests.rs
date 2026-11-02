use super::insert_with_runtime_capture_filter;
use crate::capability::ownership::OwnershipTree;
use crate::error::AlephError;
use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, ToolRegistry};
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision};
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::namespace::NamespaceScope;
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use async_trait::async_trait;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

struct Capture {
    entered: Notify,
    release: Arc<Semaphore>,
    gated: bool,
    admitted: AtomicBool,
}

#[async_trait]
impl MemoryExtension for Capture {
    fn name(&self) -> &str {
        "capture-lifecycle-effect"
    }

    async fn on_capture(
        &self,
        _ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        self.admitted
            .store(MemoryOperation::current().is_some(), Ordering::SeqCst);
        if self.gated {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        raw.content = format!("derived {}", raw.content);
        Ok(CaptureDecision::Allow)
    }
}

struct Release(Arc<Semaphore>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.add_permits(1);
    }
}

fn context() -> CaptureCtx {
    CaptureCtx {
        agent_id: "owner".into(),
        namespace: NamespaceScope::Owner,
        session_id: Some("capture-session".into()),
        source_hint: "transcript".into(),
    }
}

#[tokio::test]
async fn raw_capture_off_preserves_real_archive_without_derived_mutation() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn RawMemoryStore> = Arc::new(
        MarkdownSessionStore::new(&root.path().join("raw"))
            .await
            .unwrap(),
    );
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("derived.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    let registry = Arc::new(MemoryExtensionRegistry::new());
    registry
        .register(Arc::new(Capture {
            entered: Notify::new(),
            release: Arc::new(Semaphore::new(0)),
            gated: false,
            admitted: AtomicBool::new(false),
        }))
        .unwrap();
    insert_with_runtime_capture_filter(
        &store,
        &registry,
        Some(&Arc::downgrade(&runtime)),
        &context(),
        RawMemory::new("raw archive".into(), RawMemorySource::Transcript).with_agent("owner"),
    )
    .await
    .unwrap();
    let rows = store
        .get_unprocessed_raw_memories("owner", 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].content, "raw archive",
        "Off must skip derived capture filters"
    );
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert!(!root.path().join("derived.db").exists());
}

#[tokio::test]
async fn raw_capture_cancelled_caller_keeps_filter_and_archive_owned() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn RawMemoryStore> = Arc::new(
        MarkdownSessionStore::new(&root.path().join("raw"))
            .await
            .unwrap(),
    );
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("derived.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    runtime.start().await.unwrap();
    let capture = Arc::new(Capture {
        entered: Notify::new(),
        release: Arc::new(Semaphore::new(0)),
        gated: true,
        admitted: AtomicBool::new(false),
    });
    let release = Release(capture.release.clone());
    let registry = Arc::new(MemoryExtensionRegistry::new());
    registry.register(capture.clone()).unwrap();
    let ctx = context();
    let owner = Arc::downgrade(&runtime);
    let mut work = Box::pin(insert_with_runtime_capture_filter(
        &store,
        &registry,
        Some(&owner),
        &ctx,
        RawMemory::new("raw archive".into(), RawMemorySource::Transcript).with_agent("owner"),
    ));
    tokio::select! {
        result = &mut work => panic!("capture completed before release: {result:?}"),
        _ = async {
            tokio::time::timeout(Duration::from_secs(10), capture.entered.notified())
                .await
                .unwrap();
        } => {}
    }
    if !capture.admitted.load(Ordering::SeqCst) {
        drop(release);
        work.await.unwrap();
        runtime.stop().await.unwrap();
        panic!("the actual capture callback must carry source admission");
    }
    drop(work);
    let mut stop = Box::pin(runtime.stop());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(stop.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert_eq!(runtime.status().state, MemoryState::Draining);
    drop(stop);
    drop(release);
    tokio::time::timeout(Duration::from_secs(10), runtime.stop())
        .await
        .unwrap()
        .unwrap();
    let rows = store
        .get_unprocessed_raw_memories("owner", 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "derived raw archive");
    assert_eq!(runtime.status().state, MemoryState::Off);
}

#[tokio::test]
async fn raw_capture_compactor_rebuild_keeps_cold_and_expired_source_binding() {
    let root = tempfile::tempdir().unwrap();
    let original: Arc<dyn RawMemoryStore> = Arc::new(
        MarkdownSessionStore::new(&root.path().join("original"))
            .await
            .unwrap(),
    );
    let store: Arc<dyn RawMemoryStore> = Arc::new(
        MarkdownSessionStore::new(&root.path().join("replacement"))
            .await
            .unwrap(),
    );
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("derived.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    let owner = Arc::downgrade(&runtime);
    let registry = Arc::new(MemoryExtensionRegistry::new());
    registry
        .register(Arc::new(Capture {
            entered: Notify::new(),
            release: Arc::new(Semaphore::new(0)),
            gated: false,
            admitted: AtomicBool::new(false),
        }))
        .unwrap();
    let compactor = crate::memory::session_compactor::SessionCompactor::new(
        original.clone(),
        Default::default(),
    )
    .with_memory_runtime(owner.clone())
    .with_capture_registry(registry)
    .with_transcript_indexing()
    .with_raw_memory_writer(store.clone());
    compactor
        .store_raw_chunk("capture-session", "owner", 0, "cold raw")
        .await
        .unwrap();
    let ids = compactor
        .indexer
        .as_ref()
        .unwrap()
        .index_turn_text(
            "capture-session",
            0,
            "cold transcript",
            "",
            "owner",
            "owner",
        )
        .await;
    assert_eq!(ids.len(), 1);
    drop(runtime);
    assert!(owner.upgrade().is_none());
    compactor
        .store_raw_chunk("capture-session", "owner", 1, "expired raw")
        .await
        .unwrap();
    let rows = store
        .get_unprocessed_raw_memories("owner", 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| !row.content.starts_with("derived ")));
    assert_eq!(original.count_unprocessed("owner").await.unwrap(), 0);
    assert!(!root.path().join("derived.db").exists());
}

#[tokio::test]
async fn raw_capture_registered_session_complete_respects_off_on_off() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn RawMemoryStore> = Arc::new(
        MarkdownSessionStore::new(&root.path().join("raw"))
            .await
            .unwrap(),
    );
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("derived.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    let extensions = Arc::new(MemoryExtensionRegistry::new());
    let capture = Arc::new(Capture {
        entered: Notify::new(),
        release: Arc::new(Semaphore::new(0)),
        gated: false,
        admitted: AtomicBool::new(false),
    });
    extensions.register(capture.clone()).unwrap();
    let registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
        raw_memory_store: Some(store.clone()),
        memory_runtime: Some(runtime.clone()),
        capture_registry: Some(extensions),
        ..Default::default()
    })
    .await
    .unwrap();
    for (index, running) in [false, true, false].into_iter().enumerate() {
        if running {
            runtime.start().await.unwrap();
        }
        if index == 2 {
            runtime.stop().await.unwrap();
        }
        let result = registry
            .execute_tool(
                "session_complete",
                serde_json::json!({
                    "outcome": format!("completion-{index}"),
                }),
            )
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        let rows = store
            .get_unprocessed_raw_memories(crate::routing::DEFAULT_AGENT_ID, 10)
            .await
            .unwrap();
        let row = rows
            .iter()
            .find(|row| row.content.contains(&format!("completion-{index}")))
            .unwrap();
        assert_eq!(
            row.content.starts_with("derived "),
            running,
            "the actual registered tool must admit only its derived callback"
        );
        if index == 0 {
            assert!(!root.path().join("derived.db").exists());
        }
    }
    assert!(capture.admitted.load(Ordering::SeqCst));
    assert_eq!(runtime.status().state, MemoryState::Off);
}
