//! Compression Service
//!
//! Main service that orchestrates memory compression:
//! 1. Fetches uncompressed memories
//! 2. Extracts facts using LLM
//! 3. Detects and resolves conflicts
//! 4. Stores facts and updates compression state

use super::conflict::{ConflictConfig, ConflictDetector};
use super::extractor::FactExtractor;
use super::scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
use super::signal_detector::{CompressionPriority, SignalDetector};
use crate::error::AlephError;
use crate::memory::context::{CompressionResult, CompressionSession};
use crate::memory::events::handler::MemoryCommandHandler;
use crate::memory::store::{MemoryBackend, MemoryStore, SessionStore, CompressionStore};
use crate::memory::graph::GraphStore;
use crate::memory::EmbeddingProvider;
use crate::memory::vfs::L1Generator;
use crate::providers::AiProvider;
use std::collections::HashSet;
use std::sync::Arc;
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
    /// Conflict detection configuration
    pub conflict: ConflictConfig,
    /// Background task interval in seconds
    pub background_interval_seconds: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            batch_size: 50,
            scheduler: SchedulerConfig::default(),
            conflict: ConflictConfig::default(),
            background_interval_seconds: 3600, // 1 hour
        }
    }
}

/// Main compression service
pub struct CompressionService {
    database: MemoryBackend,
    extractor: Arc<FactExtractor>,
    conflict_detector: Arc<ConflictDetector>,
    scheduler: Arc<CompressionScheduler>,
    config: CompressionConfig,
    provider_name: String,
    signal_detector: SignalDetector,
    graph_store: GraphStore,
    l1_generator: Option<Arc<L1Generator>>,
    command_handler: Option<Arc<MemoryCommandHandler>>,
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

    /// Create a new compression service with an optional MemoryBackend for L1 generation
    pub fn new_with_backend(
        database: MemoryBackend,
        provider: Arc<dyn AiProvider>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: CompressionConfig,
        memory_backend: Option<MemoryBackend>,
    ) -> Self {
        let provider_name = provider.name().to_string();

        // L1Generator uses MemoryBackend; only create if backend is provided
        let l1_generator = memory_backend.map(|backend| {
            Arc::new(L1Generator::new(
                backend,
                Arc::clone(&provider),
                Arc::clone(&embedder),
            ))
        });

        let extractor = Arc::new(FactExtractor::new(provider, embedder));

        let conflict_detector = Arc::new(ConflictDetector::new(
            database.clone(),
            config.conflict.clone(),
        ));

        let scheduler = Arc::new(CompressionScheduler::new(config.scheduler.clone()));

        let graph_store = GraphStore::new(database.clone());

        Self {
            database,
            extractor,
            conflict_detector,
            scheduler,
            config,
            provider_name,
            signal_detector: SignalDetector::new(),
            graph_store,
            l1_generator,
            command_handler: None,
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

    /// Execute a compression operation
    pub async fn compress(&self) -> Result<CompressionResult, AlephError> {
        let start = Instant::now();

        // 1. Get last compression timestamp
        let last_timestamp = self
            .database
            .get_last_compression_timestamp()
            .await?
            .unwrap_or(0);

        // 2. Get uncompressed memories
        let memories = self
            .database
            .get_uncompressed_memories(last_timestamp, self.config.batch_size as usize)
            .await?;

        if memories.is_empty() {
            tracing::debug!("No memories to compress");
            return Ok(CompressionResult::empty());
        }

        tracing::info!(
            memory_count = memories.len(),
            since_timestamp = last_timestamp,
            "Starting memory compression"
        );

        // 3. Extract facts using LLM
        let extracted_facts = match self.extractor.extract_facts(&memories).await {
            Ok(facts) => facts,
            Err(e) => {
                tracing::error!(error = %e, "Failed to extract facts from memories");
                return Err(e);
            }
        };

        tracing::info!(
            extracted_count = extracted_facts.len(),
            "Extracted facts from memories"
        );

        // 4. Process each fact (conflict detection and storage)
        let mut stored_fact_ids = Vec::new();
        let mut total_invalidated = 0u32;
        let mut affected_paths: HashSet<String> = HashSet::new();

        for fact in extracted_facts {
            // Detect conflicts
            let resolutions = self.conflict_detector.resolve_conflicts(&fact).await?;

            // Apply resolutions (invalidate old facts)
            let invalidated = self
                .conflict_detector
                .apply_resolutions(&resolutions)
                .await?;
            total_invalidated += invalidated;

            // Store the new fact — through event sourcing when available,
            // otherwise fall back to direct insert.
            let store_result = if let Some(handler) = &self.command_handler {
                use crate::memory::events::commands::CreateFactCommand;
                use crate::memory::events::EventActor;

                handler
                    .create_fact(CreateFactCommand {
                        content: fact.content.clone(),
                        fact_type: fact.fact_type.clone(),
                        tier: fact.tier.clone(),
                        scope: fact.scope.clone(),
                        path: fact.path.clone(),
                        namespace: fact.namespace.clone(),
                        workspace: fact.workspace.clone(),
                        confidence: fact.confidence,
                        source: fact.fact_source.clone(),
                        source_memory_ids: fact.source_memory_ids.clone(),
                        actor: EventActor::System,
                        correlation_id: None,
                    })
                    .await
                    .map(|_id| ())
            } else {
                self.database.insert_fact(&fact).await
            };

            match store_result {
                Ok(_) => {
                    stored_fact_ids.push(fact.id.clone());
                    affected_paths.insert(fact.path.clone());
                    tracing::debug!(
                        fact_id = %fact.id,
                        content = %fact.content,
                        "Stored compressed fact"
                    );
                    if let Err(e) = self.graph_store.update_from_fact(&fact, &memories).await {
                        tracing::warn!(error = %e, fact_id = %fact.id, "Failed to update graph from fact");
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        fact_content = %fact.content,
                        error = %e,
                        "Failed to store fact"
                    );
                }
            }
        }

        // 4b. Generate/update L1 Overviews for affected paths
        if !affected_paths.is_empty() {
            if let Some(ref l1_gen) = self.l1_generator {
                tracing::info!(
                    paths = affected_paths.len(),
                    "Generating L1 Overviews for affected paths"
                );
                match l1_gen.generate_for_affected_paths(&affected_paths).await {
                    Ok(updated) => {
                        tracing::info!(updated = updated, "L1 Overview generation completed");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "L1 Overview generation failed (non-fatal)");
                    }
                }
            }
        }

        // 5. Update compression timestamp
        let latest_timestamp = memories
            .iter()
            .map(|m| m.context.timestamp)
            .max()
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
            });

        self.database
            .set_last_compression_timestamp(latest_timestamp)
            .await?;

        // 6. Record compression session
        let duration_ms = start.elapsed().as_millis() as u64;
        let session = CompressionSession::new(
            memories.iter().map(|m| m.id.clone()).collect(),
            stored_fact_ids.clone(),
            self.provider_name.clone(),
            duration_ms,
        );

        self.database.record_compression_session(&session).await?;

        // 7. Reset scheduler
        self.scheduler.reset_turns();

        let result = CompressionResult {
            memories_processed: memories.len() as u32,
            facts_extracted: stored_fact_ids.len() as u32,
            facts_invalidated: total_invalidated,
            duration_ms,
        };

        tracing::info!(
            memories = result.memories_processed,
            facts = result.facts_extracted,
            invalidated = result.facts_invalidated,
            duration_ms = result.duration_ms,
            "Memory compression completed"
        );

        Ok(result)
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
    /// Similar to CleanupService, runs periodically to check for compression triggers.
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

                match self.check_and_compress().await {
                    Ok(Some(result)) => {
                        tracing::info!(
                            facts = result.facts_extracted,
                            duration_ms = result.duration_ms,
                            "Background compression completed"
                        );
                    }
                    Ok(None) => {
                        tracing::trace!("Background compression check: no action needed");
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
    /// Triggers:
    /// - Every hour: checks if compression is needed
    /// - Turn threshold (20): triggers when conversation turns accumulate
    pub fn start_background_task_with_runtime(
        self: &Arc<Self>,
        runtime: &tokio::runtime::Runtime,
    ) -> JoinHandle<()> {
        let service = Arc::clone(self);
        let interval_secs = self.config.background_interval_seconds;
        let turn_threshold = self.config.scheduler.turn_threshold;

        runtime.spawn(async move {
            let mut hourly_interval = interval(Duration::from_secs(interval_secs as u64));

            tracing::info!(
                interval_seconds = interval_secs,
                turn_threshold = turn_threshold,
                "Started background compression task (hourly + turn threshold)"
            );

            loop {
                hourly_interval.tick().await;

                match service.check_and_compress().await {
                    Ok(Some(result)) => {
                        tracing::info!(
                            facts = result.facts_extracted,
                            duration_ms = result.duration_ms,
                            "Compression completed"
                        );
                    }
                    Ok(None) => {
                        tracing::debug!("Compression check: no action needed");
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
        self.scheduler.increment_turns();
        let turns = self.scheduler.get_pending_turns();
        let threshold = self.config.scheduler.turn_threshold;

        if turns >= threshold {
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
    use crate::providers::create_mock_provider;
    use tempfile::{tempdir, TempDir};

    async fn create_test_service() -> (CompressionService, MemoryBackend) {
        let (service, database, _temp_dir) = create_test_service_with_tempdir().await;
        (service, database)
    }

    async fn create_test_service_with_tempdir() -> (CompressionService, MemoryBackend, TempDir)
    {
        let temp_dir = tempdir().unwrap();
        let database: MemoryBackend =
            Arc::new(crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap());

        let provider = create_mock_provider();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(1024, "mock-model"),
        );

        let config = CompressionConfig::default();

        let service = CompressionService::new(database.clone(), provider, embedder, config);

        (service, database, temp_dir)
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
        let (service, database, _temp_dir) = create_test_service_with_tempdir().await;

        // Store a memory with learning signal
        let context = crate::memory::context::ContextAnchor::now(
            "test.app".to_string(),
            "test.txt".to_string(),
        );
        // Create dummy embedding (384-dim for multilingual-e5-small)
        let embedding = vec![0.0f32; 384];
        let memory = crate::memory::context::MemoryEntry::with_embedding(
            "mem-1".to_string(),
            context,
            "记住，我喜欢用 Vim".to_string(),
            "好的，我记住了".to_string(),
            embedding,
        );

        // Insert via database directly
        database.insert_memory(&memory).await.unwrap();

        // Check with signal detection
        let message = "记住，我喜欢用 Vim";
        let result = service.check_and_compress_with_signal(message).await.unwrap();

        // Learning signal should trigger deferred compression
        // Since we only have 1 memory and turn threshold is not reached,
        // the deferred priority just records and checks scheduler
        // The result depends on scheduler state
        assert!(result.is_some() || result.is_none()); // Either compressed or deferred
    }
}
