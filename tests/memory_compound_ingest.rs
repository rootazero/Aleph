//! End-to-end: a RecordingMockProvider emits a multi-op plan; CompoundIngestor
//! applies it; assert files on disk + SQLite reflect the plan.

#![cfg(feature = "test-helpers")]

use alephcore::memory::notes::ingest::{CompoundIngestor, DefaultCompoundIngestor, RelatedBudget};
use alephcore::memory::notes::store::NoteStore;
use alephcore::memory::notes::NoteIndexer;
use alephcore::memory::store::raw_memory::{RawMemory, RawMemorySource};
use alephcore::memory::{EmbeddingProvider, SqliteMemoryBackend};
use alephcore::providers::recording_mock::RecordingMockProvider;
use alephcore::providers::AiProvider;
use alephcore::AlephError;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Local mock embedding provider (MockEmbeddingProvider in embedding_provider.rs
// is pub(crate) inside #[cfg(test)] so it is not reachable from integration tests)
// ---------------------------------------------------------------------------

struct NoopEmbeddingProvider;

#[async_trait::async_trait]
impl EmbeddingProvider for NoopEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Ok(vec![0.0; 1024])
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Ok(texts.iter().map(|_| vec![0.0; 1024]).collect())
    }

    fn dimensions(&self) -> usize {
        1024
    }

    fn model_name(&self) -> &str {
        "noop"
    }

    fn provider_id(&self) -> &str {
        "noop"
    }
}

#[tokio::test]
async fn compound_ingest_creates_and_links_pages() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{
          "reasoning": "two pages + a link",
          "ops": [
            {"kind":"create","note_path":"learning/tokio","title":"tokio",
             "summary":"async runtime","facts":["event loop"],
             "links":["learning/rust-async"],"tags":["rust","async"]},
            {"kind":"create","note_path":"learning/rust-async","title":"rust-async",
             "summary":"futures + pin","facts":["futures are lazy"],
             "links":["learning/tokio"],"tags":["rust","async"]},
            {"kind":"link","from":"learning/tokio","to":"learning/rust-async"}
          ]
        }"#
        .into(),
    ));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(NoopEmbeddingProvider);
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        store: backend.clone(),
        indexer: indexer.clone(),
        provider,
        embedder,
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };

    let raw = RawMemory::new(
        "User mentions tokio and rust async together".to_string(),
        RawMemorySource::Transcript,
    );
    let report = ing
        .ingest_batch("default", vec![raw], None)
        .await
        .expect("ingest ok");

    assert_eq!(report.created, 2);
    assert_eq!(report.linked, 1);
    assert!(dir.path().join("note/default/learning/tokio.md").exists());
    assert!(dir
        .path()
        .join("note/default/learning/rust-async.md")
        .exists());

    let listed = NoteStore::list_notes(backend.as_ref(), "default")
        .await
        .unwrap();
    let paths: Vec<String> = listed.iter().map(|e| e.path.clone()).collect();
    assert!(paths.contains(&"learning/tokio".to_string()));
    assert!(paths.contains(&"learning/rust-async".to_string()));
}
