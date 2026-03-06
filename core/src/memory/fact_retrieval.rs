//! Fact-First Retrieval
//!
//! Retrieves relevant context with priority given to compressed facts
//! over raw memories. Falls back to raw memories when facts are insufficient.

use serde::{Deserialize, Serialize};

use crate::error::AlephError;
use crate::memory::context::{MemoryEntry, MemoryFact};
use crate::memory::namespace::NamespaceScope;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use crate::memory::store::types::{MemoryFilter, SearchFilter};
use crate::memory::store::{MemoryBackend, MemoryStore, SessionStore};

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
            similarity_threshold: 0.5,
        }
    }
}

/// Result of a retrieval operation
#[derive(Debug, Clone, Default)]
pub struct RetrievalResult {
    /// Retrieved compressed facts
    pub facts: Vec<MemoryFact>,
    /// Fallback raw memories (when facts insufficient)
    pub raw_memories: Vec<MemoryEntry>,
}

impl RetrievalResult {
    /// Check if result is empty
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.raw_memories.is_empty()
    }

    /// Total number of retrieved items
    pub fn len(&self) -> usize {
        self.facts.len() + self.raw_memories.len()
    }
}

/// Fact-first retrieval service
pub struct FactRetrieval {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: FactRetrievalConfig,
}

impl FactRetrieval {
    /// Create a new fact retrieval service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: FactRetrievalConfig,
    ) -> Self {
        Self {
            database,
            embedder,
            config,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self::new(database, embedder, FactRetrievalConfig::default())
    }

    /// Retrieve relevant context for a query
    ///
    /// Priority:
    /// 1. Compressed facts (from memory_facts table)
    /// 2. Raw memories (fallback when facts insufficient)
    pub async fn retrieve(&self, query: &str) -> Result<RetrievalResult, AlephError> {
        // Generate query embedding
        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| AlephError::other(format!("Failed to embed query: {}", e)))?;

        // 1. Search facts first
        let dim_hint = query_embedding.len() as u32;
        let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner));
        let scored_facts = self
            .database
            .vector_search(&query_embedding, dim_hint, &filter, self.config.max_facts as usize)
            .await?;

        // Map ScoredFact -> MemoryFact with similarity_score, and filter by threshold
        let facts: Vec<MemoryFact> = scored_facts
            .into_iter()
            .filter(|sf| sf.score >= self.config.similarity_threshold)
            .map(|sf| {
                let mut fact = sf.fact;
                fact.similarity_score = Some(sf.score);
                fact
            })
            .collect();

        // 2. If facts are insufficient, fallback to raw memories
        let raw_memories = if facts.len() < self.config.max_facts as usize {
            let remaining = self.config.max_raw_fallback;

            if remaining > 0 {
                self.database
                    .search_memories(&query_embedding, &MemoryFilter::default(), remaining as usize)
                    .await?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Ok(RetrievalResult {
            facts,
            raw_memories,
        })
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
        use crate::gateway::workspace::WorkspaceFilter;
        self.retrieve_with_filter(query, WorkspaceFilter::Single(workspace.to_string()))
            .await
    }

    /// Retrieve facts across workspaces using a WorkspaceFilter.
    ///
    /// Supports Single, Multiple, and All workspace scopes. This is the
    /// generalized form of `retrieve_in_workspace()`.
    pub async fn retrieve_with_filter(
        &self,
        query: &str,
        filter: crate::gateway::workspace::WorkspaceFilter,
    ) -> Result<RetrievalResult, AlephError> {
        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| AlephError::other(format!("Failed to embed query: {}", e)))?;

        // Build search filter with workspace scope
        let dim_hint = query_embedding.len() as u32;
        let search_filter = SearchFilter::valid_only(Some(NamespaceScope::Owner))
            .with_workspace(filter.clone());
        let scored_facts = self
            .database
            .vector_search(&query_embedding, dim_hint, &search_filter, self.config.max_facts as usize)
            .await?;

        let facts: Vec<MemoryFact> = scored_facts
            .into_iter()
            .filter(|sf| sf.score >= self.config.similarity_threshold)
            .map(|sf| {
                let mut fact = sf.fact;
                fact.similarity_score = Some(sf.score);
                fact
            })
            .collect();

        // Fallback to raw memories with same workspace filter
        let raw_memories = if facts.len() < self.config.max_facts as usize {
            let remaining = self.config.max_raw_fallback;
            if remaining > 0 {
                let mem_filter = MemoryFilter {
                    workspace: Some(filter),
                    ..Default::default()
                };
                self.database
                    .search_memories(&query_embedding, &mem_filter, remaining as usize)
                    .await?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Ok(RetrievalResult {
            facts,
            raw_memories,
        })
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

        let dim_hint = query_embedding.len() as u32;
        let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner));
        let scored_facts = self
            .database
            .vector_search(&query_embedding, dim_hint, &filter, max_facts as usize)
            .await?;

        let facts: Vec<MemoryFact> = scored_facts
            .into_iter()
            .filter(|sf| sf.score >= self.config.similarity_threshold)
            .map(|sf| {
                let mut fact = sf.fact;
                fact.similarity_score = Some(sf.score);
                fact
            })
            .collect();

        let raw_memories = if facts.len() < max_facts as usize && max_raw_fallback > 0 {
            self.database
                .search_memories(&query_embedding, &MemoryFilter::default(), max_raw_fallback as usize)
                .await?
        } else {
            Vec::new()
        };

        Ok(RetrievalResult {
            facts,
            raw_memories,
        })
    }

    /// Format retrieval result as context for LLM
    pub fn format_context(result: &RetrievalResult) -> String {
        let mut context = String::new();

        // Format facts
        if !result.facts.is_empty() {
            context.push_str("## Known User Information\n\n");
            for fact in &result.facts {
                context.push_str(&format!("- {}\n", format_fact_with_source(fact)));
            }
            context.push('\n');
        }

        // Format raw memories
        if !result.raw_memories.is_empty() {
            context.push_str("## Related Conversation History\n\n");
            for memory in &result.raw_memories {
                // Truncate long responses
                let ai_output: String = memory.ai_output.chars().take(300).collect();
                let truncated = if memory.ai_output.chars().count() > 300 {
                    format!("{}...", ai_output)
                } else {
                    ai_output
                };

                context.push_str(&format!(
                    "**User**: {}\n**Assistant**: {}\n\n",
                    memory.user_input, truncated
                ));
            }
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

        if !result.raw_memories.is_empty() {
            context.push_str("## 相关对话历史\n\n");
            for memory in &result.raw_memories {
                let ai_output: String = memory.ai_output.chars().take(300).collect();
                let truncated = if memory.ai_output.chars().count() > 300 {
                    format!("{}...", ai_output)
                } else {
                    ai_output
                };

                context.push_str(&format!(
                    "**用户**: {}\n**助手**: {}\n\n",
                    memory.user_input, truncated
                ));
            }
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
        use crate::gateway::workspace::WorkspaceFilter;
        use tracing::debug;

        // Phase 1: Search primary workspace
        let primary = self
            .retrieve_with_filter(query, WorkspaceFilter::Single(primary_workspace.to_string()))
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
            .retrieve_with_filter(query, WorkspaceFilter::All)
            .await?;

        // Collect primary fact IDs for deduplication
        let primary_ids: std::collections::HashSet<&str> =
            primary.facts.iter().map(|f| f.id.as_str()).collect();

        // Filter to cross-workspace results only, excluding primary workspace facts
        let mut cross_facts: Vec<CrossWorkspaceFact> = all_results
            .facts
            .into_iter()
            .filter(|f| f.workspace != primary_workspace && !primary_ids.contains(f.id.as_str()))
            .map(|f| CrossWorkspaceFact {
                content: f.content.clone(),
                source_workspace: f.workspace.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
            })
            .collect();

        // Sort by relevance and take top N
        cross_facts.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap_or(std::cmp::Ordering::Equal));
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
    use crate::memory::context::FactType;
    use crate::memory::store::lance::LanceMemoryBackend;
    use crate::sync_primitives::Arc;
    use tempfile::tempdir;

    async fn create_test_retrieval() -> (FactRetrieval, MemoryBackend) {
        use crate::memory::embedding_provider::tests::MockEmbeddingProvider;

        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();
        let database: MemoryBackend = Arc::new(LanceMemoryBackend::open_or_create(&path).await.unwrap());

        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));

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
                FactType::Learning,
                vec!["mem-1".to_string()],
            ),
            MemoryFact::new(
                "The user prefers dark mode".to_string(),
                FactType::Preference,
                vec!["mem-2".to_string()],
            ),
        ];

        let result = RetrievalResult {
            facts,
            raw_memories: vec![],
        };

        let context = FactRetrieval::format_context(&result);

        assert!(context.contains("Known User Information"));
        assert!(context.contains("learning Rust"));
        assert!(context.contains("dark mode"));
        // Verify source metadata is present (MemoryFact::new sets path from FactType)
        assert!(context.contains("[Source: aleph://knowledge/learning/"));
        assert!(context.contains("[Source: aleph://user/preferences/"));
    }

    #[tokio::test]
    async fn test_format_context_zh() {
        let facts = vec![MemoryFact::new(
            "用户正在学习 Rust".to_string(),
            FactType::Learning,
            vec!["mem-1".to_string()],
        )];

        let result = RetrievalResult {
            facts,
            raw_memories: vec![],
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
            FactType::Preference,
            vec!["mem-1".to_string()],
        );
        fact.path = "aleph://user/preferences/ui".to_string();

        let result = RetrievalResult {
            facts: vec![fact.clone()],
            raw_memories: vec![],
        };

        let context = FactRetrieval::format_context(&result);

        assert!(context.contains("[Source: aleph://user/preferences/ui#"));
        assert!(context.contains("User prefers dark mode"));
    }

    #[tokio::test]
    async fn test_format_fact_with_empty_path_fallback() {
        let mut fact = MemoryFact::new(
            "Some fact without path".to_string(),
            FactType::Other,
            vec!["mem-1".to_string()],
        );
        fact.path = String::new(); // Force empty path

        let result = RetrievalResult {
            facts: vec![fact.clone()],
            raw_memories: vec![],
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
        assert!(result.cross_workspace[0].relevance_score > result.cross_workspace[1].relevance_score);
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
}
