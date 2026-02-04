//! Fact-First Retrieval
//!
//! Retrieves relevant context with priority given to compressed facts
//! over raw memories. Falls back to raw memories when facts are insufficient.

use crate::error::AlephError;
use crate::memory::context::{MemoryEntry, MemoryFact};
use crate::memory::database::VectorDatabase;
use crate::memory::smart_embedder::SmartEmbedder;
use std::sync::Arc;

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
    database: Arc<VectorDatabase>,
    embedder: SmartEmbedder,
    config: FactRetrievalConfig,
}

impl FactRetrieval {
    /// Create a new fact retrieval service
    pub fn new(
        database: Arc<VectorDatabase>,
        embedder: SmartEmbedder,
        config: FactRetrievalConfig,
    ) -> Self {
        Self {
            database,
            embedder,
            config,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(database: Arc<VectorDatabase>, embedder: SmartEmbedder) -> Self {
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
        let facts = self
            .database
            .search_facts(&query_embedding, self.config.max_facts, false)
            .await?;

        // Filter by similarity threshold
        let facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter(|f| {
                f.similarity_score
                    .map(|s| s >= self.config.similarity_threshold)
                    .unwrap_or(false)
            })
            .collect();

        // 2. If facts are insufficient, fallback to raw memories
        let raw_memories = if facts.len() < self.config.max_facts as usize {
            let remaining = self.config.max_raw_fallback;

            if remaining > 0 {
                self.database
                    .search_memories("", "", &query_embedding, remaining)
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

        let facts = self
            .database
            .search_facts(&query_embedding, max_facts, false)
            .await?;

        let facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter(|f| {
                f.similarity_score
                    .map(|s| s >= self.config.similarity_threshold)
                    .unwrap_or(false)
            })
            .collect();

        let raw_memories = if facts.len() < max_facts as usize && max_raw_fallback > 0 {
            self.database
                .search_memories("", "", &query_embedding, max_raw_fallback)
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
                context.push_str(&format!("- {}\n", fact.content));
            }
            context.push('\n');
        }

        // Format raw memories
        if !result.raw_memories.is_empty() {
            context.push_str("## Related Conversation History\n\n");
            for memory in &result.raw_memories {
                // Truncate long responses
                let ai_output: String = memory.ai_output.chars().take(300).collect();
                let truncated = if memory.ai_output.len() > 300 {
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
                context.push_str(&format!("- {}\n", fact.content));
            }
            context.push('\n');
        }

        if !result.raw_memories.is_empty() {
            context.push_str("## 相关对话历史\n\n");
            for memory in &result.raw_memories {
                let ai_output: String = memory.ai_output.chars().take(300).collect();
                let truncated = if memory.ai_output.len() > 300 {
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

    /// Update configuration
    pub fn update_config(&mut self, config: FactRetrievalConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn get_config(&self) -> &FactRetrievalConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;
    use tempfile::tempdir;

    async fn create_test_retrieval() -> (FactRetrieval, Arc<VectorDatabase>) {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_retrieval.db");
        let database = Arc::new(VectorDatabase::new(db_path).unwrap());

        let cache_dir = temp_dir.path().join("models");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let embedder = SmartEmbedder::new(cache_dir, 300);

        let retrieval = FactRetrieval::with_defaults(Arc::clone(&database), embedder);

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
    }

    #[test]
    fn test_config_default() {
        let config = FactRetrievalConfig::default();
        assert_eq!(config.max_facts, 5);
        assert_eq!(config.max_raw_fallback, 3);
    }
}
