//! Compression Service
//!
//! Main service that orchestrates memory compression:
//! 1. Fetches uncompressed memories
//! 2. Extracts facts using LLM
//! 3. Detects and resolves conflicts
//! 4. Stores facts and updates compression state

use super::scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
use crate::error::AlephError;
use crate::memory::context::CompressionResult;
use crate::memory::store::{CompressionStore, MemoryBackend};
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as TokioMutex;
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
    /// Per-agent ingest lock, keyed by `workspace_id`.
    ///
    /// `compress_to_notes` reads the unprocessed batch, spends seconds inside an
    /// LLM ingest call, and only marks those rows processed at the END — a
    /// read-then-mark window with no DB-level claim (`get_unprocessed_raw_memories`
    /// is a plain `WHERE is_processed = 0` SELECT). Several unsynchronized sites
    /// drive one shared `Arc<CompressionService>`: the hourly background task, the
    /// session-end flush, the turn-threshold spawn, the correction flush, and the
    /// `memory.compress` RPC. Two runs overlapping on one agent would therefore
    /// fetch the SAME rows and hand them to `CompoundIngestor::ingest_batch` twice
    /// — duplicate note pages, doubled `ProfileSynthesizer` runs over one SessionEnd
    /// digest, doubled LLM spend.
    ///
    /// Serializing per agent closes the window: the second caller waits, then
    /// re-reads and finds the batch already drained. Per-agent (not global) so
    /// unrelated agents still compress concurrently. The daemon is a singleton
    /// (OS `flock`), so an in-process lock is sufficient — no DB claim column and
    /// no migration.
    ingest_locks: TokioMutex<std::collections::HashMap<String, Arc<TokioMutex<()>>>>,
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
        // rust-doctor-disable-next-line excessive-clone
        let scheduler = Arc::new(CompressionScheduler::new(config.scheduler.clone()));

        Self {
            database,
            scheduler,
            config,
            compound_ingestor: None,
            compound_enabled: false,
            profile_synthesizer: None,
            extension_registry: None,
            post_hooks: TokioRwLock::new(Vec::new()),
            ingest_locks: TokioMutex::new(std::collections::HashMap::new()),
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

        // Consume the turn budget UP FRONT — before the first fallible DB call,
        // not after the loop. Every fallible step below (starting with the
        // `unprocessed_agent_ids` fetch just after this) propagates with `?`, so a
        // single transient DB error used to return before the reset — leaving
        // `pending_turns` pinned above the threshold forever. Because
        // `record_turn_and_maybe_compress` fires on the exactly-once CROSSING
        // (`old < threshold && new >= threshold`), a counter stuck above the
        // threshold can never cross again: the turn-threshold trigger died
        // permanently until some other path's compress() happened to succeed.
        // The trigger has already fired by the time we get here, so the budget is
        // spent whether or not this run completes.
        self.scheduler.reset_turns();

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
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub async fn compress_to_notes(
        &self,
        workspace_id: &str,
    ) -> Result<CompressionResult, AlephError> {
        let start = Instant::now();

        // Serialize compressions for this agent — see `ingest_locks`. Held across
        // the fetch → LLM ingest → mark-processed window so a concurrent caller
        // cannot read the same unprocessed rows and ingest them twice.
        let agent_lock = {
            let mut locks = self.ingest_locks.lock().await;
            Arc::clone(
                locks
                    .entry(workspace_id.to_string())
                    .or_insert_with(|| Arc::new(TokioMutex::new(()))),
            )
        };
        let _ingest_guard = agent_lock.lock().await;

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
        //    When compound_enabled, delegate each source group to CompoundIngestor
        //    and skip the legacy accumulation path.
        if self.compound_enabled {
            use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};

            // rust-doctor-disable-next-line excessive-clone
            let Some(ing) = self.compound_ingestor.clone() else {
                tracing::warn!(
                    "compound ingest enabled but no ingestor configured; skipping batch"
                );
                return Ok(CompressionResult::empty());
            };

            // Rows this drain must NOT hand to the note-extraction LLM, because
            // a different consumer already owns them and that consumer reads
            // `raw_memories` by source/path — never through `is_processed`:
            //
            //   * `ToolInvocation` — per-call telemetry for the insights
            //     aggregator and the dream signal metrics. Not knowledge; would
            //     pollute L1 with "tool X ok in Yms" pseudo-notes.
            //   * `Correction` — owned by the `FeedbackDistill` dream stage,
            //     which reads them via `get_raw_by_path_prefix_since` on the
            //     `aleph://correction/` prefix. Leaving them in the batch made
            //     every correction land TWICE: once as a generic note here and
            //     once as a feedback rule there — and `flag_user_correction`
            //     kicks this very drain, so the double ingest was immediate,
            //     not hypothetical. (`source_prompts::prompt_for` has always
            //     claimed corrections never reach this path; now that is true.)
            //
            // Both are marked processed immediately below rather than left for
            // the stop-the-bleed retry grace: neither can ever yield a note, so
            // deferring them would only grow the unprocessed queue forever.
            // Marking is safe precisely because their real consumers ignore the
            // flag.
            let (bypassed, ingestable): (Vec<RawMemory>, Vec<RawMemory>) =
                raw_memories.iter().cloned().partition(|r| {
                    matches!(
                        r.source,
                        RawMemorySource::ToolInvocation { .. } | RawMemorySource::Correction { .. }
                    )
                });

            // X1 C3: let extensions contribute context before ingest. Computed
            // once for the whole drained batch and shared by every per-source
            // ingest call below — extensions fire once per drain, not per group.
            let extra_context: Option<String> = if let Some(reg) = &self.extension_registry {
                let ctx = crate::memory::extensions::types::PreCompressCtx {
                    agent_id: workspace_id.to_string(),
                    namespace: crate::memory::namespace::NamespaceScope::Owner,
                    session_id: None,
                    messages_count: ingestable.len() as u32,
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

            // ProfileSynthesizer fires INDEPENDENTLY of compound ingest
            // result: a malformed LLM plan must not block USER.md updates.
            // Fire-and-forget, never block the compression flow.
            if let Some(ps) = self.profile_synthesizer.clone() {
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

            let mut memories_processed: u32 = 0;
            let mut facts_extracted: u32 = 0;
            let mut facts_invalidated: u32 = 0;

            // Bypassed rows never enter an ingest group; mark them right away.
            if !bypassed.is_empty() {
                let ids: Vec<String> = bypassed.iter().map(|r| r.id.clone()).collect();
                match self.database.mark_raw_as_processed(&ids).await {
                    Ok(n) => {
                        memories_processed += n as u32;
                        tracing::info!(marked = n, "Marked non-ingestable raws as processed");
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Failed to mark non-ingestable raws as processed"
                        );
                    }
                }
            }

            // Group the ingestable rows by prompt-affecting source identity,
            // preserving fetch order within each group and first-seen group
            // order. The ingestor derives its per-source prompt from
            // `raws[0].source`, so a mixed drain MUST be split or
            // Reflection/SessionEnd/Delegation rows silently degrade to
            // whichever source happened to be fetched first.
            let mut groups: Vec<(String, Vec<RawMemory>)> = Vec::new();
            for r in ingestable {
                let key = ingest_group_key(&r.source);
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, rows)) => rows.push(r),
                    None => groups.push((key, vec![r])),
                }
            }

            // Marking policy (deliberate): each group's rows are marked
            // processed right after that group's ingest settles — per group,
            // not batch-at-end. On partial failure this keeps succeeded groups
            // from being re-ingested on the next tick (duplicate note pages,
            // doubled LLM spend); the failed group's rows stay unprocessed and
            // retry. The per-agent ingest lock is held across ALL groups, so
            // the read → ingest → mark window stays closed to concurrent
            // drains for the whole run.
            for (group_key, group) in groups {
                let ingest_outcome = ing
                    .ingest_batch(workspace_id, group.clone(), extra_context.as_deref())
                    .await;

                match ingest_outcome {
                    Ok(report) => {
                        // `linked` counts Link ops, each of which rewrites both
                        // endpoint notes on disk — real extraction work that
                        // used to report as zero because nothing read the field.
                        facts_extracted +=
                            report.created + report.appended + report.updated + report.linked;
                        facts_invalidated += report.contradicted + report.superseded;

                        // Stop-the-bleed: when this group's plan produced no
                        // notes AND the planner degraded, don't burn the
                        // knowledge. Defer marking rows still within the grace
                        // window so a transiently-failed extraction (flaky
                        // planner LLM / embedding outage) gets retried on a
                        // later tick instead of being discarded forever. Past
                        // the window even ingestable rows are marked, to bound
                        // the queue.
                        //
                        // The `planner_degraded` conjunct is what keeps this
                        // from punishing correct behaviour: every source prompt
                        // tells the planner to emit an empty plan when nothing
                        // clears the bar, and an un-noteworthy transcript will
                        // still be un-noteworthy six hours later — so a
                        // deliberate empty plan consumes its rows now instead of
                        // being re-planned on every tick until they age out.
                        let mut consumed: Vec<String> = Vec::with_capacity(group.len());
                        if report.is_empty() && report.planner_degraded {
                            const RETRY_GRACE_SECS: i64 = 6 * 3600;
                            let now = chrono::Utc::now().timestamp();
                            let mut deferred = 0usize;
                            for r in &group {
                                if now - r.created_at < RETRY_GRACE_SECS {
                                    deferred += 1;
                                } else {
                                    consumed.push(r.id.clone());
                                }
                            }
                            if deferred > 0 {
                                tracing::info!(
                                    group = %group_key,
                                    deferred,
                                    "compound ingest planner degraded; deferring \
                                     ingestable rows for retry (within grace window)"
                                );
                            }
                        } else {
                            consumed.extend(group.iter().map(|r| r.id.clone()));
                        }

                        if !consumed.is_empty() {
                            match self.database.mark_raw_as_processed(&consumed).await {
                                Ok(n) => {
                                    memories_processed += n as u32;
                                    tracing::info!(
                                        group = %group_key,
                                        marked = n,
                                        "Marked raw memories as processed (compound)"
                                    );
                                }
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    group = %group_key,
                                    "Failed to mark raw memories as processed"
                                ),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(group = %group_key, "compound ingest failed: {e}");
                        // SessionEnd groups are still marked processed even
                        // when the compound plan failed — ProfileSynthesizer
                        // (above) already consumed them. Other groups stay
                        // unprocessed and retry next tick; groups are
                        // independent, so keep draining the rest.
                        let is_session_end = group.first().is_some_and(|r| {
                            matches!(&r.source, RawMemorySource::SessionEnd { .. })
                        });
                        if is_session_end {
                            tracing::info!(
                                "compound ingest failed on SessionEnd group; \
                                 marking processed (ProfileSynthesizer already fired)"
                            );
                            let ids: Vec<String> = group.iter().map(|r| r.id.clone()).collect();
                            match self.database.mark_raw_as_processed(&ids).await {
                                Ok(n) => memories_processed += n as u32,
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    "Failed to mark SessionEnd raws as processed"
                                ),
                            }
                        }
                    }
                }
            }

            // Update compression timestamp.
            let latest_timestamp = raw_memories.iter().map(|r| r.created_at).max().unwrap_or(0);
            self.database
                .set_last_compression_timestamp(latest_timestamp)
                .await?;

            let duration_ms = start.elapsed().as_millis() as u64;
            tracing::info!(
                duration_ms,
                memories_processed,
                facts_extracted,
                facts_invalidated,
                "Compound ingest compression complete"
            );
            return Ok(CompressionResult {
                memories_processed,
                facts_extracted,
                facts_invalidated,
                duration_ms,
            });
        }

        // compound_enabled is always required; if ingestor is missing, warn and skip.
        tracing::warn!("compress_to_notes called without compound ingestor; skipping batch");
        Ok(CompressionResult::empty())
    }

    /// Check if compression should be triggered and execute if needed.
    ///
    /// Called from the task spawned by [`Self::record_turn_and_maybe_compress`].
    /// The re-check is not decorative: between the threshold crossing and the
    /// spawned task actually running, a concurrent `compress()` (background
    /// tick / RPC / flush) may have consumed the turn budget via
    /// `reset_turns()`, in which case this skips a redundant run.
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
    /// The scheduler-based triggers are handled separately via `record_turn_and_maybe_compress()`.
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

    /// Record a conversation turn and compress once the turn threshold is crossed.
    ///
    /// **Content-blind by design.** This used to also scan the user's message
    /// against a keyword table and compress immediately on a "correction" hit —
    /// deterministic content classification of natural language, which R7/P8
    /// forbid (and which mis-fired on any turn containing "actually", "wrong" or
    /// "should be"). Deciding *"was that a correction worth remembering?"* is the
    /// model's job: it calls `flag_user_correction`, and that tool kicks the drain
    /// itself. What remains here is pure cadence — count turns, compress at the
    /// threshold, look at nothing.
    ///
    /// Non-blocking: the actual compression runs in a spawned task.
    pub fn record_turn_and_maybe_compress(self: &Arc<Self>) {
        // Count the turn exactly once at the threshold crossing.
        let old_turns = self
            .scheduler
            .pending_turns
            .fetch_add(1, crate::sync_primitives::Ordering::AcqRel);
        let turns = old_turns + 1;
        let threshold = self.config.scheduler.turn_threshold;

        if old_turns < threshold && turns >= threshold {
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
                        "Compression completed (turn threshold)"
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

/// Grouping key for one `compress_to_notes` drain: rows whose sources map to
/// the same source-specific ingest prompt (see
/// [`crate::memory::compression::source_prompts::prompt_for`]) share a group.
/// Field payloads that do not affect prompt selection (e.g.
/// `Delegation::child_agent_id`, `Correction::severity`) are deliberately
/// ignored so a drain does not shatter into per-payload micro-batches;
/// `SessionEnd` splits by reason because Disconnect (DIGEST) and TaskDone
/// (RETRO) select different prompts.
fn ingest_group_key(source: &crate::memory::store::raw_memory::RawMemorySource) -> String {
    use crate::memory::store::raw_memory::RawMemorySource;
    match source {
        RawMemorySource::SessionEnd { reason } => format!("session_end/{reason:?}"),
        other => other.to_persisted().0.to_string(),
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

    /// Records every batch it is handed, and dawdles inside `ingest_batch` so the
    /// read-then-mark window is wide enough for a second caller to slip into.
    struct RecordingIngestor {
        ingested: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::memory::notes::ingest::CompoundIngestor for RecordingIngestor {
        async fn ingest_batch(
            &self,
            _agent_id: &str,
            raws: Vec<RawMemory>,
            _extra_context: Option<&str>,
        ) -> Result<crate::memory::notes::ingest::ApplyReport, AlephError> {
            // Stand in for the multi-second LLM ingest call.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let mut seen = self.ingested.lock().unwrap_or_else(|e| e.into_inner());
            seen.extend(raws.iter().map(|r| r.content.clone()));
            Ok(crate::memory::notes::ingest::ApplyReport::default())
        }
    }

    /// Two `compress_to_notes` calls for the SAME agent overlapping in time must
    /// ingest each unprocessed raw exactly once.
    ///
    /// Reachable in production from five unsynchronized sites on one shared
    /// `Arc<CompressionService>` (hourly background task, session-end flush,
    /// turn-threshold spawn, correction flush, `memory.compress` RPC). Without the
    /// per-agent ingest lock both callers run `WHERE is_processed = 0`, get the
    /// identical rows (neither has marked them yet — that only happens after the
    /// LLM call returns), and hand the same batch to the ingestor twice: duplicate
    /// note pages and doubled LLM spend.
    #[tokio::test]
    async fn concurrent_compress_for_one_agent_ingests_each_raw_exactly_once() {
        let temp_dir = tempdir().unwrap();
        let database: Arc<crate::memory::store::SqliteMemoryBackend> =
            Arc::new(crate::memory::store::SqliteMemoryBackend::new(temp_dir.path()).unwrap());

        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(
                1024,
                "mock-model",
            ),
        );

        let ingested = Arc::new(std::sync::Mutex::new(Vec::new()));
        let service = Arc::new(
            CompressionService::new(
                database.clone(),
                create_mock_provider(),
                embedder,
                CompressionConfig::default(),
            )
            .with_compound_ingestor(Arc::new(RecordingIngestor {
                ingested: Arc::clone(&ingested),
            })),
        );

        // One aged raw, so the stop-the-bleed grace window does not defer it.
        let mut raw = RawMemory::new(
            "user prefers dark mode".to_string(),
            RawMemorySource::Transcript,
        );
        raw.created_at = chrono::Utc::now().timestamp() - 7 * 3600;
        database.insert_raw_memory(&raw).await.unwrap();

        let (a, b) = tokio::join!(
            service.compress_to_notes("default"),
            service.compress_to_notes("default"),
        );
        a.expect("first compression succeeds");
        b.expect("second compression succeeds");

        let seen = ingested.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            seen.as_slice(),
            ["user prefers dark mode"],
            "the raw must reach the ingestor exactly once across two concurrent \
             compressions of the same agent (got {seen:?})"
        );
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
        let service = Arc::new(service);

        // Drive the PRODUCTION turn path (this used to call `record_turn`, a
        // wrapper the live gateway never went through).
        for _ in 0..5 {
            service.record_turn_and_maybe_compress();
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
                // Degraded planner, so the young transcript takes the deferral
                // branch below (a deliberate empty plan would consume it).
                Ok(ApplyReport {
                    planner_degraded: true,
                    ..Default::default()
                })
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

    /// Stop-the-bleed: a degraded planner over an ingestable row that has aged
    /// past the retry grace window must be marked processed (give-up), so the
    /// unprocessed queue stays bounded even when extraction keeps failing.
    #[tokio::test]
    async fn compress_to_notes_marks_aged_rows_when_planner_degraded() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};

        struct DegradedIngestor;
        #[async_trait::async_trait]
        impl CompoundIngestor for DegradedIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                Ok(ApplyReport {
                    planner_degraded: true,
                    ..Default::default()
                })
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let service = service.with_compound_ingestor(Arc::new(DegradedIngestor));

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

    /// Stop-the-bleed: a young ingestable row whose PLANNER degraded is
    /// deferred (left unprocessed) so a later tick can retry once the planner or
    /// embedding backend recovers — knowledge is not discarded on first failure.
    #[tokio::test]
    async fn compress_to_notes_defers_young_rows_when_planner_degraded() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};

        struct DegradedIngestor;
        #[async_trait::async_trait]
        impl CompoundIngestor for DegradedIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                Ok(ApplyReport {
                    planner_degraded: true,
                    ..Default::default()
                })
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let service = service.with_compound_ingestor(Arc::new(DegradedIngestor));

        let fresh = RawMemory::new("new conversation".to_string(), RawMemorySource::Transcript);
        database.insert_raw_memory(&fresh).await.unwrap();

        service.compress_to_notes("default").await.unwrap();

        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            1,
            "young ingestable rows must be deferred for retry, not burned"
        );
    }

    /// The mirror case, and the one the old code punished: the planner READ the
    /// batch and deliberately decided there was nothing worth writing down —
    /// exactly what every source prompt asks for ("if nothing clears the bar,
    /// emit an empty plan"). Those rows must be consumed immediately. Deferring
    /// them re-ran the same LLM call over the same un-noteworthy transcript on
    /// every tick for six hours before giving up on the identical answer.
    #[tokio::test]
    async fn compress_to_notes_consumes_young_rows_when_plan_deliberately_empty() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};
        use crate::sync_primitives::Mutex;

        struct DeliberatelyEmptyIngestor {
            calls: Mutex<usize>,
        }
        #[async_trait::async_trait]
        impl CompoundIngestor for DeliberatelyEmptyIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                *self.calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                // Parsed cleanly, planned nothing: NOT degraded.
                Ok(ApplyReport::default())
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let spy = Arc::new(DeliberatelyEmptyIngestor {
            calls: Mutex::new(0),
        });
        let service = service.with_compound_ingestor(spy.clone());

        let fresh = RawMemory::new(
            "what's the weather".to_string(),
            RawMemorySource::Transcript,
        );
        database.insert_raw_memory(&fresh).await.unwrap();

        service.compress_to_notes("default").await.unwrap();
        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            0,
            "a model-intended empty plan must consume its rows, not hold them for retry"
        );

        // And the second tick therefore costs nothing: no rows, no LLM call.
        service.compress_to_notes("default").await.unwrap();
        assert_eq!(
            *spy.calls.lock().unwrap_or_else(|e| e.into_inner()),
            1,
            "the same un-noteworthy batch must not be re-planned on the next tick"
        );
    }

    /// Regression (double ingest): a `Correction` row is owned by the
    /// `FeedbackDistill` dream stage, which reads it by path prefix
    /// (`aleph://correction/`) independently of `is_processed`. It must never
    /// also reach the note-extraction ingestor — otherwise every correction the
    /// model flags lands twice, as a generic note AND as a feedback rule. It
    /// must still be marked processed, so the unprocessed queue stays bounded.
    #[tokio::test]
    async fn compress_to_notes_leaves_corrections_for_feedback_distill() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};
        use crate::sync_primitives::Mutex;

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
                    .unwrap_or_else(|e| e.into_inner())
                    .extend(raws.iter().map(|r| r.source.clone()));
                Ok(ApplyReport::default())
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let spy = Arc::new(RecordingIngestor {
            seen: Mutex::new(vec![]),
        });
        let service = service.with_compound_ingestor(spy.clone());

        let correction = RawMemory::new(
            "never force-push to shared branches".to_string(),
            RawMemorySource::Correction {
                severity: "high".to_string(),
                suggested_rule: None,
            },
        )
        .with_path("aleph://correction/c1");
        database.insert_raw_memory(&correction).await.unwrap();

        service.compress_to_notes("default").await.unwrap();

        assert!(
            spy.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "corrections must not reach the note-extraction ingestor"
        );
        assert_eq!(
            database.count_unprocessed("default").await.unwrap(),
            0,
            "corrections must still be marked processed so the queue stays bounded"
        );
        // FeedbackDistill's reader is `is_processed`-independent, so marking
        // them does not strand them: the row is still there for the prefix read.
        let still_readable = database
            .get_raw_by_path_prefix("aleph://correction/", "default", 10)
            .await
            .unwrap();
        assert_eq!(
            still_readable.len(),
            1,
            "FeedbackDistill must still be able to read the correction after compression"
        );
    }

    /// Regression (mixed-source drain): the ingestor derives its per-source
    /// prompt from `raws[0].source`, so `compress_to_notes` must call
    /// `ingest_batch` once per source group — never hand it a mixed batch
    /// where Reflection/SessionEnd rows silently degrade to the first row's
    /// prompt.
    #[tokio::test]
    async fn compress_to_notes_splits_mixed_sources_into_per_source_batches() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};
        use crate::memory::store::raw_memory::SessionEndReason;
        use crate::sync_primitives::Mutex;

        // Records every ingest_batch call as its own vector of rows.
        struct BatchRecordingIngestor {
            calls: Mutex<Vec<Vec<RawMemory>>>,
        }
        #[async_trait::async_trait]
        impl CompoundIngestor for BatchRecordingIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                self.calls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(raws);
                Ok(ApplyReport::default())
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let spy = Arc::new(BatchRecordingIngestor {
            calls: Mutex::new(vec![]),
        });
        let service = service.with_compound_ingestor(spy.clone());

        // Aged rows (past the grace window) with interleaved sources; distinct
        // created_at values pin the fetch order (created_at ASC).
        let base = chrono::Utc::now().timestamp() - 7 * 3600;
        let mut rows = vec![
            RawMemory::new("t1".to_string(), RawMemorySource::Transcript),
            RawMemory::new("r1".to_string(), RawMemorySource::Reflection),
            RawMemory::new("t2".to_string(), RawMemorySource::Transcript),
            RawMemory::new(
                "s1".to_string(),
                RawMemorySource::SessionEnd {
                    reason: SessionEndReason::TaskDone,
                },
            ),
        ];
        for (i, r) in rows.iter_mut().enumerate() {
            r.created_at = base + i as i64;
            database.insert_raw_memory(r).await.unwrap();
        }

        let result = service.compress_to_notes("default").await.unwrap();

        let calls = spy.calls.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(calls.len(), 3, "one ingest call per source group");
        // Every call is homogeneous in prompt-affecting source identity.
        for call in &calls {
            let first = ingest_group_key(&call[0].source);
            assert!(
                call.iter().all(|r| ingest_group_key(&r.source) == first),
                "each ingest batch must be single-source (got {:?})",
                call.iter().map(|r| r.source.clone()).collect::<Vec<_>>()
            );
        }
        // First-seen group order and within-group fetch order both preserved.
        let contents: Vec<Vec<&str>> = calls
            .iter()
            .map(|c| c.iter().map(|r| r.content.as_str()).collect())
            .collect();
        assert_eq!(contents, vec![vec!["t1", "t2"], vec!["r1"], vec!["s1"]]);
        // Aged rows with an empty plan are all marked processed (per group).
        assert_eq!(result.memories_processed, 4);
        assert_eq!(database.count_unprocessed("default").await.unwrap(), 0);
    }

    /// `ApplyReport` counts must surface in `CompressionResult` so the
    /// `memory.compress` RPC and the background-task log stop reporting zeros.
    #[tokio::test]
    async fn compress_to_notes_backfills_apply_report_counts() {
        use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};

        struct CountingIngestor;
        #[async_trait::async_trait]
        impl CompoundIngestor for CountingIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<RawMemory>,
                _extra_context: Option<&str>,
            ) -> Result<ApplyReport, AlephError> {
                Ok(ApplyReport {
                    created: 2,
                    appended: 1,
                    updated: 1,
                    contradicted: 1,
                    superseded: 1,
                    linked: 3,
                    touched_paths: vec!["learning/tokio".to_string()],
                    ..Default::default()
                })
            }
        }

        let (service, database, _tmp) = create_test_service_with_tempdir().await;
        let service = service.with_compound_ingestor(Arc::new(CountingIngestor));

        let raw = RawMemory::new(
            "user prefers tokio".to_string(),
            RawMemorySource::Transcript,
        );
        database.insert_raw_memory(&raw).await.unwrap();

        let result = service.compress_to_notes("default").await.unwrap();

        assert_eq!(result.memories_processed, 1, "consumed raw rows counted");
        assert_eq!(
            result.facts_extracted, 7,
            "created + appended + updated notes AND link edges counted — `linked` was \
             a write-only counter nobody read, so 3 applied Link ops (each rewriting \
             both endpoints) reported as zero work"
        );
        assert_eq!(
            result.facts_invalidated, 2,
            "contradicted + superseded notes counted"
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

    /// The turn hook is content-blind: it counts turns and nothing else. It must
    /// not compress before the threshold no matter what the user typed — the old
    /// keyword detector fired an LLM ingest cycle on any message containing
    /// "actually" / "not right" / "should be". Deciding that a message is a correction is
    /// the model's job (`flag_user_correction`), not a substring table's.
    #[tokio::test]
    async fn turn_hook_is_content_blind_and_only_fires_at_the_threshold() {
        let (service, _db) = create_test_service().await;
        let service = Arc::new(service);
        let threshold = service.config.scheduler.turn_threshold;

        // Messages that the deleted keyword table would each have classified as an
        // immediate "correction".
        for _ in 0..(threshold - 1) {
            service.record_turn_and_maybe_compress();
        }

        assert_eq!(
            service.get_scheduler().get_pending_turns(),
            threshold - 1,
            "turns accumulate; nothing compresses before the threshold regardless of content"
        );
    }
}
