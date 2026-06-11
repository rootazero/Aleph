//! Compression Service
//!
//! Main service that orchestrates memory compression:
//! 1. Fetches uncompressed memories
//! 2. Extracts facts using LLM
//! 3. Detects and resolves conflicts
//! 4. Stores facts and updates compression state

use super::scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
use super::signal_detector::SignalDetector;
use crate::error::AlephError;
use crate::memory::context::CompressionResult;
use crate::memory::events::handler::MemoryCommandHandler;
use crate::memory::store::{CompressionStore, MemoryBackend};
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock as TokioRwLock;
use tokio::task::JoinHandle;
use tokio::time::interval;

/// Hook fired after `compress_default_notes` succeeds for a single agent.
///
/// Spec A Task 18: lets `MemoryContextProvider` evict cached
/// `<CuratedMemory>` snapshots so the next prompt-build round reads fresh
/// state from disk.
pub trait PostCompressionHook: Send + Sync {
    fn on_compression_complete<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> futures::future::BoxFuture<'a, ()>;
}

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
    #[must_use]
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
    profile_synthesizer: Option<Arc<dyn crate::memory::notes::profile::ProfileSynthesizer>>,
    /// Optional memory-extension registry. When set, `compress_to_notes` fires
    /// `on_pre_compress` and folds the contribution into the ingest prompt.
    extension_registry: Option<Arc<crate::memory::extensions::MemoryExtensionRegistry>>,
    /// Hooks fired after `compress_default_notes` finishes successfully for an
    /// agent. Wrapped in `RwLock` so `add_post_hook(&self)` works through
    /// `Arc<CompressionService>` (the engine wraps the service in `Arc` before
    /// the MCP is constructed; we register the hook later from `agent_init`).
    post_hooks: TokioRwLock<Vec<Arc<dyn PostCompressionHook>>>,
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

    /// Create a new compression service with an optional `MemoryBackend` (kept for API compatibility)
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
            profile_synthesizer: None,
            extension_registry: None,
            post_hooks: TokioRwLock::new(Vec::new()),
        }
    }

    /// Register a [`PostCompressionHook`] that fires after each successful
    /// per-agent `compress_default_notes` call.
    ///
    /// `&self` (not `&mut self`) so callers holding `Arc<CompressionService>`
    /// can wire hooks AFTER the service has been wrapped — required because
    /// the engine constructs `Arc<CompressionService>` before the MCP that
    /// implements the hook is built.
    pub async fn add_post_hook(&self, hook: Arc<dyn PostCompressionHook>) {
        self.post_hooks.write().await.push(hook);
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

    /// Attach a `ProfileSynthesizer` to trigger user-profile updates on `SessionEnd`.
    ///
    /// When set, a fire-and-forget profile update is spawned after each
    /// `SessionEnd` batch is successfully ingested.
    pub fn with_profile_synthesizer(
        mut self,
        ps: Arc<dyn crate::memory::notes::profile::ProfileSynthesizer>,
    ) -> Self {
        self.profile_synthesizer = Some(ps);
        self
    }

    /// Attach a memory-extension registry so `compress_to_notes` fires
    /// `on_pre_compress` before ingest.
    pub fn with_extension_registry(
        mut self,
        registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.extension_registry = Some(registry);
        self
    }

    /// Execute a compression operation
    ///
    /// Routes through `compress_to_notes` by default, producing Knowledge
    /// Notes instead of raw facts.  Falls back to the legacy
    /// `compress_in_workspace` path only when the notes directory cannot be
    /// determined.
    pub async fn compress(&self) -> Result<CompressionResult, AlephError> {
        use crate::memory::store::raw_memory::RawMemoryStore;

        let mut agent_ids = self.database.unprocessed_agent_ids().await?;
        if agent_ids.is_empty() {
            agent_ids.push(crate::memory::DEFAULT_AGENT.to_string());
        }

        let mut total = CompressionResult::empty();
        for agent_id in &agent_ids {
            let result = self.compress_default_notes(agent_id).await?;
            total.memories_processed += result.memories_processed;
            total.facts_extracted += result.facts_extracted;
            total.facts_invalidated += result.facts_invalidated;
            total.duration_ms += result.duration_ms;

            // Spec A Task 18: notify hooks (e.g. MemoryContextProvider) so
            // cached <CuratedMemory> snapshots get evicted. Fire only on
            // success; the `?` above already aborts on Err.
            let hooks = self.post_hooks.read().await;
            for hook in hooks.iter() {
                hook.on_compression_complete(agent_id).await;
            }
        }

        self.scheduler.reset_turns();
        self.scheduler.record_activity();

        Ok(total)
    }

    /// Internal helper: delegate to `compress_to_notes`. The notes write path
    /// is owned by the compound ingestor's own `NoteIndexer` (wired at startup),
    /// so this layer no longer constructs one.
    async fn compress_default_notes(
        &self,
        workspace_id: &str,
    ) -> Result<CompressionResult, AlephError> {
        self.compress_to_notes(workspace_id).await
    }

    /// Compress memories into Knowledge Notes instead of raw facts.
    ///
    /// This method follows the same pipeline as `compress_in_workspace` but
    /// routes extracted information into markdown-based Knowledge Notes via
    /// `NoteIndexer` instead of storing individual `MemoryFact` rows.
    pub async fn compress_to_notes(
        &self,
        workspace_id: &str,
    ) -> Result<CompressionResult, AlephError> {
        let start = Instant::now();

        // 2. Fetch unprocessed raw memories
        use crate::memory::store::raw_memory::RawMemoryStore;

        let raw_memories = self
            .database
            .get_unprocessed_raw_memories(workspace_id, self.config.batch_size as usize)
            .await
            .map_err(|e| AlephError::other(format!("Failed to fetch raw memories: {e}")))?;

        // Spec 1 G3-B: Transcript raws (per-turn captures from the gateway
        // agent loop) ARE eligible for compression alongside SessionEnd /
        // PreCompress / Delegation. The historical filter excluded them on
        // the assumption a separate per-turn pipeline existed; that pipeline
        // never landed, so excluding Transcript starves the L1 layer.
        if raw_memories.is_empty() {
            tracing::debug!("No uncompressed session memories for note-based compression");
            return Ok(CompressionResult::empty());
        }

        tracing::info!(
            memory_count = raw_memories.len(),
            "Starting note-based compression"
        );

        // 4. Extract note updates via LLM — one call per source group (Spec 1).
        //    When compound_enabled, delegate each source batch to CompoundIngestor
        //    and skip the legacy accumulation path.
        if self.compound_enabled {
            // Rows to leave unprocessed for a later retry. Empty unless the
            // stop-the-bleed grace window (below) defers ingestable rows.
            let mut deferred_ids: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            if let Some(ing) = self.compound_ingestor.clone() {
                // ToolInvocation rows are per-call telemetry, consumed by the
                // insights aggregator and dream signal metrics (which read them
                // by source, independent of `is_processed`). They are NOT
                // knowledge — keep them out of the note-extraction LLM batch so
                // they neither waste a planning call nor pollute L1 with
                // "tool X ok in Yms" pseudo-notes. They are still marked
                // processed below (`consumed_ids` covers every fetched row) so
                // the unprocessed queue stays bounded.
                let ingest_rows: Vec<_> = raw_memories
                    .iter()
                    .filter(|r| {
                        !matches!(
                            r.source,
                            crate::memory::store::raw_memory::RawMemorySource::ToolInvocation { .. }
                        )
                    })
                    .cloned()
                    .collect();
                // X1 C3: let extensions contribute context before ingest.
                let extra_context: Option<String> = if let Some(reg) = &self.extension_registry {
                    let ctx = crate::memory::extensions::types::PreCompressCtx {
                        agent_id: workspace_id.to_string(),
                        namespace: crate::memory::namespace::NamespaceScope::Owner,
                        session_id: None,
                        messages_count: ingest_rows.len() as u32,
                        oldest_at: None,
                        newest_at: None,
                    };
                    let text = reg.dispatch_on_pre_compress(&ctx).await;
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                } else {
                    None
                };
                let ingest_outcome = ing
                    .ingest_batch(workspace_id, ingest_rows, extra_context.as_deref())
                    .await;

                // ProfileSynthesizer fires INDEPENDENTLY of compound ingest
                // result: a malformed LLM plan must not block USER.md updates.
                // Fire-and-forget, never block the compression flow.
                if let Some(ps) = self.profile_synthesizer.clone() {
                    use crate::memory::store::raw_memory::RawMemorySource;
                    let session_end_raws: Vec<_> = raw_memories
                        .iter()
                        .filter(|r| matches!(&r.source, RawMemorySource::SessionEnd { .. }))
                        .collect();
                    if !session_end_raws.is_empty() {
                        let agent = workspace_id.to_string();
                        let digest: String = session_end_raws
                            .iter()
                            .map(|r| r.content.clone())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let reason = match &session_end_raws[0].source {
                            RawMemorySource::SessionEnd { reason } => format!("{reason:?}"),
                            _ => "unknown".to_string(),
                        };
                        tracing::info!(
                            agent_id = %agent,
                            session_end_count = session_end_raws.len(),
                            "ProfileSynthesizer: firing on SessionEnd raws"
                        );
                        tokio::spawn(async move {
                            let signal = crate::memory::notes::profile::SessionSignal {
                                reason,
                                digest_text: digest,
                                recent_user_turns: vec![],
                                session_id: String::new(),
                            };
                            if let Err(e) = ps.update(&agent, signal).await {
                                tracing::warn!("profile update after session end failed: {e}");
                            } else {
                                tracing::info!(
                                    agent_id = %agent,
                                    "ProfileSynthesizer: USER.md update completed"
                                );
                            }
                        });
                    }
                }

                match ingest_outcome {
                    Ok(report) => {
                        // Stop-the-bleed: when the plan produced no notes, don't
                        // burn the knowledge. Defer marking the *ingestable* rows
                        // processed while they are still within the grace window,
                        // so a transiently-failed extraction (flaky planner LLM /
                        // embedding outage) gets retried on a later tick instead
                        // of being discarded forever. Telemetry rows are excluded
                        // from ingest and can never yield a note, so they are
                        // never deferred (they must stay bounded). Past the
                        // window even ingestable rows are marked, to bound the
                        // queue.
                        if report.is_empty() {
                            const RETRY_GRACE_SECS: i64 = 6 * 3600;
                            let now = chrono::Utc::now().timestamp();
                            for r in &raw_memories {
                                let is_telemetry = matches!(
                                    r.source,
                                    crate::memory::store::raw_memory::RawMemorySource::ToolInvocation { .. }
                                );
                                if !is_telemetry && now - r.created_at < RETRY_GRACE_SECS {
                                    deferred_ids.insert(r.id.clone());
                                }
                            }
                            if !deferred_ids.is_empty() {
                                tracing::info!(
                                    deferred = deferred_ids.len(),
                                    "compound ingest produced no notes; deferring \
                                     ingestable rows for retry (within grace window)"
                                );
                            }
                        }
                        // Fall through to mark_raw_as_processed below.
                    }
                    Err(e) => {
                        tracing::warn!("compound ingest failed: {e}");
                        // SessionEnd batches are still marked processed even
                        // when the compound plan failed — ProfileSynthesizer
                        // (above) already consumed them. Other batches retry
                        // next tick because the raws stay unprocessed.
                        use crate::memory::store::raw_memory::RawMemorySource;
                        let only_session_end = raw_memories
                            .iter()
                            .all(|r| matches!(&r.source, RawMemorySource::SessionEnd { .. }));
                        if only_session_end {
                            tracing::info!(
                                "compound ingest failed on SessionEnd-only batch; \
                                 marking processed (ProfileSynthesizer already fired)"
                            );
                            // fall through to mark_processed below
                        } else {
                            return Ok(CompressionResult::empty());
                        }
                    }
                }
            } else {
                tracing::warn!(
                    "compound ingest enabled but no ingestor configured; skipping batch"
                );
                return Ok(CompressionResult::empty());
            }

            // Mark raw memories as processed (compound path), excluding any rows
            // deferred for retry by the stop-the-bleed grace window above.
            let consumed_ids: Vec<String> = raw_memories
                .iter()
                .filter(|r| !deferred_ids.contains(&r.id))
                .map(|r| r.id.clone())
                .collect();
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

    /// Start background compression task
    ///
    /// Runs periodically and compresses unconditionally (bypasses scheduler).
    /// The scheduler-based triggers are handled separately via `record_turn_and_check_signal()`.
    pub fn start_background_task(self: Arc<Self>) -> JoinHandle<()> {
        let interval_secs = self.config.background_interval_seconds;

        tokio::spawn(async move {
            // `tokio::time::interval` panics on a zero period; clamp a
            // misconfigured 0 from user config to 1s instead of killing the task.
            let mut interval = interval(Duration::from_secs(u64::from(interval_secs.max(1))));

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
    /// This method is used during `AlephCore` initialization when we have a runtime
    /// but are not yet inside its context (so `tokio::spawn` won't work).
    ///
    /// Compresses unconditionally on each interval tick.
    pub fn start_background_task_with_runtime(
        self: &Arc<Self>,
        runtime: &tokio::runtime::Runtime,
    ) -> JoinHandle<()> {
        let service = Arc::clone(self);
        let interval_secs = self.config.background_interval_seconds;

        runtime.spawn(async move {
            // `tokio::time::interval` panics on a zero period; clamp a
            // misconfigured 0 from user config to 1s instead of killing the task.
            let mut hourly_interval = interval(Duration::from_secs(u64::from(interval_secs.max(1))));

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

    /// Record a conversation turn and trigger compression — signal-aware.
    ///
    /// Always counts the turn with exactly-once threshold-crossing semantics
    /// (so the turn-threshold path keeps working). Additionally, if the user
    /// message carries an `Immediate` signal (a correction like "不对/错了/
    /// wrong"), compress NOW instead of waiting for the threshold. Learning
    /// and milestone signals ride the normal turn-threshold cadence.
    ///
    /// Non-blocking: the actual compression runs in a spawned task.
    pub fn record_turn_and_check_signal(self: &Arc<Self>, user_message: &str) {
        let detection = self.signal_detector.detect(user_message);

        // Count the turn exactly once at the threshold crossing.
        let old_turns = self
            .scheduler
            .pending_turns
            .fetch_add(1, crate::sync_primitives::Ordering::AcqRel);
        let turns = old_turns + 1;
        let threshold = self.config.scheduler.turn_threshold;
        let threshold_crossed = old_turns < threshold && turns >= threshold;

        let immediate = detection.should_compress
            && detection.priority == super::signal_detector::CompressionPriority::Immediate;

        if immediate {
            tracing::info!(signals = ?detection.signals, "Signal-triggered compression (immediate)");
            let service = Arc::clone(self);
            tokio::spawn(async move {
                match service.compress().await {
                    Ok(result) => tracing::info!(
                        facts = result.facts_extracted,
                        "Immediate compression completed (signal)"
                    ),
                    Err(e) => tracing::error!(error = %e, "Immediate compression failed (signal)"),
                }
            });
        } else if threshold_crossed {
            tracing::info!(
                turns,
                threshold,
                "Turn threshold reached, triggering compression"
            );
            let service = Arc::clone(self);
            tokio::spawn(async move {
                match service.check_and_compress().await {
                    Ok(Some(result)) => tracing::info!(
                        facts = result.facts_extracted,
                        "Immediate compression completed (turn threshold)"
                    ),
                    Ok(None) => tracing::debug!("Compression: no action needed"),
                    Err(e) => tracing::error!(error = %e, "Compression failed (turn threshold)"),
                }
            });
        }
    }

    /// Get the scheduler for external monitoring
    pub fn get_scheduler(&self) -> Arc<CompressionScheduler> {
        Arc::clone(&self.scheduler)
    }

    /// Get current configuration
    pub const fn get_config(&self) -> &CompressionConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Empty database: compression returns an empty result without error.
    #[tokio::test]
    async fn test_compress_to_notes_empty_database() {
        let (service, _database, _temp_dir) = create_test_service_with_tempdir().await;

        let result = service.compress_to_notes("default").await.unwrap();

        assert_eq!(result.memories_processed, 0);
        assert_eq!(result.facts_extracted, 0);
        assert_eq!(result.duration_ms, 0, "Empty run should be instant");
    }

    /// Regression: `ToolInvocation` telemetry rows must be excluded from the
    /// note-extraction batch (they are metrics for insights/dream signals, not
    /// knowledge) but must STILL be marked processed so the queue stays
    /// bounded.
    #[tokio::test]
    async fn compress_to_notes_excludes_tool_invocation_telemetry() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};
        use crate::sync_primitives::Mutex;

        // Recording ingestor: captures the source of every row handed to it.
        struct RecordingIngestor {
            seen: Mutex<Vec<RawMemorySource>>,
        }
        #[async_trait::async_trait]
        impl CompoundIngestor for RecordingIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                self.seen
                    .lock()
                    .unwrap()
                    .extend(raws.iter().map(|r| r.source.clone()));
                Ok(ApplyReport::default())
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let spy = Arc::new(RecordingIngestor {
            seen: Mutex::new(vec![]),
        });
        let service = service.with_compound_ingestor(spy.clone());

        let transcript =
            RawMemory::new("real conversation".to_string(), RawMemorySource::Transcript);
        let telemetry = RawMemory::new(
            "tool grep ok in 5ms".to_string(),
            RawMemorySource::ToolInvocation {
                tool_name: "grep".to_string(),
                success: true,
                duration_ms: 5,
            },
        );
        database.insert_raw_memory(&transcript).await.unwrap();
        database.insert_raw_memory(&telemetry).await.unwrap();

        service.compress_to_notes("default").await.unwrap();

        // Only the transcript should have reached the ingestor.
        let seen = spy.seen.lock().unwrap().clone();
        assert_eq!(
            seen.len(),
            1,
            "only the non-telemetry row should reach the ingestor"
        );
        assert!(matches!(seen[0], RawMemorySource::Transcript));

        // The telemetry row produced no note (stub returns empty) but must
        // STILL be marked processed — it can never yield a note, so the
        // stop-the-bleed grace window must never defer it. The young transcript,
        // by contrast, IS deferred for retry (it could yield a note once the
        // planner recovers), so it remains unprocessed.
        let unprocessed = database
            .get_unprocessed_raw_memories("default", 10)
            .await
            .unwrap();
        assert_eq!(
            unprocessed.len(),
            1,
            "telemetry must be marked; only the deferred transcript stays unprocessed"
        );
        assert!(
            matches!(unprocessed[0].source, RawMemorySource::Transcript),
            "the single deferred row must be the ingestable transcript, not telemetry"
        );
    }

    /// Stop-the-bleed: an empty plan over an ingestable row that has aged past
    /// the retry grace window must be marked processed (give-up), so the
    /// unprocessed queue stays bounded even when extraction keeps failing.
    #[tokio::test]
    async fn compress_to_notes_marks_aged_rows_when_plan_empty() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};

        struct EmptyIngestor;
        #[async_trait::async_trait]
        impl CompoundIngestor for EmptyIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                Ok(ApplyReport::default())
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let service = service.with_compound_ingestor(Arc::new(EmptyIngestor));

        // Aged transcript: created well past the 6h grace window.
        let mut aged = RawMemory::new("old conversation".to_string(), RawMemorySource::Transcript);
        aged.created_at = chrono::Utc::now().timestamp() - 7 * 3600;
        database.insert_raw_memory(&aged).await.unwrap();

        service.compress_to_notes("default").await.unwrap();

        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            0,
            "aged ingestable rows must be marked processed once past the grace window"
        );
    }

    /// Stop-the-bleed: a young ingestable row whose plan produced no note is
    /// deferred (left unprocessed) so a later tick can retry once the planner or
    /// embedding backend recovers — knowledge is not discarded on first failure.
    #[tokio::test]
    async fn compress_to_notes_defers_young_rows_when_plan_empty() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};

        struct EmptyIngestor;
        #[async_trait::async_trait]
        impl CompoundIngestor for EmptyIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                Ok(ApplyReport::default())
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let service = service.with_compound_ingestor(Arc::new(EmptyIngestor));

        let fresh = RawMemory::new("new conversation".to_string(), RawMemorySource::Transcript);
        database.insert_raw_memory(&fresh).await.unwrap();

        service.compress_to_notes("default").await.unwrap();

        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            1,
            "young ingestable rows must be deferred for retry, not burned"
        );
    }

    /// X1 C3: a registered `on_pre_compress` extension's contribution must reach
    /// `ingest_batch` as `extra_context`.
    #[tokio::test]
    async fn pre_compress_contribution_reaches_ingest_extra_context() {
        use crate::memory::extensions::types::PreCompressCtx;
        use crate::memory::extensions::{MemoryExtension, MemoryExtensionRegistry};
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};

        // Extension that contributes fixed text on pre-compress.
        struct ContribExt;
        #[async_trait::async_trait]
        impl MemoryExtension for ContribExt {
            fn name(&self) -> &str {
                "test.contrib"
            }
            async fn on_pre_compress(&self, _ctx: &PreCompressCtx) -> Result<String, AlephError> {
                Ok("CONTRIB".to_string())
            }
        }

        // Recording ingestor that captures the extra_context it receives.
        let seen = Arc::new(crate::sync_primitives::Mutex::new(None::<String>));
        struct RecordIngestor {
            seen: Arc<crate::sync_primitives::Mutex<Option<String>>>,
        }
        #[async_trait::async_trait]
        impl CompoundIngestor for RecordIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<RawMemory>,
                extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                *self.seen.lock().unwrap_or_else(|e| e.into_inner()) =
                    extra_context.map(|s| s.to_string());
                Ok(ApplyReport::default())
            }
        }

        let reg = Arc::new(MemoryExtensionRegistry::new());
        reg.register(Arc::new(ContribExt));

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let service = service
            .with_compound_ingestor(Arc::new(RecordIngestor { seen: seen.clone() }))
            .with_extension_registry(reg);

        let transcript =
            RawMemory::new("real conversation".to_string(), RawMemorySource::Transcript);
        database.insert_raw_memory(&transcript).await.unwrap();

        service.compress_to_notes("default").await.unwrap();

        let captured = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            captured.as_deref(),
            Some("CONTRIB"),
            "the on_pre_compress contribution must reach ingest_batch as extra_context"
        );
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
            .mark_raw_as_processed(std::slice::from_ref(&raw3.id))
            .await
            .unwrap();

        assert_eq!(backend.count_unprocessed("default").await.unwrap(), 0);
    }

    #[test]
    fn correction_message_classifies_immediate() {
        let detector = crate::memory::compression::signal_detector::SignalDetector::new();
        let d = detector.detect("不对，我说的是 Rust");
        assert!(d.should_compress);
        assert_eq!(
            d.priority,
            crate::memory::compression::signal_detector::CompressionPriority::Immediate
        );
    }

    #[test]
    fn neutral_message_does_not_force_compression() {
        let detector = crate::memory::compression::signal_detector::SignalDetector::new();
        let d = detector.detect("帮我看一下这段代码");
        assert!(!d.should_compress);
    }
}
