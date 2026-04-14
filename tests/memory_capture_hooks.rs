//! Integration test: Spec 1 memory capture hooks end-to-end.
//!
//! Each case writes a `RawMemory` with a specific source variant, runs the
//! `CompressionService` once via the recording mock provider, and verifies
//! that the correct specialised system prompt was used for that source.
//!
//! Rather than relying on realistic LLM output (which would be flaky), the
//! mock returns a canned `NoteExtractionResponse` with one deterministic
//! update. The test asserts on (a) the prompt fingerprint and (b) that
//! the CompressionService applied the response (memories_processed == 1).

#![cfg(feature = "test-helpers")]

use alephcore::memory::compression::{CompressionConfig, CompressionService};
use alephcore::memory::notes::NoteIndexer;
use alephcore::memory::store::raw_memory::{
    RawMemory, RawMemorySource, RawMemoryStore, SessionEndReason,
};
use alephcore::memory::{EmbeddingProvider, SqliteMemoryBackend};
use alephcore::providers::recording_mock::RecordingMockProvider;
use alephcore::AlephError;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Local mock embedding provider (needed because MockEmbeddingProvider in
// embedding_provider.rs is pub(crate) inside #[cfg(test)])
// ---------------------------------------------------------------------------

struct NoopEmbeddingProvider;

#[async_trait::async_trait]
impl EmbeddingProvider for NoopEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Ok(vec![0.0; 256])
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Ok(texts.iter().map(|_| vec![0.0; 256]).collect())
    }

    fn dimensions(&self) -> usize {
        256
    }

    fn model_name(&self) -> &str {
        "noop"
    }

    fn provider_id(&self) -> &str {
        "noop"
    }
}

// ---------------------------------------------------------------------------
// Harness: build a CompressionService + NoteIndexer backed by in-memory SQLite
// ---------------------------------------------------------------------------

async fn build_service_with_recording_provider(
    canned_response: &str,
) -> (
    CompressionService,
    Arc<Mutex<Option<String>>>,
    Arc<SqliteMemoryBackend>,
    NoteIndexer<SqliteMemoryBackend>,
    tempfile::TempDir,
) {
    let temp_dir = tempdir().unwrap();

    // Separate DB files so the compression store and note index don't share
    // the same connection.
    let db_path = temp_dir.path().join("compression.db");
    let database: Arc<SqliteMemoryBackend> = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());

    let notes_db_path = temp_dir.path().join("notes.db");
    let notes_backend: Arc<SqliteMemoryBackend> =
        Arc::new(SqliteMemoryBackend::new(&notes_db_path).unwrap());
    let memory_dir = temp_dir.path().join("memory");
    let indexer: NoteIndexer<SqliteMemoryBackend> = NoteIndexer::new(memory_dir, notes_backend);

    // Recording mock provider
    let mock_provider = RecordingMockProvider::new(canned_response.to_string());
    let recorded_prompt: Arc<Mutex<Option<String>>> = mock_provider.recorded_system_prompt();

    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(NoopEmbeddingProvider);

    let config = CompressionConfig::default();

    let service =
        CompressionService::new(database.clone(), Arc::new(mock_provider), embedder, config);

    (service, recorded_prompt, database, indexer, temp_dir)
}

// ---------------------------------------------------------------------------
// Test cases
// ---------------------------------------------------------------------------

/// Correct canned JSON for `NoteExtractionResponse`:
///   { "updates": [{ "note_path": "category/name", "action": "create",
///                   "new_facts": ["..."], "links": [] }] }

#[tokio::test]
async fn pre_compress_source_uses_rescue_prompt() {
    let canned = r#"{"updates":[{"note_path":"learning/x","action":"create","new_facts":["f"],"links":[]}]}"#;
    let (svc, recorded_prompt, raw_store, indexer, _temp_dir) =
        build_service_with_recording_provider(canned).await;

    let raw = RawMemory::new(
        "user: decided X\nassistant: noted.".into(),
        RawMemorySource::PreCompress,
    )
    .with_agent("agent-1");
    raw_store.insert_raw_memory(&raw).await.unwrap();

    let result = svc.compress_to_notes("agent-1", &indexer).await.unwrap();
    assert_eq!(result.memories_processed, 1);

    let prompt = recorded_prompt.lock().unwrap().clone().unwrap();
    assert!(
        prompt.contains("memory rescue assistant"),
        "expected RESCUE prompt, got: {prompt}"
    );
}

#[tokio::test]
async fn delegation_source_uses_lesson_prompt() {
    let canned =
        r#"{"updates":[{"note_path":"lesson/x","action":"create","new_facts":["f"],"links":[]}]}"#;
    let (svc, recorded_prompt, raw_store, indexer, _temp_dir) =
        build_service_with_recording_provider(canned).await;

    let raw = RawMemory::new(
        "DELEGATION_PROMPT: do thing\nDELEGATION_RESULT: done".into(),
        RawMemorySource::Delegation {
            child_agent_id: "child-1".into(),
        },
    )
    .with_agent("agent-2");
    raw_store.insert_raw_memory(&raw).await.unwrap();

    let result = svc.compress_to_notes("agent-2", &indexer).await.unwrap();
    assert_eq!(result.memories_processed, 1);

    let prompt = recorded_prompt.lock().unwrap().clone().unwrap();
    assert!(
        prompt.contains("delegating work to a sub-agent"),
        "expected LESSON prompt, got: {prompt}"
    );
}

#[tokio::test]
async fn session_end_disconnect_uses_digest_prompt() {
    let canned = r#"{"updates":[{"note_path":"preference/x","action":"create","new_facts":["f"],"links":[]}]}"#;
    let (svc, recorded_prompt, raw_store, indexer, _temp_dir) =
        build_service_with_recording_provider(canned).await;

    let raw = RawMemory::new(
        "user: bye\nassistant: see you".into(),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::Disconnect,
        },
    )
    .with_agent("agent-3");
    raw_store.insert_raw_memory(&raw).await.unwrap();

    let result = svc.compress_to_notes("agent-3", &indexer).await.unwrap();
    assert_eq!(result.memories_processed, 1);

    let prompt = recorded_prompt.lock().unwrap().clone().unwrap();
    assert!(
        prompt.contains("end-of-session digest"),
        "expected DIGEST prompt, got: {prompt}"
    );
}

#[tokio::test]
async fn session_end_task_done_uses_retro_prompt() {
    let canned =
        r#"{"updates":[{"note_path":"lesson/x","action":"create","new_facts":["f"],"links":[]}]}"#;
    let (svc, recorded_prompt, raw_store, indexer, _temp_dir) =
        build_service_with_recording_provider(canned).await;

    let raw = RawMemory::new(
        "TASK_OUTCOME: shipped feature Y\nKEY_LEARNINGS:\n- L1".into(),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        },
    )
    .with_agent("agent-4");
    raw_store.insert_raw_memory(&raw).await.unwrap();

    let result = svc.compress_to_notes("agent-4", &indexer).await.unwrap();
    assert_eq!(result.memories_processed, 1);

    let prompt = recorded_prompt.lock().unwrap().clone().unwrap();
    assert!(
        prompt.contains("memory retrospective assistant"),
        "expected RETRO prompt, got: {prompt}"
    );
}
