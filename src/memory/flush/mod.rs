//! Real-time memory flush (Pillar 2 of the Real-time Memory spec).
//!
//! On session conclude, the gateway spawns an immediate, non-blocking flush:
//! drain the agent's pending raw memories into linked Knowledge Notes via
//! [`CompressionService::compress_to_notes`] (which keyword-links the new notes,
//! Task 5). The flush registers itself in a [`FlushRegistry`] so a back-to-back
//! follow-on session can `await_ready` (bounded) and recall consolidated memory,
//! while a normal session never waits.

pub mod registry;
pub use registry::{FlushGuard, FlushRegistry};

use std::sync::OnceLock;
use tracing::warn;

use crate::memory::compression::CompressionService;
use crate::sync_primitives::Arc;

/// Process-global flush registry (one per daemon). Mirrors the `OnceCell`
/// singleton idiom used by `goal::global()` and the session-end accessors in
/// `thinker::memory_context_provider` — shared by the session-end spawn site and
/// any follow-on `await_ready` caller.
pub fn global_registry() -> FlushRegistry {
    static REG: OnceLock<FlushRegistry> = OnceLock::new();
    REG.get_or_init(FlushRegistry::new).clone()
}

/// Run an immediate compress→link flush for `agent`, guarded in `reg` so a
/// follow-on session can await readiness. Intended to be `tokio::spawn`ed at
/// session conclude (async, non-blocking). Failures are logged, never
/// propagated — a flush is best-effort consolidation, never gating.
pub async fn session_end_flush(
    reg: FlushRegistry,
    agent: String,
    compression: Arc<CompressionService>,
) {
    let _guard = reg.begin(&agent);
    if let Err(e) = compression.compress_to_notes(&agent).await {
        warn!(agent = %agent, error = %e, "session_end_flush: compress_to_notes failed");
    }
    // `_guard` drops here → wakes any `await_ready` waiter for this agent.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::compression::{CompressionConfig, CompressionService};
    use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::memory::EmbeddingProvider;
    use crate::providers::create_mock_provider;

    /// Minimal ingestor: lets `compress_to_notes` run its real pipeline (so the
    /// pending queue is actually drained) without standing up a full
    /// `NoteIndexer`. Mirrors the `EmptyIngestor` used by the compression
    /// service's own `compress_to_notes_marks_aged_rows_when_plan_empty` test.
    struct EmptyIngestor;
    #[async_trait::async_trait]
    impl CompoundIngestor for EmptyIngestor {
        async fn ingest_batch(
            &self,
            _agent_id: &str,
            _raws: Vec<RawMemory>,
            _extra_context: Option<&str>,
        ) -> Result<ApplyReport, crate::error::AlephError> {
            Ok(ApplyReport::default())
        }
    }

    /// `session_end_flush` must drive `compress_to_notes` to completion for the
    /// agent — draining its pending raw memory into the note pipeline — without
    /// any turn-threshold or dream-cycle gate.
    #[tokio::test]
    async fn session_end_flush_compresses_pending_raw_into_a_note() {
        let temp_dir = tempfile::tempdir().unwrap();
        let database: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(temp_dir.path()).unwrap());

        let provider = create_mock_provider();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(
                1024,
                "mock-model",
            ),
        );

        let service = CompressionService::new(
            database.clone(),
            provider,
            embedder,
            CompressionConfig::default(),
        )
        .with_compound_ingestor(Arc::new(EmptyIngestor));

        // Seed ONE aged pending raw so the stop-the-bleed grace window gives up
        // and the row is marked processed (drained) by this single flush.
        let mut raw = RawMemory::new(
            "user prefers dark mode".to_string(),
            RawMemorySource::Transcript,
        );
        raw.created_at = chrono::Utc::now().timestamp() - 7 * 3600;
        database.insert_raw_memory(&raw).await.unwrap();

        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            1,
            "precondition: one pending raw memory"
        );

        session_end_flush(FlushRegistry::new(), "default".into(), Arc::new(service)).await;

        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            0,
            "session_end_flush must drain the agent's pending raw memory via compress_to_notes"
        );
    }
}
