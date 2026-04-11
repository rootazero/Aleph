//! Fact-First Retrieval
//!
//! Retrieves relevant context with priority given to compressed facts
//! over raw memories. Falls back to raw memories when facts are insufficient.

use serde::{Deserialize, Serialize};

use crate::error::AlephError;
use crate::memory::query_expander;
use crate::memory::context::MemoryFact;
use crate::memory::hybrid_retrieval::HybridSearchConfig;
use crate::memory::namespace::NamespaceScope;
use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::ScoringPipeline;
use crate::memory::store::types::{ScoredFact, SearchFilter};
use crate::memory::rerank::{build_provider, blend_scores, RerankConfig};
use crate::memory::store::{HybridSearchParams, MemoryBackend, MemoryStore};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

/// Configuration for fact retrieval
#[derive(Debug, Clone)]
pub struct FactRetrievalConfig {
    /// Maximum number of facts to retrieve
    pub max_facts: u32,
    /// Maximum number of raw memories to use as fallback
    pub max_raw_fallback: u32,
    /// Minimum similarity threshold for facts
    pub similarity_threshold: f32,
}

impl Default for FactRetrievalConfig {
    fn default() -> Self {
        Self {
            max_facts: 5,
            max_raw_fallback: 3,
            similarity_threshold: 0.3,
        }
    }
}

/// Result of a retrieval operation
#[derive(Debug, Clone, Default)]
pub struct RetrievalResult {
    /// Retrieved compressed facts
    pub facts: Vec<MemoryFact>,
}

impl RetrievalResult {
    /// Check if result is empty
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// Total number of retrieved items
    pub fn len(&self) -> usize {
        self.facts.len()
    }
}

/// Fact-first retrieval service
///
/// Uses hybrid search (vector + BM25 RRF fusion) with the full
/// [`ScoringPipeline`] applied.  Falls back to pure vector search
/// if the hybrid path encounters an error.
pub struct FactRetrieval {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: FactRetrievalConfig,
    hybrid_config: HybridSearchConfig,
    scoring_pipeline: ScoringPipeline,
    scoring_config: ScoringPipelineConfig,
    rerank_config: RerankConfig,
}

impl FactRetrieval {
    /// Create a new fact retrieval service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: FactRetrievalConfig,
    ) -> Self {
        let scoring_config = ScoringPipelineConfig::default();
        let scoring_pipeline = ScoringPipeline::from_config(&scoring_config);
        Self {
            database,
            embedder,
            config,
            hybrid_config: HybridSearchConfig::default(),
            scoring_pipeline,
            scoring_config,
            rerank_config: RerankConfig::default(),
        }
    }

    /// Create with default configuration
    pub fn with_defaults(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self::new(database, embedder, FactRetrievalConfig::default())
    }

    /// Set the rerank configuration (enables cross-encoder reranking when config.enabled = true)
    pub fn with_rerank_config(mut self, config: RerankConfig) -> Self {
        self.rerank_config = config;
        self
    }

    /// Retrieve relevant context for a query
    ///
    /// Uses hybrid search (vector + BM25 RRF fusion) with the full
    /// scoring pipeline applied.  Falls back to pure vector search
    /// when the hybrid path encounters an error.
    pub async fn retrieve(&self, query: &str) -> Result<RetrievalResult, AlephError> {
        // Generate query embedding
        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| AlephError::other(format!("Failed to embed query: {}", e)))?;

        let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner));
        self.hybrid_search_with_fallback(query, &query_embedding, &filter)
            .await
    }

    /// Retrieve relevant context for a query within a specific workspace.
    ///
    /// Same as `retrieve()` but adds a workspace filter to isolate results
    /// to the given workspace. This enables memory isolation per workspace.
    pub async fn retrieve_in_workspace(
        &self,
        query: &str,
        workspace: &str,
    ) -> Result<RetrievalResult, AlephError> {
        use crate::gateway::agent_env::AgentEnvFilter;
        self.retrieve_with_filter(query, AgentEnvFilter::Single(workspace.to_string()))
            .await
    }

    /// Retrieve facts across workspaces using a AgentEnvFilter.
    ///
    /// Supports Single, Multiple, and All workspace scopes. This is the
    /// generalized form of `retrieve_in_workspace()`.
    pub async fn retrieve_with_filter(
        &self,
        query: &str,
        filter: crate::gateway::agent_env::AgentEnvFilter,
    ) -> Result<RetrievalResult, AlephError> {
        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| AlephError::other(format!("Failed to embed query: {}", e)))?;

        self.retrieve_with_embedding(query, &query_embedding, filter)
            .await
    }

    /// Internal: retrieve using a pre-computed embedding vector.
    /// Avoids redundant embedding API calls when the same query is searched
    /// across multiple workspace scopes (e.g., smart recall Phase 1 + Phase 2).
    async fn retrieve_with_embedding(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        filter: crate::gateway::agent_env::AgentEnvFilter,
    ) -> Result<RetrievalResult, AlephError> {
        let search_filter =
            SearchFilter::valid_only(Some(NamespaceScope::Owner)).with_agent_filter(filter.clone());
        self.hybrid_search_with_fallback(query_text, query_embedding, &search_filter)
            .await
    }

    /// Retrieve with custom limits
    pub async fn retrieve_with_limits(
        &self,
        query: &str,
        max_facts: u32,
        max_raw_fallback: u32,
    ) -> Result<RetrievalResult, AlephError> {
        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| AlephError::other(format!("Failed to embed query: {}", e)))?;

        let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner));
        let result = self
            .hybrid_search_with_fallback_limit(
                query,
                &query_embedding,
                &filter,
                max_facts as usize,
            )
            .await;

        let _ = max_raw_fallback; // no longer used — raw memory fallback removed

        result
    }

    // ── Hybrid search core ───────────────────────────────────────────────

    /// Run hybrid search (vector + BM25 RRF fusion) with scoring pipeline,
    /// falling back to pure vector search on error.
    async fn hybrid_search_with_fallback(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        filter: &SearchFilter,
    ) -> Result<RetrievalResult, AlephError> {
        self.hybrid_search_with_fallback_limit(
            query_text,
            query_embedding,
            filter,
            self.config.max_facts as usize,
        )
        .await
    }

    /// Run hybrid search with a custom result limit.
    async fn hybrid_search_with_fallback_limit(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<RetrievalResult, AlephError> {
        let dim_hint = query_embedding.len() as u32;

        // Expand query for BM25 (appends Chinese synonyms for CJK queries)
        let expanded = query_expander::expand(query_text);

        // Try hybrid search first (vector + BM25 RRF fusion)
        let scored_facts = match self
            .database
            .hybrid_search(&HybridSearchParams {
                embedding: query_embedding,
                dim_hint,
                query_text: &expanded.bm25_query,
                vector_weight: self.hybrid_config.vector_weight,
                text_weight: self.hybrid_config.text_weight,
                filter,
                limit,
            })
            .await
        {
            Ok(results) => results,
            Err(e) => {
                // Fallback to pure vector search
                tracing::warn!(
                    error = %e,
                    "Hybrid search failed, falling back to vector-only"
                );
                self.database
                    .vector_search(query_embedding, dim_hint, filter, limit)
                    .await?
            }
        };

        // Apply scoring pipeline for re-ranking
        let scored_facts = self.apply_pipeline(scored_facts, query_embedding, query_text);

        // Apply cross-encoder reranking if enabled
        let scored_facts = if self.rerank_config.enabled {
            self.apply_rerank(query_text, scored_facts).await
        } else {
            scored_facts
        };

        // Filter by threshold and convert
        let facts: Vec<MemoryFact> = scored_facts
            .into_iter()
            .filter(|sf| sf.score >= self.config.similarity_threshold)
            .map(|sf| {
                let mut fact = sf.fact;
                fact.similarity_score = Some(sf.score);
                fact
            })
            .collect();

        Ok(RetrievalResult { facts })
    }

    /// Apply the scoring pipeline to re-rank candidates.
    fn apply_pipeline(
        &self,
        scored: Vec<ScoredFact>,
        query_embedding: &[f32],
        query_text: &str,
    ) -> Vec<ScoredFact> {
        let ctx = ScoringContext {
            query: query_text.to_string(),
            query_embedding: Some(query_embedding.to_vec()),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            config: self.scoring_config.clone(),
        };
        self.scoring_pipeline.run(scored, &ctx)
    }

    /// Apply cross-encoder reranking to scored results.
    ///
    /// Calls the configured rerank provider (HTTP API), then blends the
    /// cross-encoder scores with the original retrieval scores.
    /// Returns the original results unchanged on any error.
    async fn apply_rerank(
        &self,
        query: &str,
        results: Vec<ScoredFact>,
    ) -> Vec<ScoredFact> {
        if results.is_empty() {
            return results;
        }

        let provider = build_provider(&self.rerank_config);
        let documents: Vec<String> = results.iter().map(|sf| sf.fact.content.clone()).collect();
        let top_n = results.len();

        let rerank_results = match provider.rerank(query, &documents, top_n).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "Rerank failed, returning un-reranked results");
                return results;
            }
        };

        let originals: Vec<(String, f32)> = results
            .iter()
            .map(|sf| (sf.fact.id.clone(), sf.score))
            .collect();

        let blended = blend_scores(
            &originals,
            &rerank_results,
            self.rerank_config.rerank_weight,
        );

        let mut reranked = Vec::with_capacity(blended.len());
        for (id, score) in &blended {
            if let Some(sf) = results.iter().find(|sf| &sf.fact.id == id) {
                reranked.push(ScoredFact {
                    fact: sf.fact.clone(),
                    score: *score,
                });
            }
        }
        reranked
    }

    /// Format retrieval result as context for LLM
    pub fn format_context(result: &RetrievalResult) -> String {
        let mut context = String::new();

        if !result.facts.is_empty() {
            context.push_str("## Known User Information\n\n");
            for fact in &result.facts {
                context.push_str(&format!("- {}\n", format_fact_with_source(fact)));
            }
            context.push('\n');
        }

        context
    }

    /// Format retrieval result in Chinese
    pub fn format_context_zh(result: &RetrievalResult) -> String {
        let mut context = String::new();

        if !result.facts.is_empty() {
            context.push_str("## 已知用户信息\n\n");
            for fact in &result.facts {
                context.push_str(&format!("- {}\n", format_fact_with_source(fact)));
            }
            context.push('\n');
        }

        context
    }

    /// Two-Phase Smart Recall: automatically expands to cross-workspace search
    /// when primary results are sparse or low-relevance.
    ///
    /// Phase 1: Search the primary workspace.
    /// Phase 2 (conditional): If top score < threshold OR result count < minimum,
    ///   expand to ALL workspaces, excluding already-found primary results.
    pub async fn retrieve_with_smart_recall(
        &self,
        query: &str,
        primary_workspace: &str,
        config: &crate::config::types::profile::SmartRecallConfig,
    ) -> Result<SmartRetrievalResult, AlephError> {
        use crate::gateway::agent_env::AgentEnvFilter;
        use tracing::debug;

        // Embed query ONCE — reused across both phases
        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| AlephError::other(format!("Failed to embed query: {}", e)))?;

        // Phase 1: Search primary workspace
        let primary = self
            .retrieve_with_embedding(
                query,
                &query_embedding,
                AgentEnvFilter::Single(primary_workspace.to_string()),
            )
            .await?;

        // Evaluate trigger conditions
        let top_score = primary
            .facts
            .iter()
            .filter_map(|f| f.similarity_score)
            .fold(0.0f32, f32::max);
        let result_count = primary.facts.len();

        let low_score = top_score < config.score_threshold;
        let too_few = result_count < config.min_primary_results;

        if !low_score && !too_few {
            // Phase 1 sufficient — no cross-workspace expansion needed
            return Ok(SmartRetrievalResult {
                primary,
                cross_workspace: Vec::new(),
                recall_triggered: false,
                trigger_reason: None,
            });
        }

        // Phase 2: Expand to all workspaces
        let trigger_reason = if low_score && too_few {
            format!(
                "top_score {:.2} < threshold {:.2} AND count {} < min {}",
                top_score, config.score_threshold, result_count, config.min_primary_results
            )
        } else if low_score {
            format!(
                "top_score {:.2} < threshold {:.2}",
                top_score, config.score_threshold
            )
        } else {
            format!(
                "count {} < min {}",
                result_count, config.min_primary_results
            )
        };

        debug!(
            query = query,
            primary_workspace = primary_workspace,
            reason = %trigger_reason,
            "Smart Recall Phase 2 triggered"
        );

        let all_results = self
            .retrieve_with_embedding(query, &query_embedding, AgentEnvFilter::All)
            .await?;

        // Collect primary fact IDs for deduplication
        let primary_ids: std::collections::HashSet<&str> =
            primary.facts.iter().map(|f| f.id.as_str()).collect();

        // Filter to cross-workspace results only, excluding primary workspace facts
        let mut cross_facts: Vec<CrossWorkspaceFact> = all_results
            .facts
            .into_iter()
            .filter(|f| f.agent != primary_workspace && !primary_ids.contains(f.id.as_str()))
            .map(|f| CrossWorkspaceFact {
                content: f.content.clone(),
                source_workspace: f.agent.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
            })
            .collect();

        // Sort by relevance and take top N
        cross_facts.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        cross_facts.truncate(config.max_cross_results);

        debug!(
            cross_count = cross_facts.len(),
            "Smart Recall Phase 2 complete"
        );

        Ok(SmartRetrievalResult {
            primary,
            cross_workspace: cross_facts,
            recall_triggered: true,
            trigger_reason: Some(trigger_reason),
        })
    }

    /// Retrieve facts and asynchronously record recall signals.
    ///
    /// Wraps the existing `retrieve` method.  After a successful retrieval,
    /// a fire-and-forget `tokio::spawn` records which facts were recalled so
    /// the promotion scorer can later use those signals.
    pub async fn retrieve_with_signals(
        &self,
        query: &str,
        namespace: &NamespaceScope,
        channel: &str,
        session_id: Option<&str>,
    ) -> Result<RetrievalResult, AlephError> {
        use crate::memory::store::sqlite::recall_signals::RecallHit;

        let result = self.retrieve(query).await?;

        if !result.facts.is_empty() {
            let hits: Vec<RecallHit> = result
                .facts
                .iter()
                .map(|f| RecallHit {
                    fact_id: f.id.clone(),
                    score: f.similarity_score.unwrap_or(f.confidence) as f64,
                })
                .collect();

            let db = self.database.clone();
            let query_owned = query.to_string();
            let channel_owned = channel.to_string();
            let session_owned = session_id.map(|s| s.to_string());
            let ns = namespace.to_namespace_value();

            tokio::spawn(async move {
                if let Err(e) = db.record_signals(
                    &query_owned,
                    &channel_owned,
                    &hits,
                    session_owned.as_deref(),
                    &ns,
                ) {
                    tracing::warn!("recall signal recording failed: {e}");
                }
            });
        }

        Ok(result)
    }

    /// Update configuration
    pub fn update_config(&mut self, config: FactRetrievalConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn get_config(&self) -> &FactRetrievalConfig {
        &self.config
    }
}

/// Result of a Two-Phase Smart Recall retrieval
#[derive(Debug, Clone)]
pub struct SmartRetrievalResult {
    /// Primary workspace results (Phase 1)
    pub primary: RetrievalResult,
    /// Cross-workspace results (Phase 2, empty if not triggered)
    pub cross_workspace: Vec<CrossWorkspaceFact>,
    /// Whether Phase 2 was triggered
    pub recall_triggered: bool,
    /// Reason for trigger (for logging/debugging)
    pub trigger_reason: Option<String>,
}

/// A fact from a different workspace discovered by Smart Recall
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossWorkspaceFact {
    /// Fact content
    pub content: String,
    /// Source workspace ID
    pub source_workspace: String,
    /// Relevance score from vector search
    pub relevance_score: f32,
}

// ============================================================================
// Layered MemoryContext
// ============================================================================

/// Two-layer memory context for prompt injection.
///
/// Layer 1 (`background`): stable, high-value synthesis facts that persist
/// across conversations — the system's long-term knowledge about the user.
///
/// Layer 2 (`relevant`): query-relevant facts retrieved via hybrid search
/// for the current conversation turn.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Layer 1: stable background (synthesis facts)
    pub background: Vec<MemoryFact>,
    /// Layer 2: query-relevant detail (hybrid search results)
    pub relevant: Vec<MemoryFact>,
}

impl MemoryContext {
    pub fn is_empty(&self) -> bool {
        self.background.is_empty() && self.relevant.is_empty()
    }

    /// Format for system prompt injection.
    pub fn to_prompt_sections(&self) -> String {
        let mut sections = String::new();

        if !self.background.is_empty() {
            sections.push_str("## Long-term knowledge about the user\n");
            for fact in &self.background {
                sections.push_str("- ");
                sections.push_str(&fact.content);
                sections.push('\n');
            }
            sections.push('\n');
        }

        if !self.relevant.is_empty() {
            sections.push_str("## Relevant memories for this conversation\n");
            for fact in &self.relevant {
                sections.push_str("- ");
                sections.push_str(&fact.content);
                sections.push('\n');
            }
        }

        sections
    }
}

impl FactRetrieval {
    /// Build a two-layer memory context for prompt injection.
    ///
    /// - **Layer 1 (background)**: top synthesis facts from the namespace,
    ///   representing stable long-term knowledge.
    /// - **Layer 2 (relevant)**: query-relevant facts from hybrid vector search.
    pub async fn build_layered_context(
        &self,
        query: &str,
        namespace: &NamespaceScope,
    ) -> Result<MemoryContext, AlephError> {
        // Layer 1: background synthesis facts (sync — SQLite behind Mutex)
        let ns_value = namespace.to_namespace_value();
        let background = self.database.top_synthesized(&ns_value, 10)?;

        // Layer 2: query-relevant retrieval (existing async path)
        let retrieval = self.retrieve(query).await?;

        Ok(MemoryContext {
            background,
            relevant: retrieval.facts,
        })
    }
}

/// Format a single fact with source citation metadata.
///
/// Produces format: `[Source: path#id] content` for LLM citation.
/// Falls back to `[Source: #id] content` when path is empty.
fn format_fact_with_source(fact: &MemoryFact) -> String {
    if fact.path.is_empty() {
        format!("[Source: #{}] {}", fact.id, fact.content)
    } else {
        format!("[Source: {}#{}] {}", fact.path, fact.id, fact.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::NoteType;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use tempfile::tempdir;

    async fn create_test_retrieval() -> (FactRetrieval, MemoryBackend) {
        use crate::memory::embedding_provider::tests::MockEmbeddingProvider;

        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();
        let database: MemoryBackend =
            Arc::new(SqliteMemoryBackend::new(&path).unwrap());

        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));

        // Leak the temp_dir to prevent cleanup during test
        std::mem::forget(temp_dir);

        let retrieval = FactRetrieval::with_defaults(database.clone(), embedder);

        (retrieval, database)
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_empty_retrieval() {
        let (retrieval, _) = create_test_retrieval().await;

        let result = retrieval.retrieve("test query").await.unwrap();

        assert!(result.is_empty());
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_format_context_with_facts() {
        let facts = vec![
            MemoryFact::new(
                "The user is learning Rust".to_string(),
                NoteType::Learning,
                vec!["mem-1".to_string()],
            ),
            MemoryFact::new(
                "The user prefers dark mode".to_string(),
                NoteType::Preference,
                vec!["mem-2".to_string()],
            ),
        ];

        let result = RetrievalResult {
            facts,
        };

        let context = FactRetrieval::format_context(&result);

        assert!(context.contains("Known User Information"));
        assert!(context.contains("learning Rust"));
        assert!(context.contains("dark mode"));
        // Verify source metadata is present (MemoryFact::new sets path from NoteType)
        assert!(context.contains("[Source: aleph://knowledge/learning/"));
        assert!(context.contains("[Source: aleph://user/preferences/"));
    }

    #[tokio::test]
    async fn test_format_context_zh() {
        let facts = vec![MemoryFact::new(
            "用户正在学习 Rust".to_string(),
            NoteType::Learning,
            vec!["mem-1".to_string()],
        )];

        let result = RetrievalResult {
            facts,
        };

        let context = FactRetrieval::format_context_zh(&result);

        assert!(context.contains("已知用户信息"));
        assert!(context.contains("学习 Rust"));
        // Verify source metadata in Chinese format too
        assert!(context.contains("[Source: aleph://knowledge/learning/"));
    }

    #[tokio::test]
    async fn test_format_context_includes_source_citation() {
        let mut fact = MemoryFact::new(
            "User prefers dark mode".to_string(),
            NoteType::Preference,
            vec!["mem-1".to_string()],
        );
        fact.path = "aleph://user/preferences/ui".to_string();

        let result = RetrievalResult {
            facts: vec![fact.clone()],
        };

        let context = FactRetrieval::format_context(&result);

        assert!(context.contains("[Source: aleph://user/preferences/ui#"));
        assert!(context.contains("User prefers dark mode"));
    }

    #[tokio::test]
    async fn test_format_fact_with_empty_path_fallback() {
        let mut fact = MemoryFact::new(
            "Some fact without path".to_string(),
            NoteType::Other,
            vec!["mem-1".to_string()],
        );
        fact.path = String::new(); // Force empty path

        let result = RetrievalResult {
            facts: vec![fact.clone()],
        };

        let context = FactRetrieval::format_context(&result);

        // Should use fallback format with just #id
        assert!(context.contains(&format!("[Source: #{}]", fact.id)));
        assert!(context.contains("Some fact without path"));
        // Should NOT contain double slash from empty path
        assert!(!context.contains("[Source: #]"));
    }

    #[test]
    fn test_config_default() {
        let config = FactRetrievalConfig::default();
        assert_eq!(config.max_facts, 5);
        assert_eq!(config.max_raw_fallback, 3);
    }

    // ── Smart Recall tests ──────────────────────────────────────────────────

    #[test]
    fn test_smart_retrieval_result_not_triggered() {
        let result = SmartRetrievalResult {
            primary: RetrievalResult::default(),
            cross_workspace: Vec::new(),
            recall_triggered: false,
            trigger_reason: None,
        };
        assert!(!result.recall_triggered);
        assert!(result.cross_workspace.is_empty());
        assert!(result.trigger_reason.is_none());
    }

    #[test]
    fn test_smart_retrieval_result_triggered() {
        let result = SmartRetrievalResult {
            primary: RetrievalResult::default(),
            cross_workspace: vec![
                CrossWorkspaceFact {
                    content: "Deep work requires 90-minute blocks".to_string(),
                    source_workspace: "health".to_string(),
                    relevance_score: 0.72,
                },
                CrossWorkspaceFact {
                    content: "Cal Newport's Deep Work".to_string(),
                    source_workspace: "reading".to_string(),
                    relevance_score: 0.68,
                },
            ],
            recall_triggered: true,
            trigger_reason: Some("top_score 0.52 < threshold 0.60".to_string()),
        };
        assert!(result.recall_triggered);
        assert_eq!(result.cross_workspace.len(), 2);
        assert_eq!(result.cross_workspace[0].source_workspace, "health");
        assert_eq!(result.cross_workspace[1].source_workspace, "reading");
        assert!(
            result.cross_workspace[0].relevance_score > result.cross_workspace[1].relevance_score
        );
    }

    #[test]
    fn test_cross_workspace_fact_serde() {
        let fact = CrossWorkspaceFact {
            content: "Test content".to_string(),
            source_workspace: "coding".to_string(),
            relevance_score: 0.85,
        };
        let json = serde_json::to_string(&fact).unwrap();
        let deserialized: CrossWorkspaceFact = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.content, "Test content");
        assert_eq!(deserialized.source_workspace, "coding");
        assert!((deserialized.relevance_score - 0.85).abs() < f32::EPSILON);
    }

    // ── MemoryContext tests ────────────────────────────────────────────────

    #[test]
    fn memory_context_empty() {
        let ctx = MemoryContext::default();
        assert!(ctx.is_empty());
        assert!(ctx.background.is_empty());
        assert!(ctx.relevant.is_empty());
        assert_eq!(ctx.to_prompt_sections(), "");
    }

    #[test]
    fn memory_context_to_prompt_sections() {
        let bg_fact = MemoryFact::new(
            "User is a Rust developer".to_string(),
            NoteType::Personal,
            vec!["src-1".to_string()],
        );
        let rel_fact = MemoryFact::new(
            "User asked about async patterns".to_string(),
            NoteType::Learning,
            vec!["src-2".to_string()],
        );

        let ctx = MemoryContext {
            background: vec![bg_fact],
            relevant: vec![rel_fact],
        };

        assert!(!ctx.is_empty());

        let sections = ctx.to_prompt_sections();
        assert!(sections.contains("## Long-term knowledge about the user"));
        assert!(sections.contains("- User is a Rust developer"));
        assert!(sections.contains("## Relevant memories for this conversation"));
        assert!(sections.contains("- User asked about async patterns"));
    }

    #[test]
    fn memory_context_background_only() {
        let fact = MemoryFact::new(
            "Background fact".to_string(),
            NoteType::Other,
            vec!["src-1".to_string()],
        );
        let ctx = MemoryContext {
            background: vec![fact],
            relevant: vec![],
        };
        assert!(!ctx.is_empty());
        let sections = ctx.to_prompt_sections();
        assert!(sections.contains("## Long-term knowledge"));
        assert!(!sections.contains("## Relevant memories"));
    }

    #[test]
    fn memory_context_relevant_only() {
        let fact = MemoryFact::new(
            "Relevant fact".to_string(),
            NoteType::Other,
            vec!["src-1".to_string()],
        );
        let ctx = MemoryContext {
            background: vec![],
            relevant: vec![fact],
        };
        assert!(!ctx.is_empty());
        let sections = ctx.to_prompt_sections();
        assert!(!sections.contains("## Long-term knowledge"));
        assert!(sections.contains("## Relevant memories"));
    }
}
