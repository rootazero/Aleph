//! Session Compactor — intra-session context management.
//!
//! Prevents token overflow in long conversations by summarizing tool call
//! sequences and older turns, while preserving a fresh tail of recent
//! messages verbatim.
//!
//! The [`SessionCompactor`] orchestrates the full lifecycle:
//! - **`prepare_history()`** — assembles compressed history + fresh tail for the
//!   agent loop, injecting prior summaries as `<session_context>` XML blocks.
//! - **`post_turn_compress()`** — runs asynchronously after each agent turn to
//!   chunk compressible messages, generate d0 summaries, and trigger hierarchical
//!   condensation (d0→d1→d2) when fanout thresholds are met.

use std::sync::atomic::{AtomicU64, Ordering};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::AlephError;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::router::SessionKey;
use crate::memory::context::{NoteType, MemoryFact, MemoryScope};
use crate::memory::store::types::SearchFilter;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

pub mod context_window;
pub mod fallback;
pub mod summary_engine;
pub mod tool_compactor;

use crate::agent_loop::compaction::tool_aware_chunker::{parse_semantic_units, ToolAwareChunker};
use context_window::{estimate_tokens, partition_fresh_tail_pairs};
use fallback::{deterministic_truncate, FallbackLevel};
use summary_engine::summary_to_fact;

// ---------------------------------------------------------------------------
// CompactorMetrics
// ---------------------------------------------------------------------------

/// Atomic counters for observing session compactor activity.
///
/// All fields use `Relaxed` ordering — these are best-effort counters with no
/// cross-thread ordering requirements.
#[derive(Debug, Default)]
pub struct CompactorMetrics {
    /// Number of d0 (leaf) summaries created.
    pub tool_compactions: AtomicU64,
    /// Number of d0 facts stored successfully.
    pub d0_summaries_created: AtomicU64,
    /// Number of d0→d1 condensations performed.
    pub d1_condensations: AtomicU64,
    /// Number of d1→d2 condensations performed.
    pub d2_condensations: AtomicU64,
    /// Number of times deterministic fallback was used for summary generation.
    pub fallback_count: AtomicU64,
    /// Number of `prepare_history` calls.
    pub prepare_history_calls: AtomicU64,
}

// ---------------------------------------------------------------------------
// SessionCompactorConfig
// ---------------------------------------------------------------------------

/// Configuration for the SessionCompactor subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCompactorConfig {
    /// Enable or disable the compactor entirely.
    pub enabled: bool,
    /// Number of most-recent turns to keep verbatim (never compressed).
    pub fresh_tail_count: usize,
    /// Fraction of the context window at which compaction is triggered (0.0–1.0).
    pub context_threshold: f64,
    /// Target token count per leaf chunk when building the summary tree.
    pub leaf_chunk_tokens: usize,
    /// Minimum number of leaf chunks to merge at depth-1.
    pub d1_min_fanout: usize,
    /// Minimum number of depth-1 nodes to merge at depth-2.
    pub d2_min_fanout: usize,
    /// Maximum recursion depth for hierarchical summarization.
    pub max_summary_depth: u32,
    /// Characters-per-token ratio used for token estimation.
    pub token_estimate_ratio: f64,
    /// Hours to retain session-local facts before expiry.
    pub session_fact_retention_hours: u64,
    /// Minimum confidence required to promote a session-local fact to long-term memory.
    pub promote_confidence_threshold: f32,
}

impl Default for SessionCompactorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fresh_tail_count: 20,
            context_threshold: 0.75,
            leaf_chunk_tokens: 1000,
            d1_min_fanout: 4,
            d2_min_fanout: 3,
            max_summary_depth: 2,
            token_estimate_ratio: 3.5,
            session_fact_retention_hours: 24,
            promote_confidence_threshold: 0.8,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionCompactor
// ---------------------------------------------------------------------------

/// Central orchestrator for intra-session context compression.
///
/// Ties together [`context_window`], [`summary_engine`], and [`fallback`]
/// to keep long conversations within the model's token budget.
pub struct SessionCompactor {
    database: MemoryBackend,
    provider: Option<Arc<dyn AiProvider>>,
    config: SessionCompactorConfig,
    metrics: Arc<CompactorMetrics>,
    indexer: Option<crate::memory::transcript_indexer::TranscriptIndexer>,
}

impl SessionCompactor {
    /// Create a new SessionCompactor with the given storage backend and config.
    pub fn new(database: MemoryBackend, config: SessionCompactorConfig) -> Self {
        Self {
            database,
            provider: None,
            config,
            metrics: Arc::new(CompactorMetrics::default()),
            indexer: None,
        }
    }

    /// Return a reference to the compactor metrics.
    pub fn metrics(&self) -> &Arc<CompactorMetrics> {
        &self.metrics
    }

    /// Attach an AI provider for LLM-based summary generation.
    ///
    /// When set, summaries use a three-level fallback strategy:
    /// 1. Normal LLM summarization
    /// 2. Aggressive LLM summarization (shorter target)
    /// 3. Deterministic truncation (no LLM)
    ///
    /// When absent, all summaries use deterministic truncation only.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Attach an embedding provider for transcript indexing.
    ///
    /// When set, each compressed turn is also embedded and stored as a
    /// `NoteType::Transcript` fact for direct conversation retrieval.
    pub fn with_embedder(mut self, embedder: Arc<dyn crate::memory::EmbeddingProvider>) -> Self {
        self.indexer = Some(crate::memory::transcript_indexer::TranscriptIndexer::new(
            self.database.clone(),
            embedder,
        ));
        self
    }

    /// Return a reference to the compactor configuration.
    pub fn config(&self) -> &SessionCompactorConfig {
        &self.config
    }

    // -----------------------------------------------------------------------
    // prepare_history — assemble compressed context for the agent loop
    // -----------------------------------------------------------------------

    /// Assemble compressed history for a new agent loop turn.
    ///
    /// 1. Query SQLite for existing session summaries (scope=SessionLocal,
    ///    path prefix `aleph://session/{session_id}/`, is_valid=true).
    /// 2. Fetch fresh tail raw messages from the agent's session store.
    /// 3. Inject summaries as user-role messages wrapped in `<session_context>` XML.
    /// 4. Evict oldest low-depth summaries if the assembled context exceeds budget.
    pub async fn prepare_history(
        &self,
        agent: &AgentInstance,
        session_key: &SessionKey,
        _current_input: &str,
        token_budget: u64,
    ) -> Vec<UnifiedMessage> {
        self.metrics
            .prepare_history_calls
            .fetch_add(1, Ordering::Relaxed);
        tracing::info!(target: "session_compactor", "prepare");

        if !self.config.enabled {
            // Disabled: return raw history from the agent (last N messages).
            let raw = agent.get_history(session_key, None).await;
            return raw
                .into_iter()
                .map(|m| session_message_to_unified(&m))
                .collect();
        }

        // Short session: skip LanceDB query when there's nothing to compress
        let raw_messages = agent.get_history(session_key, None).await;
        if raw_messages.len() <= self.config.fresh_tail_count {
            return raw_messages
                .iter()
                .map(session_message_to_unified)
                .collect();
        }

        let session_id = session_key.to_key_string();
        let path_prefix = format!("aleph://session/{}/", session_id);

        // Step 1: fetch existing summaries from LanceDB
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_scope(MemoryScope::SessionLocal)
            .with_path_prefix(&path_prefix);

        let mut summaries = match self
            .database
            .get_facts_by_path_prefix(&path_prefix, &filter, 200)
            .await
        {
            Ok(facts) => facts,
            Err(e) => {
                warn!(
                    error = %e,
                    session = %session_id,
                    "Failed to fetch session summaries, falling back to raw history"
                );
                let raw = agent.get_history(session_key, None).await;
                return raw
                    .into_iter()
                    .map(|m| session_message_to_unified(&m))
                    .collect();
            }
        };

        // Sort summaries: highest depth first (d2 before d1 before d0),
        // then by path (which includes sequence number) for deterministic ordering.
        summaries.sort_by(|a, b| {
            let da = extract_depth(&a.path);
            let db = extract_depth(&b.path);
            db.cmp(&da).then_with(|| a.path.cmp(&b.path))
        });

        // Step 2: fetch fresh tail from in-memory session store
        let tail_start = if raw_messages.len() > self.config.fresh_tail_count {
            raw_messages.len() - self.config.fresh_tail_count
        } else {
            0
        };
        let fresh_tail: Vec<UnifiedMessage> = raw_messages[tail_start..]
            .iter()
            .map(session_message_to_unified)
            .collect();

        // Step 3: assemble — summaries first, then fresh tail
        let mut result: Vec<UnifiedMessage> = Vec::new();
        let ratio = self.config.token_estimate_ratio;
        let budget = token_budget as usize;
        let mut used_tokens: usize = 0;

        // Reserve tokens for fresh tail
        let tail_tokens: usize = fresh_tail
            .iter()
            .map(|m| estimate_tokens(&m.text_content(), ratio))
            .sum();

        let summary_budget = budget.saturating_sub(tail_tokens);

        // Inject summaries as user-role <session_context> messages
        for fact in &summaries {
            let depth = extract_depth(&fact.path);
            let summary_tokens = estimate_tokens(&fact.content, ratio);

            if used_tokens + summary_tokens > summary_budget {
                // Over budget — evict remaining (lower-depth) summaries
                debug!(
                    session = %session_id,
                    used_tokens,
                    summary_budget,
                    "Summary token budget exhausted, evicting remaining summaries"
                );
                break;
            }

            let xml_content = format!(
                "<session_context depth=\"d{depth}\">\n{}\n</session_context>",
                fact.content
            );
            result.push(UnifiedMessage::user(xml_content));
            used_tokens += summary_tokens;
        }

        // Append fresh tail
        result.extend(fresh_tail);

        debug!(
            session = %session_id,
            summary_count = summaries.len(),
            injected = result.len().saturating_sub(raw_messages[tail_start..].len()),
            tail_count = raw_messages[tail_start..].len(),
            total_tokens_est = used_tokens + tail_tokens,
            "Prepared session history"
        );

        result
    }

    // -----------------------------------------------------------------------
    // post_turn_compress — async compression after agent loop
    // -----------------------------------------------------------------------

    /// Run compression after an agent loop turn completes.
    ///
    /// 1. Partition raw messages — skip fresh tail.
    /// 2. Chunk compressible messages into `leaf_chunk_tokens`-sized groups.
    /// 3. Generate d0 summaries for each chunk (deterministic fallback).
    /// 4. Check condensation triggers (d0→d1 when count ≥ d1_min_fanout,
    ///    d1→d2 when count ≥ d2_min_fanout).
    /// 5. Store new facts in SQLite and invalidate source facts on condensation.
    pub async fn post_turn_compress(
        &self,
        agent: &AgentInstance,
        session_key: &SessionKey,
    ) -> Result<CompressResult, AlephError> {
        if !self.config.enabled {
            return Ok(CompressResult::default());
        }

        let session_id = session_key.to_key_string();
        tracing::info!(target: "session_compactor", session = %session_id, "compress_start");
        let agent_id = agent.id().to_string();
        let ratio = self.config.token_estimate_ratio;

        // Fetch all raw messages
        let raw_messages = agent.get_history(session_key, None).await;

        // Convert to (role, content) pairs for chunking
        let pairs: Vec<(String, String)> = raw_messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    crate::gateway::agent_instance::MessageRole::User => "user",
                    crate::gateway::agent_instance::MessageRole::Assistant => "assistant",
                    crate::gateway::agent_instance::MessageRole::System => "system",
                    crate::gateway::agent_instance::MessageRole::Tool => "tool",
                };
                (role.to_string(), m.content.clone())
            })
            .collect();

        // Partition: [0..split] = compressible, [split..] = fresh tail
        let split = partition_fresh_tail_pairs(&pairs, self.config.fresh_tail_count);
        if split == 0 {
            // Not enough messages to compress
            return Ok(CompressResult::default());
        }

        let compressible = &pairs[..split];

        // Convert compressible (role, content) pairs to UnifiedMessage for semantic parsing.
        // We use a best-effort conversion: "assistant" role → Assistant message,
        // everything else → User message.  The ToolAwareChunker only needs the
        // content for token estimation and the structural role for grouping.
        let compressible_messages: Vec<crate::providers::message::UnifiedMessage> = compressible
            .iter()
            .map(|(role, content)| {
                if role == "assistant" {
                    crate::providers::message::UnifiedMessage::assistant(content)
                } else {
                    crate::providers::message::UnifiedMessage::user(content)
                }
            })
            .collect();

        // Parse into semantic units respecting tool_use/tool_result boundaries.
        let units = parse_semantic_units(&compressible_messages);
        let chunker = ToolAwareChunker::new(self.config.leaf_chunk_tokens, ratio);
        // Pass compressible_messages.len() as fresh_start so ALL units are chunked
        // (the fresh tail was already excluded by the split above).
        let semantic_chunks =
            chunker.chunk(&units, &compressible_messages, compressible_messages.len());

        if semantic_chunks.is_empty() {
            return Ok(CompressResult::default());
        }

        // Count existing d0 facts to determine next sequence number
        let existing_d0 = self.count_valid_facts_at_depth(&session_id, 0).await?;
        let mut next_seq = existing_d0.min(u32::MAX as usize) as u32;
        let mut d0_created = 0u32;

        // Generate d0 summaries for each semantic chunk.
        for semantic_chunk in &semantic_chunks {
            // Reconstruct (role, content) pairs from the chunk's message indices.
            let chunk: Vec<(String, String)> = semantic_chunk
                .message_indices()
                .into_iter()
                .filter_map(|idx| compressible.get(idx).cloned())
                .collect();

            if chunk.is_empty() {
                continue;
            }

            let summary_text = self.generate_summary(&chunk, 0, None).await;
            let source_tokens: usize = chunk.iter().map(|(_, c)| estimate_tokens(c, ratio)).sum();

            let fact = summary_to_fact(
                &session_id,
                0,
                next_seq,
                summary_text,
                chunk.len(),
                source_tokens,
                &agent_id,
            );

            if let Err(e) = self.database.insert_fact(&fact).await {
                warn!(
                    error = %e,
                    session = %session_id,
                    seq = next_seq,
                    "Failed to store d0 summary"
                );
            } else {
                d0_created += 1;
                self.metrics
                    .d0_summaries_created
                    .fetch_add(1, Ordering::Relaxed);

                // Also store raw chunk content for post-compression semantic recovery.
                // This allows recall_context to search pre-compression conversation text
                // at aleph://session/{id}/raw/{seq}.
                let raw_content: String = chunk
                    .iter()
                    .map(|(role, text)| format!("[{}]: {}", role, text))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                self.store_raw_chunk(&session_id, next_seq as usize, &raw_content)
                    .await?;

                // Index raw content as transcript for direct retrieval
                if let Some(ref indexer) = self.indexer {
                    indexer.index_turn_text(
                        &session_id,
                        next_seq,
                        &raw_content,
                        "", // AI output already included in raw_content
                        "owner",
                        &agent_id,
                    ).await;
                }
            }

            next_seq += 1;
        }

        info!(
            session = %session_id,
            d0_created,
            chunks = semantic_chunks.len(),
            compressible_messages = compressible.len(),
            "Post-turn compression: d0 summaries generated"
        );

        // Check condensation: d0 → d1
        let mut d1_created = 0u32;
        let total_d0 = self.count_valid_facts_at_depth(&session_id, 0).await?;
        if total_d0 >= self.config.d1_min_fanout {
            d1_created = self
                .try_condense(&session_id, &agent_id, 0, 1, self.config.d1_min_fanout)
                .await?;
            if d1_created > 0 {
                self.metrics
                    .d1_condensations
                    .fetch_add(d1_created as u64, Ordering::Relaxed);
            }
        }

        // Check condensation: d1 → d2
        let mut d2_created = 0u32;
        if self.config.max_summary_depth >= 2 {
            let total_d1 = self.count_valid_facts_at_depth(&session_id, 1).await?;
            if total_d1 >= self.config.d2_min_fanout {
                d2_created = self
                    .try_condense(&session_id, &agent_id, 1, 2, self.config.d2_min_fanout)
                    .await?;
                if d2_created > 0 {
                    self.metrics
                        .d2_condensations
                        .fetch_add(d2_created as u64, Ordering::Relaxed);
                }
            }
        }

        Ok(CompressResult {
            d0_created,
            d1_created,
            d2_created,
        })
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Generate a summary for the given messages at the specified depth.
    ///
    /// Uses a three-level fallback strategy when an AI provider is available:
    /// 1. **Normal** — LLM summarization at ~35% compression ratio
    /// 2. **Aggressive** — LLM summarization at ~20% compression ratio
    /// 3. **Deterministic** — rule-based first-sentence extraction
    ///
    /// Falls back to the next level when the LLM call fails or when the
    /// output is not shorter than the input (which would defeat the purpose
    /// of compression).
    async fn generate_summary(
        &self,
        messages: &[(String, String)],
        depth: u32,
        previous_context: Option<&str>,
    ) -> String {
        let ratio = self.config.token_estimate_ratio;
        let source_token_count: usize = messages
            .iter()
            .map(|(_, c)| estimate_tokens(c, ratio))
            .sum();

        if let Some(ref provider) = self.provider {
            // Level 1: Normal LLM summarization
            let prompt = summary_engine::build_summary_prompt(
                messages,
                depth,
                previous_context,
                FallbackLevel::Normal,
            );
            match self.call_llm(provider.as_ref(), &prompt).await {
                Ok(text)
                    if !text.is_empty() && estimate_tokens(&text, ratio) < source_token_count =>
                {
                    return summary_engine::strip_analysis_block(&text);
                }
                Ok(_) => {
                    warn!(
                        depth,
                        source_tokens = source_token_count,
                        "LLM summary (normal) was empty or not shorter than input, escalating to aggressive"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        depth,
                        "LLM summary (normal) failed, escalating to aggressive"
                    );
                }
            }

            // Level 2: Aggressive LLM summarization
            let prompt = summary_engine::build_summary_prompt(
                messages,
                depth,
                previous_context,
                FallbackLevel::Aggressive,
            );
            match self.call_llm(provider.as_ref(), &prompt).await {
                Ok(text)
                    if !text.is_empty() && estimate_tokens(&text, ratio) < source_token_count =>
                {
                    return summary_engine::strip_analysis_block(&text);
                }
                Ok(_) => {
                    warn!(
                        depth,
                        source_tokens = source_token_count,
                        "LLM summary (aggressive) was empty or not shorter than input, falling back to deterministic"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        depth,
                        "LLM summary (aggressive) failed, falling back to deterministic"
                    );
                }
            }

            // Level 3: Deterministic fallback
            warn!(depth, "Using deterministic fallback for summary generation");
        }

        // No provider or all LLM attempts failed — deterministic fallback
        self.metrics.fallback_count.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(target: "session_compactor", depth, "fallback");
        let target = fallback::target_tokens(source_token_count, FallbackLevel::Normal);
        let max_chars = (target as f64 * ratio) as usize;
        deterministic_truncate(messages, max_chars)
    }

    /// Call the LLM provider with a summarization prompt.
    async fn call_llm(
        &self,
        provider: &dyn AiProvider,
        prompt: &str,
    ) -> crate::error::Result<String> {
        let msgs = [UnifiedMessage::user(prompt)];
        let system = "You are a precise summarizer. Output only the summary, no preamble or meta-commentary.";
        let payload = RequestPayload::new(&msgs).with_system(Some(system));
        let response = provider.process(payload).await?;
        Ok(response.text_content())
    }

    /// Count valid facts at a given depth for a session.
    async fn count_valid_facts_at_depth(
        &self,
        session_id: &str,
        depth: u32,
    ) -> Result<usize, AlephError> {
        let path_prefix = format!("aleph://session/{}/d{}/", session_id, depth);
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_scope(MemoryScope::SessionLocal)
            .with_path_prefix(&path_prefix);
        self.database.count_facts(&filter).await
    }

    /// Store raw conversation chunk for post-compression semantic recovery.
    ///
    /// Persists the verbatim (role-prefixed) content of a chunk at
    /// `aleph://session/{session_id}/raw/{seq}` so that `recall_context`
    /// can search pre-compression conversation content even after summaries
    /// have replaced the originals.
    pub async fn store_raw_chunk(
        &self,
        session_id: &str,
        seq: usize,
        content: &str,
    ) -> Result<(), AlephError> {
        let path = format!("aleph://session/{}/raw/{}", session_id, seq);
        let fact = MemoryFact::new(content.to_string(), NoteType::Other, Vec::new())
            .with_fact_source(crate::memory::context::FactSource::SessionCompressed)
            .with_path(path)
            .with_scope(MemoryScope::SessionLocal)
            .with_confidence(1.0);
        if let Err(e) = self.database.insert_fact(&fact).await {
            warn!(
                error = %e,
                session = %session_id,
                seq,
                "Failed to store raw chunk"
            );
        }
        Ok(())
    }

    /// Fetch all valid facts at a given depth for a session.
    async fn fetch_valid_facts_at_depth(
        &self,
        session_id: &str,
        depth: u32,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let path_prefix = format!("aleph://session/{}/d{}/", session_id, depth);
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_scope(MemoryScope::SessionLocal)
            .with_path_prefix(&path_prefix);
        self.database
            .get_facts_by_path_prefix(&path_prefix, &filter, 500)
            .await
    }

    /// Try to condense facts from `source_depth` into `target_depth`.
    ///
    /// If source count ≥ min_fanout:
    /// 1. Fetch all valid source facts
    /// 2. Build messages from their content
    /// 3. Generate a target-depth summary
    /// 4. Store the new fact
    /// 5. Invalidate all source facts
    async fn try_condense(
        &self,
        session_id: &str,
        agent_id: &str,
        source_depth: u32,
        target_depth: u32,
        _min_fanout: usize,
    ) -> Result<u32, AlephError> {
        tracing::info!(target: "session_compactor", source_depth, target_depth, "condense");

        let source_facts = self
            .fetch_valid_facts_at_depth(session_id, source_depth)
            .await?;

        if source_facts.is_empty() {
            return Ok(0);
        }

        // Build messages from source fact content
        let messages: Vec<(String, String)> = source_facts
            .iter()
            .map(|f| ("assistant".to_string(), f.content.clone()))
            .collect();

        // Generate condensed summary — for condensation (d0→d1, d1→d2) we can
        // pass the source facts' content as "previous context" since they are
        // themselves summaries from the previous depth level.
        let summary_text = self.generate_summary(&messages, target_depth, None).await;

        // Determine sequence number for the new target fact
        let existing_target = self
            .count_valid_facts_at_depth(session_id, target_depth)
            .await?;
        let seq = existing_target.min(u32::MAX as usize) as u32;

        let source_tokens: usize = messages
            .iter()
            .map(|(_, c)| estimate_tokens(c, self.config.token_estimate_ratio))
            .sum();

        let fact = summary_to_fact(
            session_id,
            target_depth,
            seq,
            summary_text,
            source_facts.len(),
            source_tokens,
            agent_id,
        );

        self.database.insert_fact(&fact).await?;

        // Invalidate source facts
        let reason = format!("condensed into d{target_depth}/{seq}");
        for source in &source_facts {
            if let Err(e) = self.database.invalidate_fact(&source.id, &reason).await {
                warn!(
                    error = %e,
                    fact_id = %source.id,
                    "Failed to invalidate source fact during condensation"
                );
            }
        }

        info!(
            session = %session_id,
            source_depth,
            target_depth,
            source_count = source_facts.len(),
            "Condensed d{source_depth} → d{target_depth}"
        );

        Ok(1)
    }
}

// ---------------------------------------------------------------------------
// CompressResult
// ---------------------------------------------------------------------------

/// Summary of what `post_turn_compress` produced.
#[derive(Debug, Clone, Default)]
pub struct CompressResult {
    /// Number of d0 (leaf) summaries created.
    pub d0_created: u32,
    /// Number of d1 summaries created (condensed from d0).
    pub d1_created: u32,
    /// Number of d2 summaries created (condensed from d1).
    pub d2_created: u32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the depth level from a session compactor VFS path.
///
/// Path format: `aleph://session/{id}/d{depth}/{seq}`
fn extract_depth(path: &str) -> u32 {
    path.split("/d")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Convert an [`AgentInstance`] session message to a [`UnifiedMessage`].
fn session_message_to_unified(
    msg: &crate::gateway::agent_instance::SessionMessage,
) -> UnifiedMessage {
    use crate::gateway::agent_instance::MessageRole;
    match msg.role {
        MessageRole::User => UnifiedMessage::user(&msg.content),
        MessageRole::Assistant => UnifiedMessage::assistant(&msg.content),
        MessageRole::System => UnifiedMessage::user(format!("[system] {}", msg.content)),
        MessageRole::Tool => UnifiedMessage::user(format!("[tool] {}", msg.content)),
    }
}

// ---------------------------------------------------------------------------
// CompactionStrategy impl
// ---------------------------------------------------------------------------

use crate::agent_loop::compaction::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};
use std::future::Future;
use std::pin::Pin;

impl CompactionStrategy for SessionCompactor {
    fn name(&self) -> &str {
        "session_compactor"
    }

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate {
        let total = ctx.pressure.used_tokens;
        let fresh_ratio = ctx.fresh_tail_count as f64 / ctx.messages.len().max(1) as f64;
        let compressible = (total as f64 * (1.0 - fresh_ratio)) as usize;
        TokenEstimate {
            estimated_savings: (compressible as f64 * 0.65) as usize,
            confidence: 0.6,
        }
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
        Box::pin(async move {
            let before = ctx.pressure.ratio;
            // Placeholder: actual integration with post_turn_compress requires AgentInstance
            // and SessionKey which are not available in CompactionContext.
            // Full integration comes in Task 11/12.
            Ok(CompactionResult {
                freed_tokens: 0,
                compacted_count: 0,
                strategy_name: self.name().to_string(),
                pressure_before: before,
                pressure_after: before,
            })
        })
    }

    fn is_applicable(&self, ctx: &CompactionContext) -> bool {
        ctx.pressure_level >= PressureLevel::Warning && self.config.enabled
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_depth_d0() {
        assert_eq!(extract_depth("aleph://session/abc/d0/3"), 0);
    }

    #[test]
    fn extract_depth_d1() {
        assert_eq!(extract_depth("aleph://session/abc/d1/0"), 1);
    }

    #[test]
    fn extract_depth_d2() {
        assert_eq!(extract_depth("aleph://session/abc/d2/1"), 2);
    }

    #[test]
    fn extract_depth_missing_returns_zero() {
        assert_eq!(extract_depth("aleph://user/preferences/"), 0);
    }

    #[test]
    fn extract_depth_complex_session_id() {
        // Session IDs can contain colons (e.g., "agent:main:main")
        assert_eq!(extract_depth("aleph://session/agent:main:main/d1/5"), 1);
    }

    #[test]
    fn compress_result_default_is_zero() {
        let r = CompressResult::default();
        assert_eq!(r.d0_created, 0);
        assert_eq!(r.d1_created, 0);
        assert_eq!(r.d2_created, 0);
    }
}

// ---------------------------------------------------------------------------
// Chunker integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod chunker_integration_tests {
    use crate::agent_loop::compaction::tool_aware_chunker::{
        parse_semantic_units, SemanticUnit, ToolAwareChunker,
    };
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    use serde_json::json;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn user_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::user(text)
    }

    fn assistant_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::assistant(text)
    }

    fn tool_use_msg(call_id: &str, tool_name: &str) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: call_id.to_owned(),
                name: tool_name.to_owned(),
                arguments: json!({}),
            }],
        }
    }

    fn tool_result_msg(call_id: &str, tool_name: &str, output: &str) -> UnifiedMessage {
        UnifiedMessage::tool_result(call_id, tool_name, output, false)
    }

    // ── tests ─────────────────────────────────────────────────────────────────

    /// parse_semantic_units groups tool_use + tool_result into a single ToolRound,
    /// so they are never split across d0 summary chunks.
    #[test]
    fn tool_use_and_result_form_single_unit() {
        let messages = vec![
            user_msg("Please search for something"),
            tool_use_msg("c1", "web_search"),
            tool_result_msg("c1", "web_search", "Found 10 results"),
            assistant_msg("Here are the results"),
        ];

        let units = parse_semantic_units(&messages);
        // Expect: UserMessage(0) + ToolRound(1,2,follow_up=3) → 2 units
        assert_eq!(units.len(), 2, "expected 2 semantic units, got {units:?}");
        assert!(matches!(units[0], SemanticUnit::UserMessage { index: 0 }));
        match &units[1] {
            SemanticUnit::ToolRound {
                tool_use_index,
                tool_result_index,
                follow_up_index,
                tool_name,
            } => {
                assert_eq!(*tool_use_index, 1);
                assert_eq!(*tool_result_index, 2);
                assert_eq!(*follow_up_index, Some(3));
                assert_eq!(tool_name, "web_search");
            }
            other => panic!("expected ToolRound, got {other:?}"),
        }
    }

    /// A ToolRound that exceeds the chunk token limit is placed in its own chunk,
    /// never fragmented across multiple chunks.
    #[test]
    fn tool_round_never_split_even_when_oversized() {
        let large_output = "y".repeat(500); // well above a 10-token limit
        let messages = vec![
            user_msg("Do something"),
            tool_use_msg("c2", "big_tool"),
            tool_result_msg("c2", "big_tool", &large_output),
            assistant_msg("Done."),
        ];

        let units = parse_semantic_units(&messages);
        let chunker = ToolAwareChunker::new(10, 4.0); // very tight limit
        let chunks = chunker.chunk(&units, &messages, messages.len());

        // Every ToolRound's indices must all appear in the same chunk.
        for chunk in &chunks {
            for unit in &chunk.units {
                if let SemanticUnit::ToolRound {
                    tool_use_index,
                    tool_result_index,
                    follow_up_index,
                    ..
                } = unit
                {
                    let idx = chunk.message_indices();
                    assert!(idx.contains(tool_use_index), "tool_use_index split");
                    assert!(idx.contains(tool_result_index), "tool_result_index split");
                    if let Some(fu) = follow_up_index {
                        assert!(idx.contains(fu), "follow_up_index split");
                    }
                }
            }
        }
    }

    /// Simulate the compressible-messages conversion used in post_turn_compress:
    /// (role, content) pairs → UnifiedMessage → parse_semantic_units.
    /// Verifies the conversion preserves message count and role mapping.
    #[test]
    fn pair_to_unified_message_conversion_preserves_structure() {
        let pairs: Vec<(String, String)> = vec![
            ("user".to_string(), "Hello".to_string()),
            ("assistant".to_string(), "Hi there".to_string()),
            ("user".to_string(), "Tell me more".to_string()),
        ];

        let unified: Vec<UnifiedMessage> = pairs
            .iter()
            .map(|(role, content)| {
                if role == "assistant" {
                    UnifiedMessage::assistant(content)
                } else {
                    UnifiedMessage::user(content)
                }
            })
            .collect();

        let units = parse_semantic_units(&unified);
        // All three messages are plain (no tool calls), so 3 semantic units.
        assert_eq!(units.len(), 3);
        assert!(matches!(units[0], SemanticUnit::UserMessage { .. }));
        assert!(matches!(units[1], SemanticUnit::AssistantText { .. }));
        assert!(matches!(units[2], SemanticUnit::UserMessage { .. }));
    }

    /// Chunk indices produced by ToolAwareChunker correctly map back to the
    /// original (role, content) pairs — no index out-of-bounds.
    #[test]
    fn chunk_indices_map_back_to_pairs() {
        let pairs: Vec<(String, String)> = (0..6)
            .map(|i| ("user".to_string(), format!("message {i}")))
            .collect();

        let unified: Vec<UnifiedMessage> =
            pairs.iter().map(|(_, c)| UnifiedMessage::user(c)).collect();

        let units = parse_semantic_units(&unified);
        let chunker = ToolAwareChunker::new(20, 4.0);
        let chunks = chunker.chunk(&units, &unified, unified.len());

        // Every index returned by a chunk must be a valid index into pairs.
        for chunk in &chunks {
            for idx in chunk.message_indices() {
                assert!(
                    pairs.get(idx).is_some(),
                    "chunk index {idx} out of bounds for pairs of len {}",
                    pairs.len()
                );
            }
        }
    }
}
