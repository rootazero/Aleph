use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;

use super::spawn_note_vault_watcher;
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::SqliteMemoryBackend;

struct BlockingEmbedder {
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl EmbeddingProvider for BlockingEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, crate::error::AlephError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(vec![0.0; 768])
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, crate::error::AlephError> {
        Ok(texts.iter().map(|_| vec![0.0; 768]).collect())
    }

    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> &str {
        "watcher-test"
    }

    fn provider_id(&self) -> &str {
        "watcher-test"
    }
}

fn note_indexer(
    root: std::path::PathBuf,
    db_path: &std::path::Path,
) -> Arc<NoteIndexer<SqliteMemoryBackend>> {
    let db = Arc::new(SqliteMemoryBackend::new(db_path).unwrap());
    Arc::new(NoteIndexer::new(root, db))
}

async fn wait_until<F, Fut>(mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if condition().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn write_note(path: &std::path::Path, body: &str) {
    tokio::fs::write(
        path,
        format!("---\ncategory: reference\ntags: []\n---\n\n{body}\n"),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_waits_for_an_emitted_batch_and_releases_consumer_indexer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("note");
    let category = root.join("main/reference");
    tokio::fs::create_dir_all(&category).await.unwrap();

    let embedder = Arc::new(BlockingEmbedder {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
    let indexer = Arc::new(NoteIndexer::new(root.clone(), db).with_embedder(embedder.clone()));
    let mut watcher = spawn_note_vault_watcher(indexer.clone()).unwrap();

    write_note(
        &category.join("held.md"),
        "the consumer must finish this batch",
    )
    .await;
    assert!(
        wait_until(|| async {
            !indexer
                .store()
                .list_notes("main")
                .await
                .unwrap_or_default()
                .is_empty()
        })
        .await
    );
    embedder.entered.notified().await;

    assert!(
        tokio::time::timeout(Duration::from_millis(50), watcher.shutdown())
            .await
            .is_err()
    );
    assert!(Arc::strong_count(&indexer) >= 2);

    embedder.release.notify_one();
    watcher.shutdown().await.unwrap();
    assert_eq!(Arc::strong_count(&indexer), 1);
    assert_eq!(indexer.store().list_notes("main").await.unwrap().len(), 1);
    watcher.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_shutdown_retains_stop_and_consumer_handles() {
    let (stop_started_tx, stop_started_rx) = tokio::sync::oneshot::channel();
    let (stop_release_tx, stop_release_rx) = tokio::sync::oneshot::channel();
    let stop_handle = tokio::spawn(async move {
        stop_started_tx.send(()).unwrap();
        stop_release_rx.await.unwrap();
    });
    let (consumer_release_tx, consumer_release_rx) = tokio::sync::oneshot::channel();
    let consumer_handle = tokio::spawn(async move {
        consumer_release_rx.await.unwrap();
    });
    let mut watcher = super::NoteVaultWatcher {
        root: std::path::PathBuf::new(),
        debouncer: None,
        consumer_handle: Some(consumer_handle),
        stop_handle: Some(stop_handle),
        shutdown_failure: None,
    };
    let stop_id = watcher.stop_handle.as_ref().unwrap().id();
    let consumer_id = watcher.consumer_handle.as_ref().unwrap().id();

    let shutdown = tokio::time::timeout(Duration::from_millis(50), watcher.shutdown()).await;
    assert!(shutdown.is_err());
    assert_eq!(watcher.stop_handle.as_ref().unwrap().id(), stop_id);
    assert_eq!(watcher.consumer_handle.as_ref().unwrap().id(), consumer_id);

    stop_started_rx.await.unwrap();
    stop_release_tx.send(()).unwrap();
    let shutdown = tokio::time::timeout(Duration::from_millis(50), watcher.shutdown()).await;
    assert!(shutdown.is_err());
    assert!(watcher.stop_handle.is_none());
    assert_eq!(watcher.consumer_handle.as_ref().unwrap().id(), consumer_id);

    consumer_release_tx.send(()).unwrap();
    watcher.shutdown().await.unwrap();
    assert!(watcher.stop_handle.is_none());
    assert!(watcher.consumer_handle.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn consumer_join_failure_stays_failed_after_producer_stop() {
    let stop_handle = tokio::spawn(async {});
    let consumer_handle = tokio::spawn(async {
        panic!("consumer lifecycle test panic");
    });
    let mut watcher = super::NoteVaultWatcher {
        root: std::path::PathBuf::new(),
        debouncer: None,
        consumer_handle: Some(consumer_handle),
        stop_handle: Some(stop_handle),
        shutdown_failure: None,
    };

    assert!(watcher.shutdown().await.is_err());
    assert!(watcher.shutdown().await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_helper_join_failure_still_observes_consumer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("note");
    tokio::fs::create_dir_all(root.join("main/reference"))
        .await
        .unwrap();
    let indexer = note_indexer(root, &dir.path().join("memory.db"));
    let mut watcher = spawn_note_vault_watcher(indexer).unwrap();
    drop(watcher.debouncer.take());
    watcher.stop_handle = Some(tokio::spawn(async {
        panic!("stop helper lifecycle test panic");
    }));

    assert!(watcher.shutdown().await.is_err());
    assert!(watcher.consumer_handle.is_none());
    assert!(watcher.shutdown().await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_and_consumer_completion_are_observed_independently() {
    let consumer_observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = consumer_observed.clone();
    let consumer_handle = tokio::spawn(async move {
        observed.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let stop_handle = tokio::spawn(async {
        panic!("producer lifecycle test panic");
    });
    let mut watcher = super::NoteVaultWatcher {
        root: std::path::PathBuf::new(),
        debouncer: None,
        consumer_handle: Some(consumer_handle),
        stop_handle: Some(stop_handle),
        shutdown_failure: None,
    };

    assert!(watcher.shutdown().await.is_err());
    assert!(consumer_observed.load(std::sync::atomic::Ordering::SeqCst));
}
#[tokio::test(flavor = "multi_thread")]
async fn successful_shutdown_is_idempotent_and_replacement_receives_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("note");
    let category = root.join("main/reference");
    tokio::fs::create_dir_all(&category).await.unwrap();

    let indexer = note_indexer(root.clone(), &dir.path().join("memory.db"));
    let mut first = spawn_note_vault_watcher(indexer.clone()).unwrap();
    write_note(&category.join("first.md"), "first").await;
    assert!(
        wait_until(|| async {
            indexer
                .store()
                .list_notes("main")
                .await
                .unwrap_or_default()
                .iter()
                .any(|note| note.path == "reference/first")
        })
        .await
    );
    first.shutdown().await.unwrap();
    first.shutdown().await.unwrap();

    let mut replacement = spawn_note_vault_watcher(indexer.clone()).unwrap();
    write_note(&category.join("second.md"), "second").await;
    assert!(
        wait_until(|| async {
            indexer
                .store()
                .list_notes("main")
                .await
                .unwrap_or_default()
                .iter()
                .any(|note| note.path == "reference/second")
        })
        .await
    );
    replacement.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_registration_failure_keeps_native_and_consumer_completion_observable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("notes");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let runtime = crate::memory::runtime::MemoryRuntime::new(
        crate::memory::runtime::MemoryRuntimeConfig {
            metadata_path: dir.path().join("owned.db"),
            note_memory_dir: root.clone(),
        },
        Arc::new(crate::capability::ownership::OwnershipTree::new()),
    );
    let mut watcher = super::prepare_owned_note_vault_watcher(
        note_indexer(root.clone(), &dir.path().join("standalone.db")),
        Arc::downgrade(&runtime),
    )
    .unwrap();
    let consumer = watcher.consumer_handle.as_ref().unwrap().id();
    tokio::fs::remove_dir(&root).await.unwrap();
    assert!(watcher.start_watching().is_err());
    assert!(watcher.debouncer.is_some());
    assert_eq!(watcher.consumer_handle.as_ref().unwrap().id(), consumer);
    watcher.shutdown().await.unwrap();
    assert!(watcher.debouncer.is_none());
    assert!(watcher.stop_handle.is_none());
    assert!(watcher.consumer_handle.is_none());
    assert_eq!(
        runtime.status().state,
        crate::memory::runtime::MemoryState::Off
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_root_has_no_resources_or_directory_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("missing-note");
    let indexer = note_indexer(root.clone(), &dir.path().join("memory.db"));

    assert!(spawn_note_vault_watcher(indexer).is_err());
    assert!(!root.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_is_nonblocking_fallback_not_joined_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("note");
    let category = root.join("main/reference");
    tokio::fs::create_dir_all(&category).await.unwrap();

    let embedder = Arc::new(BlockingEmbedder {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let indexer = Arc::new(
        NoteIndexer::new(
            root.clone(),
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap()),
        )
        .with_embedder(embedder.clone()),
    );
    let watcher = spawn_note_vault_watcher(indexer.clone()).unwrap();
    write_note(&category.join("drop.md"), "drop fallback").await;
    embedder.entered.notified().await;

    let started = std::time::Instant::now();
    drop(watcher);
    assert!(started.elapsed() < Duration::from_millis(200));
    assert!(Arc::strong_count(&indexer) >= 2);

    embedder.release.notify_one();
    assert!(wait_until(|| async { Arc::strong_count(&indexer) == 1 }).await);
}
