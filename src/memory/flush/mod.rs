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

    for partition in flush_partitions(&compression, &agent).await {
        drain_partition(&compression, &partition).await;
    }
    // `_guard` drops here → wakes any `await_ready` waiter, once every concurrent
    // flush for this agent has also finished.
}

/// Every raw-memory partition this agent's flush is responsible for: the base
/// id, plus each composed sibling (`{base}__u-…` / `{base}__p-…` /
/// `{base}__proj-…`) that currently holds unprocessed rows.
///
/// One session writes into more than one partition. Turn-level raw memory goes
/// through `session_write_id` (project-scoped / personal-scoped when either axis
/// is active) while the SessionEnd digest is filed under the bare agent id, and
/// the flush only ever drained the id it was handed. So with project or personal
/// scoping on, "real-time flush" was real time for the digest and up to an hour
/// (the background tick, which iterates `unprocessed_agent_ids` and therefore
/// did cover them) for everything the session actually said — while
/// `await_ready`, keyed on the base id, told the next session the consolidation
/// it was waiting for had finished.
///
/// Matched on the `{base}__` prefix, never a bare `starts_with(base)`: agents
/// `main` and `mainframe` are unrelated corpora.
///
/// A failure to enumerate degrades to the base partition alone — the behaviour
/// before this existed — rather than skipping the flush.
async fn flush_partitions(compression: &Arc<CompressionService>, agent: &str) -> Vec<String> {
    let mut partitions = vec![agent.to_string()];
    let prefix = format!("{agent}{}", crate::memory::project_scope::NS_SEP);
    match compression.store().unprocessed_agent_ids().await {
        Ok(ids) => {
            partitions.extend(ids.into_iter().filter(|id| id.starts_with(&prefix)));
        }
        Err(e) => {
            warn!(agent = %agent, error = %e, "flush_agent_memory: partition enumeration failed; draining the base partition only");
        }
    }
    partitions
}

/// Drain, don't sip. One `compress_to_notes` call consumes at most `batch_size`
/// (50) rows and the fetch is `ORDER BY created_at ASC` — the 50 OLDEST. The
/// session-end caller commits the SessionEnd digest and *then* flushes precisely
/// so the digest is consumed; but the digest is the NEWEST row, so on any
/// session that accumulated more than a batch of raw rows (tool-heavy sessions
/// produce several per turn) a single call never reaches it. Loop while the
/// pending count STRICTLY decreases: the stop-the-bleed grace window in
/// `compress_to_notes` deliberately leaves young rows unprocessed, so
/// `while count > 0` would spin forever.
async fn drain_partition(compression: &Arc<CompressionService>, agent: &str) {
    const MAX_DRAIN_ROUNDS: usize = 8;
    let mut prev = usize::MAX;
    for _ in 0..MAX_DRAIN_ROUNDS {
        if let Err(e) = compression.compress_to_notes(agent).await {
            warn!(agent = %agent, error = %e, "flush_agent_memory: compress_to_notes failed");
            break;
        }
        let Ok(remaining) = compression.store().count_unprocessed(agent).await else {
            break;
        };
        if remaining == 0 || remaining >= prev {
            break;
        }
        prev = remaining;
    }
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

    /// One session writes into more than one partition: turn-level raw memory
    /// goes through `session_write_id` (project- or personal-scoped when either
    /// axis is on) while the SessionEnd digest is filed under the bare agent id.
    /// The flush drained only the id it was handed, so with scoping enabled the
    /// "real-time" pillar was real time for the digest and up to an hour (the
    /// background tick) for everything the session actually said — while
    /// `await_ready`, keyed on the base id, told the next session the wait was
    /// over.
    #[tokio::test]
    async fn flush_agent_memory_drains_the_agents_scoped_partitions_too() {
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

        let aged = chrono::Utc::now().timestamp() - 7 * 3600;
        // Base partition (the SessionEnd digest's home), the session's personal
        // scope, a project scope — and an unrelated agent whose id merely starts
        // with the same characters, which must NOT be drained by this flush.
        for (agent, body) in [
            ("default", "digest"),
            ("default__u-owner", "what the session said"),
            ("default__proj-abc123", "project-scoped turn"),
            ("defaultish", "a different agent entirely"),
        ] {
            let mut raw = RawMemory::new(body.to_string(), RawMemorySource::Transcript)
                .with_agent(agent.to_string());
            raw.created_at = aged;
            database.insert_raw_memory(&raw).await.unwrap();
        }

        let reg = FlushRegistry::new();
        let guard = reg.begin("default");
        flush_agent_memory(guard, "default".into(), Arc::new(service)).await;

        for agent in ["default", "default__u-owner", "default__proj-abc123"] {
            assert_eq!(
                database.count_unprocessed(agent).await.unwrap(),
                0,
                "{agent} is part of this agent's memory and must be drained by its flush"
            );
        }
        assert_eq!(
            database.count_unprocessed("defaultish").await.unwrap(),
            1,
            "`defaultish` is an unrelated corpus, not a scoped partition of `default`"
        );
    }
}
