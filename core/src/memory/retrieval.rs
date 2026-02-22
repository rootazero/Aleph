/// Memory retrieval module
///
/// This module handles retrieval of semantically similar past interactions
/// filtered by current context (app + window).
use crate::config::MemoryConfig;
use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::dreaming::record_activity;
use crate::memory::graph::GraphStore;
use crate::memory::smart_embedder::SmartEmbedder;
use crate::memory::store::{MemoryBackend, SessionStore};
use crate::memory::store::types::MemoryFilter;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Memory retrieval service for searching past interactions
#[derive(Clone)]
pub struct MemoryRetrieval {
    database: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
    config: Arc<MemoryConfig>,
    graph_store: Option<GraphStore>,
}

impl MemoryRetrieval {
    /// Create new retrieval service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<SmartEmbedder>,
        config: Arc<MemoryConfig>,
    ) -> Self {
        // NOTE: DreamDaemon and GraphStore are SQLite-based and will be migrated
        // to LanceDB in Phase 5. For now, skip their initialization.
        Self {
            database,
            embedder,
            config,
            graph_store: None,
        }
    }

    async fn resolve_entity_filter(
        &self,
        context: &ContextAnchor,
        query: &str,
    ) -> Option<String> {
        let graph_store = self.graph_store.as_ref()?;
        let hints = GraphStore::extract_query_hints(query);
        if hints.is_empty() {
            return None;
        }

        let context_key = format!(
            "app:{}|window:{}",
            context.app_bundle_id, context.window_title
        );

        for hint in hints {
            if let Ok(resolved) = graph_store.resolve_entity(&hint, Some(&context_key)).await {
                if let Some(best) = resolved.first() {
                    if !best.ambiguous {
                        return Some(best.node_id.clone());
                    }
                }
            }
        }

        None
    }

    /// Retrieve memories for current context
    ///
    /// Process flow:
    /// 1. Check if memory is enabled
    /// 2. Generate embedding for query
    /// 3. Search database filtered by context
    /// 4. Filter by similarity threshold
    /// 5. Return top-K results
    ///
    /// # Arguments
    /// * `context` - Context anchor (app + window)
    /// * `query` - User query text
    ///
    /// # Returns
    /// * `Result<Vec<MemoryEntry>>` - List of relevant memories with similarity scores
    pub async fn retrieve_memories(
        &self,
        context: &ContextAnchor,
        query: &str,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();
        debug!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            query_len = query.len(),
            max_items = self.config.max_context_items,
            "Starting memory retrieval"
        );

        // 1. Check if memory is enabled
        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        // 2. Generate query embedding
        debug!("Generating query embedding");
        let query_embedding = self.embedder.embed(query).await.map_err(|e| {
            warn!(error = %e, "Failed to generate query embedding");
            AlephError::config(format!("Failed to generate query embedding: {}", e))
        })?;

        debug!(
            embedding_dim = query_embedding.len(),
            "Query embedding generated"
        );

        // 3. Search database with optional graph entity filter
        let filter = MemoryFilter::for_context(&context.app_bundle_id, &context.window_title);
        let limit = self.config.max_context_items as usize;
        let mut memories = if let Some(entity_id) =
            self.resolve_entity_filter(context, query).await
        {
            let filtered = self
                .database
                .get_memories_for_entity(&entity_id, limit)
                .await?;
            if filtered.is_empty() {
                self.database
                    .search_memories(&query_embedding, &filter, limit)
                    .await?
            } else {
                filtered
            }
        } else {
            self.database
                .search_memories(&query_embedding, &filter, limit)
                .await?
        };

        debug!(memories_found = memories.len(), "Database search completed");

        // 4. Filter by similarity threshold
        let original_count = memories.len();
        memories.retain(|m| m.similarity_score.unwrap_or(0.0) >= self.config.similarity_threshold);

        let filtered_count = memories.len();
        if filtered_count < original_count {
            debug!(
                original_count = original_count,
                filtered_count = filtered_count,
                threshold = self.config.similarity_threshold,
                "Filtered memories by similarity threshold"
            );
        }

        info!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            memories_count = filtered_count,
            threshold = self.config.similarity_threshold,
            "Memory retrieval completed"
        );

        // 5. Results are already sorted by similarity (descending) from database
        Ok(memories)
    }

    /// Retrieve memories with custom limit
    ///
    /// Same as `retrieve_memories` but with a custom limit instead of config.max_context_items.
    /// This is useful for AI-based retrieval where we need more candidates.
    ///
    /// # Arguments
    /// * `context` - Context anchor (app + window)
    /// * `query` - User query text
    /// * `limit` - Maximum number of memories to return
    ///
    /// # Returns
    /// * `Result<Vec<MemoryEntry>>` - List of relevant memories with similarity scores
    pub async fn retrieve_memories_with_limit(
        &self,
        context: &ContextAnchor,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();
        debug!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            query_len = query.len(),
            limit = limit,
            "Starting memory retrieval with custom limit"
        );

        // 1. Check if memory is enabled
        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        // 2. Generate query embedding
        debug!("Generating query embedding");
        let query_embedding = self.embedder.embed(query).await.map_err(|e| {
            warn!(error = %e, "Failed to generate query embedding");
            AlephError::config(format!("Failed to generate query embedding: {}", e))
        })?;

        debug!(
            embedding_dim = query_embedding.len(),
            "Query embedding generated"
        );

        // 3. Search database with optional graph entity filter and custom limit
        let filter = MemoryFilter::for_context(&context.app_bundle_id, &context.window_title);
        let mut memories = if let Some(entity_id) =
            self.resolve_entity_filter(context, query).await
        {
            let filtered = self
                .database
                .get_memories_for_entity(&entity_id, limit)
                .await?;
            if filtered.is_empty() {
                self.database
                    .search_memories(&query_embedding, &filter, limit)
                    .await?
            } else {
                filtered
            }
        } else {
            self.database
                .search_memories(&query_embedding, &filter, limit)
                .await?
        };

        debug!(memories_found = memories.len(), "Database search completed");

        // 4. Filter by similarity threshold
        let original_count = memories.len();
        memories.retain(|m| m.similarity_score.unwrap_or(0.0) >= self.config.similarity_threshold);

        let filtered_count = memories.len();
        if filtered_count < original_count {
            debug!(
                original_count = original_count,
                filtered_count = filtered_count,
                threshold = self.config.similarity_threshold,
                "Filtered memories by similarity threshold"
            );
        }

        info!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            memories_count = filtered_count,
            limit = limit,
            threshold = self.config.similarity_threshold,
            "Memory retrieval with custom limit completed"
        );

        // 5. Results are already sorted by similarity (descending) from database
        Ok(memories)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::LanceMemoryBackend;
    use uuid::Uuid;

    fn create_test_db() -> MemoryBackend {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_retrieval_{}", Uuid::new_v4()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        Arc::new(rt.block_on(LanceMemoryBackend::open_or_create(&db_path)).unwrap())
    }

    fn create_test_model() -> Arc<SmartEmbedder> {
        let cache_dir = SmartEmbedder::default_cache_dir().unwrap();
        Arc::new(SmartEmbedder::new(cache_dir, 300))
    }

    fn create_test_config() -> Arc<MemoryConfig> {
        let mut config = MemoryConfig::default();
        config.similarity_threshold = 0.0; // Accept all similarities for testing
        Arc::new(config)
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_retrieval_creation() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let _retrieval = MemoryRetrieval::new(db, model, config);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_retrieve_empty_database() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let retrieval = MemoryRetrieval::new(db, model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        let memories = retrieval
            .retrieve_memories(&context, "any query")
            .await
            .unwrap();

        assert!(memories.is_empty());
    }

    // NOTE: Tests that require MemoryIngestion (which still uses StateDatabase)
    // have been temporarily removed. They will be restored when MemoryIngestion
    // is migrated to MemoryBackend in Phase 5.

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_retrieve_when_disabled() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.enabled = false;
        let config = Arc::new(config);

        let retrieval = MemoryRetrieval::new(db, model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        let memories = retrieval
            .retrieve_memories(&context, "any query")
            .await
            .unwrap();

        assert!(memories.is_empty());
    }

}

