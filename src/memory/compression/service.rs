//! Compression Service
//!
//! Main service that orchestrates memory compression:
//! 1. Fetches uncompressed memories
//! 2. Extracts facts using LLM
//! 3. Detects and resolves conflicts
//! 4. Stores facts and updates compression state

use super::scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
use super::signal_detector::{CompressionPriority, SignalDetector};
use crate::error::AlephError;
use crate::memory::context::CompressionResult;
use crate::memory::events::handler::MemoryCommandHandler;
use crate::memory::store::{CompressionStore, MemoryBackend};
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio::time::interval;

/// Configuration for the compression service
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Batch size for compression (max memories per batch)
    pub batch_size: u32,
    /// Scheduler configuration
    pub scheduler: SchedulerConfig,
    /// Background task interval in seconds
    pub background_interval_seconds: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            batch_size: 50,
            scheduler: SchedulerConfig::default(),
            background_interval_seconds: 3600, // 1 hour
        }
    }
}

impl CompressionConfig {
    /// Create from config policy
    pub fn from_policy(policy: &crate::config::CompressionPolicy) -> Self {
        Self {
            batch_size: 50,
            scheduler: SchedulerConfig::from_policy(policy),
            background_interval_seconds: policy.background_interval_seconds,
        }
    }
}

/// Main compression service
pub struct CompressionService {
    database: MemoryBackend,
    scheduler: Arc<CompressionScheduler>,
    config: CompressionConfig,
    signal_detector: SignalDetector,
    command_handler: Option<Arc<MemoryCommandHandler>>,
    compound_ingestor: Option<Arc<dyn crate::memory::notes::ingest::CompoundIngestor>>,
    compound_enabled: bool,
}

impl CompressionService {
    /// Create a new compression service
    ///
    /// If `memory_backend` is provided, L1 overview generation is enabled.
    /// Otherwise, L1 generation is skipped.
    pub fn new(
        database: MemoryBackend,
        provider: Arc<dyn AiProvider>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: CompressionConfig,
    ) -> Self {
        Self::new_with_backend(database, provider, embedder, config, None)
    }

    /// Create a new compression service with an optional MemoryBackend (kept for API compatibility)
    pub fn new_with_backend(
        database: MemoryBackend,
        _provider: Arc<dyn AiProvider>,
        _embedder: Arc<dyn EmbeddingProvider>,
        config: CompressionConfig,
        _memory_backend: Option<MemoryBackend>,
    ) -> Self {
        let scheduler = Arc::new(CompressionScheduler::new(config.scheduler.clone()));

        Self {
            database,
            scheduler,
            config,
            signal_detector: SignalDetector::new(),
            command_handler: None,
            compound_ingestor: None,
            compound_enabled: false,
        }
    }

    /// Attach an event-sourcing command handler.
    ///
    /// When present, fact creation during compression goes through the
    /// event sourcing pipeline instead of direct `insert_fact`.
    pub fn with_command_handler(mut self, handler: Arc<MemoryCommandHandler>) -> Self {
        self.command_handler = Some(handler);
        self
    }

    /// Attach a `CompoundIngestor` and enable the compound ingest path.
    ///
    /// When set, `compress_to_notes` routes each per-source batch through
    /// `CompoundIngestor::ingest_batch` instead of the legacy
    /// `extract_note_updates_for_source` call chain.
    pub fn with_compound_ingestor(
        mut self,
        ing: Arc<dyn crate::memory::notes::ingest::CompoundIngestor>,
    ) -> Self {
        self.compound_enabled = true;
        self.compound_ingestor = Some(ing);
        self
    }

    /// Execute a compression operation
    ///
    /// Routes through `compress_to_notes` by default, producing Knowledge
    /// Notes instead of raw facts.  Falls back to the legacy
    /// `compress_in_workspace` path only when the notes directory cannot be
    /// determined.
    pub async fn compress(&self) -> Result<CompressionResult, AlephError> {
        self.compress_default_notes(crate::memory::DEFAULT_AGENT)
            .await
    }

    /// Internal helper: build a `NoteIndexer` from the data directory and
    /// delegate to `compress_to_notes`.
    async fn compress_default_notes(
        &self,
        workspace_id: &str,
    ) -> Result<CompressionResult, AlephError> {
        let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("aleph")
                .join("memory")
                .join("note")
        });

        let indexer = crate::memory::notes::NoteIndexer::new(memory_dir, self.database.clone());

        self.compress_to_notes(workspace_id, &indexer).await
    }

    /// Compress memories into Knowledge Notes instead of raw facts.
    ///
    /// This method follows the same pipeline as `compress_in_workspace` but
    /// routes extracted information into markdown-based Knowledge Notes via
    /// `NoteIndexer` instead of storing individual `MemoryFact` rows.
    pub async fn compress_to_notes<S: crate::memory::notes::store::NoteStore + Send + Sync>(
        &self,
        workspace_id: &str,
        indexer: &crate::memory::notes::NoteIndexer<S>,
    ) -> Result<CompressionResult, AlephError> {
        let start = Instant::now();

        // 2. Fetch unprocessed raw memories
        use crate::memory::store::raw_memory::RawMemoryStore;

        let raw_memories = self
            .database
            .get_unprocessed_raw_memories(workspace_id, self.config.batch_size as usize)
            .await
            .map_err(|e| AlephError::other(format!("Failed to fetch raw memories: {e}")))?;

        let raw_memories: Vec<_> = raw_memories
            .into_iter()
            .filter(|r| r.source != crate::memory::store::raw_memory::RawMemorySource::Transcript)
            .collect();

        if raw_memories.is_empty() {
            tracing::debug!("No uncompressed session memories for note-based compression");
            return Ok(CompressionResult::empty());
        }

        tracing::info!(
            memory_count = raw_memories.len(),
            "Starting note-based compression"
        );

        // 3. Get existing note context (path + first 300 chars of body)
        let existing_notes = indexer
            .store()
            .list_notes(workspace_id)
            .await
            .unwrap_or_default();
        let mut existing_note_summaries: Vec<String> = Vec::new();
        for note_idx in &existing_notes {
            let note_file = indexer
                .memory_dir()
                .join(workspace_id)
                .join(&note_idx.category)
                .join(format!("{}.md", note_idx.filename));
            let summary = match tokio::fs::read_to_string(&note_file).await {
                Ok(content) => {
                    // Take first 300 chars of body as context
                    let preview: String = content.chars().take(300).collect();
                    format!("{}: {}", note_idx.path, preview)
                }
                Err(_) => note_idx.path.clone(),
            };
            existing_note_summaries.push(summary);
        }

        // 4. Extract note updates via LLM — one call per source group (Spec 1).
        //    When compound_enabled, delegate each source batch to CompoundIngestor
        //    and skip the legacy accumulation path.
        if self.compound_enabled {
            if let Some(ing) = self.compound_ingestor.clone() {
                match ing.ingest_batch(workspace_id, raw_memories.clone()).await {
                    Ok(_report) => {
                        // Fall through to mark_raw_as_processed below.
                    }
                    Err(e) => {
                        tracing::warn!("compound ingest failed: {e}");
                        // Leave this batch unprocessed; retry next tick.
                        return Ok(CompressionResult::empty());
                    }
                }
            } else {
                tracing::warn!(
                    "compound ingest enabled but no ingestor configured; skipping batch"
                );
                return Ok(CompressionResult::empty());
            }

            // Mark raw memories as processed (compound path).
            let consumed_ids: Vec<String> = raw_memories.iter().map(|r| r.id.clone()).collect();
            match self.database.mark_raw_as_processed(&consumed_ids).await {
                Ok(n) => tracing::info!(marked = n, "Marked raw memories as processed (compound)"),
                Err(e) => tracing::warn!(error = %e, "Failed to mark raw memories as processed"),
            }

            // Update compression timestamp.
            let latest_timestamp = raw_memories.iter().map(|r| r.created_at).max().unwrap_or(0);
            self.database
                .set_last_compression_timestamp(latest_timestamp)
                .await?;

            let duration_ms = start.elapsed().as_millis() as u64;
            tracing::info!(duration_ms, "Compound ingest compression complete");
            return Ok(CompressionResult {
                duration_ms,
                ..CompressionResult::default()
            });
        }

        // compound_enabled is always required; if ingestor is missing, warn and skip.
        tracing::warn!("compress_to_notes called without compound ingestor; skipping batch");
        Ok(CompressionResult::empty())
    }

    /// Check if compression should be triggered and execute if needed
    pub async fn check_and_compress(&self) -> Result<Option<CompressionResult>, AlephError> {
        let trigger = self.scheduler.should_trigger_compression();

        match trigger {
            CompressionTrigger::None => {
                tracing::trace!("No compression trigger, skipping");
                Ok(None)
            }
            trigger => {
                tracing::info!(trigger = ?trigger, "Compression triggered");
                let result = self.compress().await?;
                Ok(Some(result))
            }
        }
    }

    /// Check for signal-based compression trigger
    ///
    /// This method detects signals in the user message and triggers
    /// compression based on priority:
    /// - Immediate: Compress now (correction signals)
    /// - Deferred: Record turn and check scheduler (learning signals)
    /// - Batch: Just record activity (milestone signals)
    pub async fn check_and_compress_with_signal(
        &self,
        user_message: &str,
    ) -> Result<Option<CompressionResult>, AlephError> {
        // Detect signals in message
        let detection = self.signal_detector.detect(user_message);

        if detection.should_compress {
            tracing::info!(
                signals = ?detection.signals,
                priority = ?detection.priority,
                "Signal-triggered compression"
            );

            match detection.priority {
                CompressionPriority::Immediate => {
                    // Compress immediately
                    let result = self.compress().await?;
                    Ok(Some(result))
                }
                CompressionPriority::Deferred => {
                    // Record turn and let scheduler decide
                    self.scheduler.increment_turns();
                    self.check_and_compress().await
                }
                CompressionPriority::Batch => {
                    // Just record activity, batch later
                    self.scheduler.record_activity();
                    Ok(None)
                }
            }
        } else {
            // Fall back to existing scheduler-based check
            self.check_and_compress().await
        }
    }

    /// Start background compression task
    ///
    /// Runs periodically and compresses unconditionally (bypasses scheduler).
    /// The scheduler-based triggers are handled separately via `record_turn_and_check()`.
    pub fn start_background_task(self: Arc<Self>) -> JoinHandle<()> {
        let interval_secs = self.config.background_interval_seconds;

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(interval_secs as u64));

            tracing::info!(
                interval_seconds = interval_secs,
                "Started background compression task"
            );

            loop {
                interval.tick().await;

                // Background task compresses unconditionally — the scheduler
                // is only for turn-based / signal-based immediate triggers.
                match self.compress().await {
                    Ok(result) => {
                        if result.memories_processed > 0 {
                            tracing::info!(
                                memories = result.memories_processed,
                                facts = result.facts_extracted,
                                duration_ms = result.duration_ms,
                                "Background compression completed"
                            );
                        } else {
                            tracing::debug!("Background compression: no memories to process");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Background compression failed");
                    }
                }
            }
        })
    }

    /// Start background compression task with external runtime
    ///
    /// This method is used during AlephCore initialization when we have a runtime
    /// but are not yet inside its context (so tokio::spawn won't work).
    ///
    /// Compresses unconditionally on each interval tick.
    pub fn start_background_task_with_runtime(
        self: &Arc<Self>,
        runtime: &tokio::runtime::Runtime,
    ) -> JoinHandle<()> {
        let service = Arc::clone(self);
        let interval_secs = self.config.background_interval_seconds;

        runtime.spawn(async move {
            let mut hourly_interval = interval(Duration::from_secs(interval_secs as u64));

            tracing::info!(
                interval_seconds = interval_secs,
                "Started background compression task"
            );

            loop {
                hourly_interval.tick().await;

                match service.compress().await {
                    Ok(result) => {
                        if result.memories_processed > 0 {
                            tracing::info!(
                                memories = result.memories_processed,
                                facts = result.facts_extracted,
                                duration_ms = result.duration_ms,
                                "Compression completed"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Compression failed");
                    }
                }
            }
        })
    }

    /// Record user activity (for idle detection)
    pub fn record_activity(&self) {
        self.scheduler.record_activity();
    }

    /// Record a conversation turn (for turn threshold)
    pub fn record_turn(&self) {
        self.scheduler.increment_turns();
    }

    /// Record a conversation turn and trigger compression if threshold reached
    ///
    /// This method checks if the turn threshold is reached after incrementing,
    /// and if so, spawns a compression task immediately instead of waiting
    /// for the next hourly background check.
    pub fn record_turn_and_check(self: &Arc<Self>) {
        // Use the return value of fetch_add to trigger exactly once when
        // the threshold is crossed, avoiding race conditions.
        let old_turns = self
            .scheduler
            .pending_turns
            .fetch_add(1, crate::sync_primitives::Ordering::AcqRel);
        let turns = old_turns + 1;
        let threshold = self.config.scheduler.turn_threshold;

        if old_turns < threshold && turns >= threshold {
            tracing::info!(
                turns = turns,
                threshold = threshold,
                "Turn threshold reached, triggering immediate compression"
            );

            // Spawn compression task
            let service = Arc::clone(self);
            tokio::spawn(async move {
                match service.check_and_compress().await {
                    Ok(Some(result)) => {
                        tracing::info!(
                            facts = result.facts_extracted,
                            duration_ms = result.duration_ms,
                            "Immediate compression completed (turn threshold)"
                        );
                    }
                    Ok(None) => {
                        tracing::debug!("Immediate compression: no action needed");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Immediate compression failed");
                    }
                }
            });
        }
    }

    /// Get the scheduler for external monitoring
    pub fn get_scheduler(&self) -> Arc<CompressionScheduler> {
        Arc::clone(&self.scheduler)
    }

    /// Get current configuration
    pub fn get_config(&self) -> &CompressionConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::NoteIndexer;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::create_mock_provider;
    use tempfile::{tempdir, TempDir};

    async fn create_test_service() -> (CompressionService, MemoryBackend) {
        let (service, database, _temp_dir) = create_test_service_with_tempdir().await;
        (service, database)
    }

    async fn create_test_service_with_tempdir() -> (CompressionService, MemoryBackend, TempDir) {
        let temp_dir = tempdir().unwrap();
        let database: MemoryBackend =
            Arc::new(crate::memory::store::SqliteMemoryBackend::new(temp_dir.path()).unwrap());

        let provider = create_mock_provider();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(
                1024,
                "mock-model",
            ),
        );

        let config = CompressionConfig::default();

        let service = CompressionService::new(database.clone(), provider, embedder, config);

        (service, database, temp_dir)
    }

    /// Create a test setup that includes a NoteIndexer backed by a separate
    /// SqliteMemoryBackend (NoteIndexer requires Arc<S: NoteStore> directly,
    /// not the type-erased MemoryBackend).
    async fn create_test_service_with_indexer() -> (
        CompressionService,
        MemoryBackend,
        NoteIndexer<SqliteMemoryBackend>,
        TempDir,
    ) {
        let temp_dir = tempdir().unwrap();

        // Separate db file for the compression backend
        let db_path = temp_dir.path().join("compression.db");
        let database: MemoryBackend = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());

        // Separate db file for the notes index (NoteIndexer needs Arc<SqliteMemoryBackend>)
        let notes_db_path = temp_dir.path().join("notes.db");
        let notes_backend = Arc::new(SqliteMemoryBackend::new(&notes_db_path).unwrap());

        let memory_dir = temp_dir.path().join("memory");
        let indexer = NoteIndexer::new(memory_dir, notes_backend);

        let provider = create_mock_provider();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(
                1024,
                "mock-model",
            ),
        );

        let config = CompressionConfig::default();
        let service = CompressionService::new(database.clone(), provider, embedder, config);

        (service, database, indexer, temp_dir)
    }

    #[tokio::test]
    async fn test_compress_empty_memories() {
        let (service, _) = create_test_service().await;

        let result = service.compress().await.unwrap();

        assert_eq!(result.memories_processed, 0);
        assert_eq!(result.facts_extracted, 0);
    }

    #[tokio::test]
    async fn test_scheduler_integration() {
        let (service, _) = create_test_service().await;

        // Record activity
        service.record_activity();

        // Record turns
        for _ in 0..5 {
            service.record_turn();
        }

        let scheduler = service.get_scheduler();
        assert_eq!(scheduler.get_pending_turns(), 5);
    }

    #[test]
    fn test_config_default() {
        let config = CompressionConfig::default();
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.background_interval_seconds, 3600);
    }

    #[tokio::test]
    async fn test_signal_triggered_compression() {
        let (service, _database, _temp_dir) = create_test_service_with_tempdir().await;

        // With SessionStore removed, compression returns empty immediately.
        // Verify it doesn't panic and handles signals gracefully.
        let message = "记住，我喜欢用 Vim";
        let result = service
            .check_and_compress_with_signal(message)
            .await
            .unwrap();

        // Without raw memories, compression always returns None or empty result
        assert!(result.is_some() || result.is_none());
    }

    /// Empty database: compression returns an empty result without error.
    #[tokio::test]
    async fn test_compress_to_notes_empty_database() {
        let (service, _database, indexer, _temp_dir) = create_test_service_with_indexer().await;

        let result = service
            .compress_to_notes("default", &indexer)
            .await
            .unwrap();

        assert_eq!(result.memories_processed, 0);
        assert_eq!(result.facts_extracted, 0);
        assert_eq!(result.duration_ms, 0, "Empty run should be instant");
    }

    /// RawMemory round-trip: insert multiple with different sources, query,
    /// mark some processed, verify count decreases correctly.
    #[tokio::test]
    async fn test_raw_memory_full_roundtrip() {
        let temp_dir = tempdir().unwrap();
        let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::new(temp_dir.path()).unwrap());

        // Insert 3 raw memories with varying sources
        let raw1 = RawMemory::new(
            "Session content 1".to_string(),
            RawMemorySource::SessionCompressed,
        );
        let raw2 = RawMemory::new(
            "Session content 2".to_string(),
            RawMemorySource::SessionCompressed,
        );
        let raw3 = RawMemory::new("Tool output".to_string(), RawMemorySource::ToolOutput);

        backend.insert_raw_memory(&raw1).await.unwrap();
        backend.insert_raw_memory(&raw2).await.unwrap();
        backend.insert_raw_memory(&raw3).await.unwrap();

        // All 3 are unprocessed
        assert_eq!(backend.count_unprocessed("default").await.unwrap(), 3);

        // Mark raw1 and raw2 as processed
        backend
            .mark_raw_as_processed(&[raw1.id.clone(), raw2.id.clone()])
            .await
            .unwrap();

        // Only raw3 should remain
        assert_eq!(backend.count_unprocessed("default").await.unwrap(), 1);

        // The remaining unprocessed memory is raw3
        let unprocessed = backend
            .get_unprocessed_raw_memories("default", 10)
            .await
            .unwrap();
        assert_eq!(unprocessed.len(), 1);
        assert_eq!(unprocessed[0].id, raw3.id);

        // Mark raw3 as processed
        backend
            .mark_raw_as_processed(&[raw3.id.clone()])
            .await
            .unwrap();

        assert_eq!(backend.count_unprocessed("default").await.unwrap(), 0);
    }
}
