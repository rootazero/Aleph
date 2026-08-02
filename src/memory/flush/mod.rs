//! Real-time memory flush (Pillar 2 of the Real-time Memory spec).
//!
//! Drain an agent's pending raw memories into linked Knowledge Notes via
//! [`CompressionService::compress_to_notes`] (which keyword-links the new notes).
//! Triggered on session conclude, and by `flag_user_correction` when the model
//! judges that the user corrected it. The flush registers itself in a
//! [`FlushRegistry`] so a back-to-back follow-on session can `await_ready`
//! (bounded) and recall consolidated memory, while a normal session never waits.

pub mod registry;
pub use registry::{FlushGuard, FlushRegistry};

use std::sync::OnceLock;
use tracing::warn;

use crate::memory::compression::CompressionService;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::sync_primitives::Arc;

/// Process-global flush registry (one per daemon). Mirrors the `OnceCell`
/// singleton idiom used by `goal::global()` and the session-end accessors in
/// `thinker::memory_context_provider` — shared by the session-end spawn site and
/// any follow-on `await_ready` caller.
pub fn global_registry() -> FlushRegistry {
    static REG: OnceLock<FlushRegistry> = OnceLock::new();
    REG.get_or_init(FlushRegistry::new).clone()
}

/// Run an immediate compress→link flush for `agent`, holding `guard` (acquired
/// from [`FlushRegistry::begin`]) for the flush duration so a follow-on session
/// can await readiness.
///
/// Two call sites, both fire-and-forget:
/// - **session end** (`gateway::session_manager::ops::emit`) — the original Pillar 2
///   trigger, after the SessionEnd digest row is committed.
/// - **`flag_user_correction`** — the model judged that the user corrected it, so
///   sediment the lesson now instead of waiting for the next dream cycle.
///
/// The guard MUST be acquired synchronously at each call site — *before* this
/// future is spawned — so the registry entry is observable the instant the flush
/// is decided on. Acquiring it inside the spawned task (the previous shape) raced
/// a back-to-back `await_ready`: `tokio::spawn` returns before the task is polled,
/// so the waiter could observe an empty registry and silently no-op the readiness
/// gate. Failures are logged, never propagated — a flush is best-effort
/// consolidation, never gating.
pub async fn flush_agent_memory(
    guard: FlushGuard,
    agent: String,
    compression: Arc<CompressionService>,
) {
    let _guard = guard;

    // Drain, don't sip. One `compress_to_notes` call consumes at most
    // `batch_size` (50) rows and the fetch is `ORDER BY created_at ASC` — the
    // 50 OLDEST. The session-end caller commits the SessionEnd digest and *then*
    // flushes precisely so the digest is consumed; but the digest is the NEWEST
    // row, so on any session that accumulated more than a batch of raw rows
    // (tool-heavy sessions produce several per turn) a single call never
    // reaches it. Loop while the pending count STRICTLY decreases: the
    // stop-the-bleed grace window in `compress_to_notes` deliberately leaves
    // young rows unprocessed, so `while count > 0` would spin forever.
    const MAX_DRAIN_ROUNDS: usize = 8;
    let mut prev = usize::MAX;
    for _ in 0..MAX_DRAIN_ROUNDS {
        if let Err(e) = compression.compress_to_notes(&agent).await {
            warn!(agent = %agent, error = %e, "flush_agent_memory: compress_to_notes failed");
            break;
        }
        let Ok(remaining) = compression.store().count_unprocessed(&agent).await else {
            break;
        };
        if remaining == 0 || remaining >= prev {
            break;
        }
        prev = remaining;
    }
    // `_guard` drops here → wakes any `await_ready` waiter, once every concurrent
    // flush for this agent has also finished.
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

    /// `flush_agent_memory` must drive `compress_to_notes` to completion for the
    /// agent — draining its pending raw memory into the note pipeline — without
    /// any turn-threshold or dream-cycle gate.
    #[tokio::test]
    async fn flush_agent_memory_compresses_pending_raw_into_a_note() {
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

        let reg = FlushRegistry::new();
        let guard = reg.begin("default");
        flush_agent_memory(guard, "default".into(), Arc::new(service)).await;

        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            0,
            "flush_agent_memory must drain the agent's pending raw memory via compress_to_notes"
        );
    }

    /// One `compress_to_notes` call consumes at most `batch_size` (50) rows and
    /// takes the OLDEST first, so a single call cannot reach the SessionEnd
    /// digest that the session-end caller deliberately commits *before*
    /// flushing. With a backlog above one batch, a sipping flush left the
    /// freshest and highest-value row unprocessed — exactly the row the
    /// ordering exists to capture.
    #[tokio::test]
    async fn flush_agent_memory_drains_a_backlog_larger_than_one_batch() {
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

        // 130 rows = 3 batches of 50. All aged past the grace window so each
        // drain round genuinely consumes them.
        let aged = chrono::Utc::now().timestamp() - 7 * 3600;
        for i in 0..130 {
            let mut raw =
                RawMemory::new(format!("tool call {i} output"), RawMemorySource::Transcript);
            raw.created_at = aged + i;
            database.insert_raw_memory(&raw).await.unwrap();
        }
        assert_eq!(database.count_unprocessed("default").await.unwrap(), 130);

        let reg = FlushRegistry::new();
        let guard = reg.begin("default");
        flush_agent_memory(guard, "default".into(), Arc::new(service)).await;

        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            0,
            "a single flush must drain the whole backlog, not just the first batch"
        );
    }
}
