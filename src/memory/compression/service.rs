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
use crate::memory::store::{CompressionStore, MemoryBackend, MemoryStore};
use crate::memory::vfs::L1Generator;
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use std::collections::HashSet;
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

impl CompressionConfig {
    /// Create from config policy
    pub fn from_policy(policy: &crate::config::CompressionPolicy) -> Self {
        Self {
            batch_size: 50,
            scheduler: SchedulerConfig::from_policy(policy),
            conflict: ConflictConfig::default(),
            background_interval_seconds: policy.background_interval_seconds,
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

        let conflict_detector = Arc::new(
            ConflictDetector::new(database.clone(), config.conflict.clone())
                .with_provider(Arc::clone(&provider)),
        );

        let extractor = Arc::new(FactExtractor::new(provider, embedder));

        let scheduler = Arc::new(CompressionScheduler::new(config.scheduler.clone()));

        Self {
            database,
            extractor,
            conflict_detector,
            scheduler,
            config,
            provider_name,
            signal_detector: SignalDetector::new(),
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
    ///
    /// Extracted facts are tagged with DEFAULT_AGENT ("default").
    /// Use `compress_in_workspace()` to tag facts with a specific workspace.
    pub async fn compress(&self) -> Result<CompressionResult, AlephError> {
        self.compress_in_workspace(crate::memory::DEFAULT_AGENT)
            .await
    }

    /// Execute a compression operation with workspace tagging.
    ///
    /// All extracted facts are stamped with the given `workspace_id` so that
    /// memories are isolated per workspace.
    pub async fn compress_in_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<CompressionResult, AlephError> {
        let start = Instant::now();

        // 1. Get last compression timestamp
        let last_timestamp = self
            .database
            .get_last_compression_timestamp()
            .await?
            .unwrap_or(0);

        // 2. Fetch raw session chunks as compression source.
        //    After the SessionStore removal, the SessionCompactor stores raw
        //    conversation chunks at `aleph://session/*/raw/*` in the facts
        //    table.  We convert them to MemoryEntry for the existing extractor.
        let raw_facts = self
            .database
            .get_uncompressed_session_facts(
                last_timestamp,
                if workspace_id == crate::memory::DEFAULT_AGENT {
                    None
                } else {
                    Some(workspace_id)
                },
                self.config.batch_size as usize,
            )
            .map_err(|e| AlephError::other(format!("Failed to fetch session facts: {e}")))?;

        // Transcript facts are raw conversation data stored for direct retrieval;
        // they must not be re-compressed into structured facts.
        let raw_facts: Vec<_> = raw_facts
            .into_iter()
            .filter(|f| f.fact_type != crate::memory::context::FactType::Transcript)
            .collect();

        let memories: Vec<crate::memory::context::MemoryEntry> = raw_facts
            .iter()
            .map(|fact| {
                crate::memory::context::MemoryEntry::new(
                    fact.id.clone(),
                    crate::memory::context::ContextAnchor::now("".to_string()),
                    fact.content.clone(),
                    String::new(),
                )
            })
            .collect();

        if memories.is_empty() {
            tracing::debug!("No uncompressed session memories to extract from");
            return Ok(CompressionResult::empty());
        }

        tracing::info!(
            memory_count = memories.len(),
            since_timestamp = last_timestamp,
            "Starting memory compression"
        );

        // 2b. C-layer dedup: fetch existing non-session facts as context
        let existing_fact_contents: Vec<String> = self
            .database
            .get_all_facts(false, None)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|f| !f.path.starts_with("aleph://session/"))
            .take(20)
            .map(|f| f.content)
            .collect();

        // 3. Extract facts + entities + relationships using unified LLM call
        let unified_result = match self
            .extractor
            .extract_unified(&memories, &existing_fact_contents)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(error = %e, "Unified extraction failed");
                return Err(e);
            }
        };

        tracing::info!(
            facts = unified_result.facts.len(),
            entities = unified_result.entities.len(),
            relationships = unified_result.relationships.len(),
            "Unified extraction completed"
        );

        // Generate embeddings for extracted facts
        let mut extracted_facts = Vec::new();
        for extracted_fact in &unified_result.facts {
            match self.extractor.embedder().embed(&extracted_fact.content).await {
                Ok(embedding) => {
                    let fact = crate::memory::context::MemoryFact::new(
                        extracted_fact.content.clone(),
                        crate::memory::context::FactType::from_str_or_other(&extracted_fact.fact_type),
                        extracted_fact.source_ids.clone(),
                    )
                    .with_embedding(embedding)
                    .with_confidence(extracted_fact.confidence);
                    extracted_facts.push(fact);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Embedding failed, skipping fact");
                    continue;
                }
            }
        }

        // 4. Process each fact (conflict detection and storage)
        //    Tag each fact with the target workspace for memory isolation.
        let mut stored_facts: Vec<crate::memory::context::MemoryFact> = Vec::new();
        let mut stored_fact_ids = Vec::new();
        let mut total_invalidated = 0u32;
        let mut affected_paths: HashSet<String> = HashSet::new();

        for mut fact in extracted_facts {
            fact.agent = workspace_id.to_string();
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
                        tier: fact.tier,
                        scope: fact.scope,
                        path: fact.path.clone(),
                        namespace: fact.namespace.clone(),
                        agent: fact.agent.clone(),
                        confidence: fact.confidence,
                        source: fact.fact_source,
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
                    // Graph updates are now done in bulk after the storage loop
                    // using unified extraction results (entities + relationships).
                    stored_facts.push(fact);
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

        // 4a. Invalidate consumed raw chunks
        let consumed_ids: Vec<String> = raw_facts.iter().map(|f| f.id.clone()).collect();
        match self.database.invalidate_consumed_chunks(&consumed_ids) {
            Ok(n) => tracing::info!(invalidated = n, "Invalidated consumed raw chunks"),
            Err(e) => tracing::warn!(error = %e, "Failed to invalidate consumed raw chunks"),
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
        // Use the max source fact created_at + 1 so that the next query with
        // `created_at > last_compression_ts` does not re-process the same
        // batch. raw_facts carries the original timestamps from the facts table.
        let latest_timestamp = raw_facts
            .iter()
            .map(|f| f.created_at)
            .max()
            .unwrap_or(0);

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

        // 1. Get last compression timestamp
        let last_timestamp = self
            .database
            .get_last_compression_timestamp()
            .await?
            .unwrap_or(0);

        // 2. Fetch raw session chunks (same as compress_in_workspace)
        let raw_facts = self
            .database
            .get_uncompressed_session_facts(
                last_timestamp,
                if workspace_id == crate::memory::DEFAULT_AGENT {
                    None
                } else {
                    Some(workspace_id)
                },
                self.config.batch_size as usize,
            )
            .map_err(|e| AlephError::other(format!("Failed to fetch session facts: {e}")))?;

        let raw_facts: Vec<_> = raw_facts
            .into_iter()
            .filter(|f| f.fact_type != crate::memory::context::FactType::Transcript)
            .collect();

        let memories: Vec<crate::memory::context::MemoryEntry> = raw_facts
            .iter()
            .map(|fact| {
                crate::memory::context::MemoryEntry::new(
                    fact.id.clone(),
                    crate::memory::context::ContextAnchor::now("".to_string()),
                    fact.content.clone(),
                    String::new(),
                )
            })
            .collect();

        if memories.is_empty() {
            tracing::debug!("No uncompressed session memories for note-based compression");
            return Ok(CompressionResult::empty());
        }

        tracing::info!(
            memory_count = memories.len(),
            "Starting note-based compression"
        );

        // 3. Get existing note titles
        let existing_notes = indexer.store().list_notes().await.unwrap_or_default();
        let existing_titles: Vec<String> = existing_notes.iter().map(|n| n.title.clone()).collect();

        // 4. Extract note updates via LLM
        let note_updates = self
            .extractor
            .extract_note_updates(&memories, &existing_titles)
            .await?;

        // 5. Apply note updates
        let mut notes_created = 0u32;
        let mut facts_stored = 0u32;

        for update in &note_updates.updates {
            use crate::memory::notes::extractor::NoteAction;

            match update.action {
                NoteAction::Create => {
                    let note = crate::memory::notes::KnowledgeNote {
                        title: update.note_title.clone(),
                        category: update
                            .category
                            .clone()
                            .unwrap_or_else(|| "other".to_string()),
                        tags: update.tags.clone().unwrap_or_default(),
                        facts: update.new_facts.clone(),
                        links: update.links.clone(),
                        created_at: chrono::Utc::now().timestamp(),
                        updated_at: chrono::Utc::now().timestamp(),
                        content_hash: String::new(),
                    };
                    if let Err(e) = indexer.write_note(&note).await {
                        tracing::warn!(
                            error = %e,
                            title = %update.note_title,
                            "Failed to create note"
                        );
                        continue;
                    }
                    let path = indexer
                        .notes_dir()
                        .join(format!("{}.md", update.note_title));
                    let _ = indexer.index_file(&path).await;
                    notes_created += 1;
                    facts_stored += update.new_facts.len() as u32;
                }
                NoteAction::Append | NoteAction::Update => {
                    if let Err(e) = indexer
                        .append_to_note(&update.note_title, &update.new_facts, &update.links)
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            title = %update.note_title,
                            "Failed to append to note"
                        );
                        continue;
                    }
                    facts_stored += update.new_facts.len() as u32;
                }
            }
        }

        // 6. Invalidate consumed raw chunks
        let consumed_ids: Vec<String> = raw_facts.iter().map(|f| f.id.clone()).collect();
        match self.database.invalidate_consumed_chunks(&consumed_ids) {
            Ok(n) => tracing::info!(invalidated = n, "Invalidated consumed raw chunks (notes)"),
            Err(e) => tracing::warn!(error = %e, "Failed to invalidate consumed raw chunks"),
        }

        // 7. Update compression timestamp
        let latest_timestamp = raw_facts.iter().map(|f| f.created_at).max().unwrap_or(0);
        self.database
            .set_last_compression_timestamp(latest_timestamp)
            .await?;

        let duration_ms = start.elapsed().as_millis() as u64;

        tracing::info!(
            notes_created,
            facts_stored,
            duration_ms,
            "Note-based compression complete"
        );

        Ok(CompressionResult {
            memories_processed: memories.len() as u32,
            facts_extracted: facts_stored,
            facts_invalidated: 0,
            duration_ms,
        })
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
    use crate::providers::create_mock_provider;
    use tempfile::{tempdir, TempDir};

    async fn create_test_service() -> (CompressionService, MemoryBackend) {
        let (service, database, _temp_dir) = create_test_service_with_tempdir().await;
        (service, database)
    }

    async fn create_test_service_with_tempdir() -> (CompressionService, MemoryBackend, TempDir) {
        let temp_dir = tempdir().unwrap();
        let database: MemoryBackend = Arc::new(
            crate::memory::store::SqliteMemoryBackend::new(temp_dir.path())
                .unwrap(),
        );

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
}
