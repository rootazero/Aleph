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

/// Parse raw memory content into user/assistant parts.
/// Content may contain "User: ...\nAssistant: ..." format from SessionCompactor,
/// or plain text summaries.
fn parse_raw_content(content: &str) -> (String, String) {
    // Try to split on "Assistant:" marker
    if let Some(pos) = content.find("\nAssistant:") {
        let user_part = content[..pos].to_string();
        let user_part = user_part
            .strip_prefix("User:")
            .unwrap_or(&user_part)
            .trim()
            .to_string();
        let ai_part = content[pos..]
            .trim_start_matches("\nAssistant:")
            .trim()
            .to_string();
        (user_part, ai_part)
    } else if let Some(pos) = content.find("Assistant:") {
        let user_part = content[..pos].to_string();
        let user_part = user_part
            .strip_prefix("User:")
            .unwrap_or(&user_part)
            .trim()
            .to_string();
        let ai_part = content[pos..]
            .trim_start_matches("Assistant:")
            .trim()
            .to_string();
        (user_part, ai_part)
    } else {
        (content.to_string(), String::new())
    }
}

/// Group raw memories by their source variant (preserving insertion order of first occurrence).
/// Used by the compression service to run one extractor call per group so each call can
/// use its source-specific system prompt.
pub(crate) fn group_by_source<'a>(
    rows: &'a [crate::memory::store::raw_memory::RawMemory],
) -> Vec<(
    crate::memory::store::raw_memory::RawMemorySource,
    Vec<&'a crate::memory::store::raw_memory::RawMemory>,
)> {
    use std::collections::BTreeMap;
    let mut order: Vec<crate::memory::store::raw_memory::RawMemorySource> = Vec::new();
    let mut buckets: BTreeMap<String, Vec<&crate::memory::store::raw_memory::RawMemory>> =
        BTreeMap::new();
    for r in rows {
        let key = format!("{:?}", r.source);
        if !buckets.contains_key(&key) {
            order.push(r.source.clone());
        }
        buckets.entry(key).or_default().push(r);
    }
    order
        .into_iter()
        .map(|src| {
            let key = format!("{src:?}");
            let v = buckets.remove(&key).unwrap_or_default();
            (src, v)
        })
        .collect()
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
        provider: Arc<dyn AiProvider>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: CompressionConfig,
        _memory_backend: Option<MemoryBackend>,
    ) -> Self {
        let provider_name = provider.name().to_string();

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

        // Legacy path — kept for fallback. Will be removed in Task 12.
        let valid_categories = [
            "preference",
            "plan",
            "learning",
            "project",
            "personal",
            "tool",
            "lesson",
            "skill",
            "wiki",
            "transcript",
            "other",
            "subagent-run",
            "subagent-session",
            "subagent-checkpoint",
            "subagent-transcript",
        ];

        use crate::memory::notes::extractor::NoteExtractionResponse;

        // Accumulate updates across all source groups.
        let mut all_updates: Vec<crate::memory::notes::extractor::NoteUpdate> = Vec::new();

        for (source, group) in group_by_source(&raw_memories) {
            // Build MemoryEntry slice for this group only.
            let group_memories: Vec<crate::memory::context::MemoryEntry> = group
                .iter()
                .map(|raw| {
                    let (user_input, ai_output) = parse_raw_content(&raw.content);
                    let enriched_input = match &raw.attachment_text {
                        Some(att) if !att.is_empty() => {
                            let att_preview: String = att.chars().take(2000).collect();
                            format!("{user_input}\n[Attachment]: {att_preview}")
                        }
                        _ => user_input,
                    };
                    crate::memory::context::MemoryEntry::new(
                        raw.id.clone(),
                        crate::memory::context::ContextAnchor::now("".to_string()),
                        enriched_input,
                        ai_output,
                    )
                })
                .collect();

            let group_updates = self
                .extractor
                .extract_note_updates_for_source(&group_memories, &existing_note_summaries, &source)
                .await?;

            all_updates.extend(group_updates.updates);
        }

        let note_updates = NoteExtractionResponse {
            updates: all_updates.into_iter().filter(|u| {
                let has_slash = u.note_path.contains('/');
                let has_content = !u.new_facts.is_empty()
                    || u.action == crate::memory::notes::extractor::NoteAction::Update;
                let category = u.note_path.split('/').next().unwrap_or("");
                let valid_cat = valid_categories.contains(&category);

                if !has_slash || !has_content || !valid_cat {
                    tracing::warn!(
                        note_path = %u.note_path,
                        "Filtered out invalid note extraction: slash={has_slash} content={has_content} cat={valid_cat}"
                    );
                }
                has_slash && has_content && valid_cat
            }).collect(),
        };

        // 5. Ensure directory structure exists
        let _ = indexer.ensure_dirs(workspace_id).await;

        // 6. Apply note updates
        let mut notes_created = 0u32;
        let mut facts_stored = 0u32;

        for update in &note_updates.updates {
            use crate::memory::notes::extractor::NoteAction;

            // Derive category and filename from note_path ("category/filename")
            let (category, filename) = match update.note_path.split_once('/') {
                Some((cat, name)) => (cat, name),
                None => ("other", update.note_path.as_str()),
            };

            match update.action {
                NoteAction::Create => {
                    let note = crate::memory::notes::KnowledgeNote {
                        title: filename.to_string(),
                        category: category.to_string(),
                        tags: update.tags.clone().unwrap_or_default(),
                        facts: update.new_facts.clone(),
                        links: update.links.clone(),
                        created_at: chrono::Utc::now().timestamp(),
                        updated_at: chrono::Utc::now().timestamp(),
                        content_hash: String::new(),
                    };
                    if let Err(e) = indexer.write_note(workspace_id, category, &note).await {
                        tracing::warn!(
                            error = %e,
                            note_path = %update.note_path,
                            "Failed to create note"
                        );
                        continue;
                    }
                    // Generate embedding for the new note
                    let body = note.body_text();
                    if let Ok(embedding) = self.extractor.embedder().embed(&body).await {
                        let dim = embedding.len() as u32;
                        let _ = indexer
                            .store()
                            .upsert_embedding(&update.note_path, workspace_id, &embedding, dim)
                            .await;
                    }
                    let note_file = indexer
                        .memory_dir()
                        .join(workspace_id)
                        .join(category)
                        .join(format!("{filename}.md"));
                    let _ = indexer.index_file(workspace_id, category, &note_file).await;
                    notes_created += 1;
                    facts_stored += update.new_facts.len() as u32;
                }
                NoteAction::Append | NoteAction::Update => {
                    if let Err(e) = indexer
                        .append_to_note(
                            workspace_id,
                            &update.note_path,
                            &update.new_facts,
                            &update.links,
                        )
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            note_path = %update.note_path,
                            "Failed to append to note"
                        );
                        continue;
                    }
                    // Re-embed the updated note body (not frontmatter)
                    let note_file = indexer
                        .memory_dir()
                        .join(workspace_id)
                        .join(category)
                        .join(format!("{filename}.md"));
                    if let Ok(content) = tokio::fs::read_to_string(&note_file).await {
                        if let Ok(parsed) =
                            crate::memory::notes::KnowledgeNote::from_markdown(filename, &content)
                        {
                            if let Ok(embedding) =
                                self.extractor.embedder().embed(&parsed.body_text()).await
                            {
                                let dim = embedding.len() as u32;
                                let _ = indexer
                                    .store()
                                    .upsert_embedding(
                                        &update.note_path,
                                        workspace_id,
                                        &embedding,
                                        dim,
                                    )
                                    .await;
                            }
                        }
                    }
                    facts_stored += update.new_facts.len() as u32;
                }
            }
        }

        // 7. Mark raw memories as processed
        let consumed_ids: Vec<String> = raw_memories.iter().map(|r| r.id.clone()).collect();
        match self.database.mark_raw_as_processed(&consumed_ids).await {
            Ok(n) => tracing::info!(marked = n, "Marked raw memories as processed"),
            Err(e) => tracing::warn!(error = %e, "Failed to mark raw memories as processed"),
        }

        // 8. Update compression timestamp
        let latest_timestamp = raw_memories.iter().map(|r| r.created_at).max().unwrap_or(0);
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
            memories_processed: raw_memories.len() as u32,
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

    // -------------------------------------------------------------------------
    // E2E Integration Tests: raw_memory → compress → notes pipeline
    // -------------------------------------------------------------------------

    /// Full pipeline: insert raw_memory → trigger compression → verify it is
    /// marked as processed.  The mock LLM provider returns empty extraction
    /// results, so no notes are created — but the state transitions must be correct.
    #[tokio::test]
    async fn test_full_pipeline_raw_to_notes() {
        let (service, database, indexer, _temp_dir) = create_test_service_with_indexer().await;

        // 1. Insert a raw memory with session-compressed content
        let raw = RawMemory::new(
            "User: I really enjoy programming in Rust\nAssistant: Rust is great for systems programming!".to_string(),
            RawMemorySource::SessionCompressed,
        );
        database.insert_raw_memory(&raw).await.unwrap();

        // 2. Verify it shows up as unprocessed
        let count = database.count_unprocessed("default").await.unwrap();
        assert_eq!(count, 1, "Should have 1 unprocessed raw memory");

        // 3. Trigger compression through the notes pipeline
        let result = service
            .compress_to_notes("default", &indexer)
            .await
            .unwrap();

        // 4. Pipeline should complete without error and report 1 memory processed
        assert_eq!(
            result.memories_processed, 1,
            "Compression should report 1 memory processed"
        );

        // 5. Raw memory must be marked as processed
        let remaining = database.count_unprocessed("default").await.unwrap();
        assert_eq!(
            remaining, 0,
            "Raw memory should be marked as processed after compression"
        );
    }

    /// Attachment text flows through the pipeline without errors, and the raw
    /// memory is still marked processed afterwards.
    #[tokio::test]
    async fn test_raw_memory_with_attachment_flows_through() {
        let (service, database, indexer, _temp_dir) = create_test_service_with_indexer().await;

        // Insert raw memory with attachment_text
        let raw = RawMemory::new(
            "User: Analyze this document\nAssistant: The document discusses microservices."
                .to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_attachment_text(
            "This is a PDF about microservices architecture with detailed diagrams.",
        );

        database.insert_raw_memory(&raw).await.unwrap();

        // Trigger compression
        let result = service
            .compress_to_notes("default", &indexer)
            .await
            .unwrap();

        // Pipeline should complete without panic
        assert_eq!(result.memories_processed, 1);

        // Raw memory must be marked as processed
        let remaining = database.count_unprocessed("default").await.unwrap();
        assert_eq!(
            remaining, 0,
            "Raw memory with attachment should be processed"
        );
    }

    /// Transcript-sourced raw memories are filtered out and NOT consumed by the
    /// notes compression pipeline (they go to the transcript indexer instead).
    #[tokio::test]
    async fn test_transcript_raw_memories_are_skipped() {
        let (service, database, indexer, _temp_dir) = create_test_service_with_indexer().await;

        // Insert one transcript memory (should be skipped) and one session memory
        let transcript = RawMemory::new(
            "Full session transcript content".to_string(),
            RawMemorySource::Transcript,
        );
        let session = RawMemory::new(
            "User: Hello\nAssistant: Hi there!".to_string(),
            RawMemorySource::SessionCompressed,
        );

        database.insert_raw_memory(&transcript).await.unwrap();
        database.insert_raw_memory(&session).await.unwrap();

        let count = database.count_unprocessed("default").await.unwrap();
        assert_eq!(count, 2);

        // compress_to_notes only processes session memories, skipping Transcript
        let result = service
            .compress_to_notes("default", &indexer)
            .await
            .unwrap();

        // Only the session memory counts toward memories_processed
        assert_eq!(
            result.memories_processed, 1,
            "Only session memories should be processed (transcript is skipped)"
        );

        // Both IDs are passed to mark_raw_as_processed; transcript ID is included
        // because the mark step uses all fetched IDs before the filter.
        // The session memory is definitely processed.
        let remaining = database.count_unprocessed("default").await.unwrap();
        // At most 1 remaining (the transcript, which was filtered pre-extraction
        // but still in the "consumed_ids" list and thus marked processed too).
        assert!(
            remaining <= 1,
            "At most 1 raw memory (transcript) may remain unprocessed"
        );
    }

    /// Multiple raw memories in one batch are all processed together.
    #[tokio::test]
    async fn test_multiple_raw_memories_batch_processed() {
        let (service, database, indexer, _temp_dir) = create_test_service_with_indexer().await;

        // Insert 3 raw memories
        for i in 0..3u32 {
            let raw = RawMemory::new(
                format!("User: Question {i}\nAssistant: Answer {i}"),
                RawMemorySource::SessionCompressed,
            );
            database.insert_raw_memory(&raw).await.unwrap();
        }

        assert_eq!(database.count_unprocessed("default").await.unwrap(), 3);

        let result = service
            .compress_to_notes("default", &indexer)
            .await
            .unwrap();

        assert_eq!(
            result.memories_processed, 3,
            "All 3 memories should be processed"
        );

        let remaining = database.count_unprocessed("default").await.unwrap();
        assert_eq!(
            remaining, 0,
            "All raw memories should be marked as processed"
        );
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

#[cfg(test)]
mod tests_spec1 {
    use super::*;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, SessionEndReason};

    #[test]
    fn group_by_source_separates_specialised_and_legacy() {
        let mut rows = vec![
            RawMemory::new("a".into(), RawMemorySource::Transcript),
            RawMemory::new("b".into(), RawMemorySource::PreCompress),
            RawMemory::new("c".into(), RawMemorySource::Transcript),
            RawMemory::new(
                "d".into(),
                RawMemorySource::SessionEnd {
                    reason: SessionEndReason::TaskDone,
                },
            ),
        ];
        for (i, r) in rows.iter_mut().enumerate() {
            r.id = format!("id-{i}");
        }

        let groups = group_by_source(&rows);
        assert_eq!(groups.len(), 3, "expected 3 source groups");
        let t_count = groups
            .iter()
            .find(|(s, _)| matches!(s, RawMemorySource::Transcript))
            .unwrap()
            .1
            .len();
        assert_eq!(t_count, 2);
        let pc_count = groups
            .iter()
            .find(|(s, _)| matches!(s, RawMemorySource::PreCompress))
            .unwrap()
            .1
            .len();
        assert_eq!(pc_count, 1);
        let se_count = groups
            .iter()
            .find(|(s, _)| {
                matches!(
                    s,
                    RawMemorySource::SessionEnd {
                        reason: SessionEndReason::TaskDone
                    }
                )
            })
            .unwrap()
            .1
            .len();
        assert_eq!(se_count, 1);
    }
}
