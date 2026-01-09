//! Memory capability strategy.
//!
//! This strategy retrieves relevant memory snippets using either:
//! - AI-based selection (when configured)
//! - Embedding-based vector similarity (default)

use crate::capability::strategy::CapabilityStrategy;
use crate::config::MemoryConfig;
use crate::error::{AetherError, Result};
use crate::memory::{ai_retrieval::AiMemoryRetriever, ContextAnchor as MemoryContextAnchor};
use crate::memory::{EmbeddingModel, MemoryRetrieval, VectorDatabase};
use crate::payload::{AgentPayload, Capability};
use crate::providers::AiProvider;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Memory capability strategy
///
/// Retrieves relevant memory snippets using vector similarity search,
/// optionally with AI-based selection for improved relevance.
pub struct MemoryStrategy {
    /// Vector database for memory storage
    memory_db: Option<Arc<VectorDatabase>>,
    /// Memory configuration
    memory_config: Option<Arc<MemoryConfig>>,
    /// AI provider for AI-based retrieval (optional)
    ai_provider: Option<Arc<dyn AiProvider>>,
    /// Enable AI-based retrieval
    use_ai_retrieval: bool,
    /// AI retrieval timeout
    ai_retrieval_timeout: Duration,
    /// Maximum candidates to send to AI
    ai_retrieval_max_candidates: u32,
    /// Fallback count when AI fails
    ai_retrieval_fallback_count: u32,
    /// Exclusion set for memory retrieval
    exclusion_set: Vec<String>,
}

impl MemoryStrategy {
    /// Create a new memory strategy
    pub fn new(
        memory_db: Option<Arc<VectorDatabase>>,
        memory_config: Option<Arc<MemoryConfig>>,
    ) -> Self {
        Self {
            memory_db,
            memory_config,
            ai_provider: None,
            use_ai_retrieval: false,
            ai_retrieval_timeout: Duration::from_millis(3000),
            ai_retrieval_max_candidates: 20,
            ai_retrieval_fallback_count: 3,
            exclusion_set: Vec::new(),
        }
    }

    /// Configure AI-based retrieval
    pub fn with_ai_retrieval(
        mut self,
        provider: Option<Arc<dyn AiProvider>>,
        enabled: bool,
        timeout: Duration,
        max_candidates: u32,
        fallback_count: u32,
    ) -> Self {
        self.ai_provider = provider;
        self.use_ai_retrieval = enabled;
        self.ai_retrieval_timeout = timeout;
        self.ai_retrieval_max_candidates = max_candidates;
        self.ai_retrieval_fallback_count = fallback_count;
        self
    }

    /// Set exclusion set for memory retrieval
    pub fn with_exclusion_set(mut self, exclusion_set: Vec<String>) -> Self {
        self.exclusion_set = exclusion_set;
        self
    }

    /// Get embedding model directory
    fn get_embedding_model_dir() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        Ok(PathBuf::from(home_dir)
            .join(".config")
            .join("aether")
            .join("models")
            .join("bge-small-zh-v1.5"))
    }
}

#[async_trait]
impl CapabilityStrategy for MemoryStrategy {
    fn capability_type(&self) -> Capability {
        Capability::Memory
    }

    fn priority(&self) -> u32 {
        0 // Memory executes first
    }

    fn is_available(&self) -> bool {
        self.memory_db.is_some() && self.memory_config.is_some()
    }

    fn validate_config(&self) -> Result<()> {
        // Check if embedding model directory exists when db is configured
        if self.memory_db.is_some() {
            match Self::get_embedding_model_dir() {
                Ok(dir) => {
                    if !dir.exists() {
                        return Err(AetherError::config(format!(
                            "Embedding model directory not found: {}",
                            dir.display()
                        )));
                    }
                }
                Err(e) => return Err(e),
            }
        }

        // If AI retrieval is enabled, verify provider is configured
        if self.use_ai_retrieval && self.ai_provider.is_none() {
            return Err(AetherError::config(
                "AI retrieval enabled but no AI provider configured",
            ));
        }

        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        // Check if database is accessible
        if let Some(db) = &self.memory_db {
            // Try to get stats to verify db is healthy
            match db.get_stats().await {
                Ok(stats) => {
                    debug!(
                        total_memories = stats.total_memories,
                        "Memory database health check passed"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Memory database health check failed");
                    return Ok(false);
                }
            }
        }
        Ok(self.is_available())
    }

    fn status_info(&self) -> std::collections::HashMap<String, String> {
        let mut info = std::collections::HashMap::new();
        info.insert("capability".to_string(), "Memory".to_string());
        info.insert("name".to_string(), "memory".to_string());
        info.insert("priority".to_string(), "0".to_string());
        info.insert("available".to_string(), self.is_available().to_string());
        info.insert("has_db".to_string(), self.memory_db.is_some().to_string());
        info.insert(
            "has_config".to_string(),
            self.memory_config.is_some().to_string(),
        );
        info.insert(
            "ai_retrieval_enabled".to_string(),
            self.use_ai_retrieval.to_string(),
        );
        info.insert(
            "has_ai_provider".to_string(),
            self.ai_provider.is_some().to_string(),
        );
        info
    }

    async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Check if memory database and config are available
        let Some(db) = &self.memory_db else {
            warn!("Memory capability requested but no memory database configured");
            return Ok(payload);
        };

        let Some(config) = &self.memory_config else {
            warn!("Memory capability requested but no memory config available");
            return Ok(payload);
        };

        let query = &payload.user_input;
        let anchor = &payload.meta.context_anchor;

        info!(
            query_length = query.len(),
            app = %anchor.app_bundle_id,
            window = ?anchor.window_title,
            use_ai_retrieval = self.use_ai_retrieval,
            "Retrieving memory snippets"
        );

        // Convert payload::ContextAnchor to memory::ContextAnchor
        let memory_anchor = MemoryContextAnchor::with_timestamp(
            anchor.app_bundle_id.clone(),
            anchor.window_title.clone().unwrap_or_default(),
            payload.meta.timestamp,
        );

        // Initialize embedding model
        let model_dir = Self::get_embedding_model_dir()?;
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Choose retrieval strategy
        let memories = if self.use_ai_retrieval {
            self.execute_ai_retrieval(
                db,
                config,
                &embedding_model,
                &memory_anchor,
                query,
            )
            .await?
        } else {
            self.execute_embedding_retrieval(db, config, &embedding_model, &memory_anchor, query)
                .await?
        };

        if memories.is_empty() {
            info!("No relevant memories found");
        } else {
            info!(count = memories.len(), "Retrieved relevant memory snippets");
        }

        // Store in payload context
        payload.context.memory_snippets = if memories.is_empty() {
            None
        } else {
            Some(memories)
        };

        Ok(payload)
    }
}

impl MemoryStrategy {
    /// Execute AI-based memory retrieval
    async fn execute_ai_retrieval(
        &self,
        db: &Arc<VectorDatabase>,
        config: &Arc<MemoryConfig>,
        embedding_model: &Arc<EmbeddingModel>,
        anchor: &MemoryContextAnchor,
        query: &str,
    ) -> Result<Vec<crate::memory::MemoryEntry>> {
        let Some(ai_provider) = &self.ai_provider else {
            warn!("AI retrieval enabled but no provider configured, falling back to embedding");
            return self
                .execute_embedding_retrieval(db, config, embedding_model, anchor, query)
                .await;
        };

        info!("Using AI-based memory retrieval");

        // First, fetch candidate memories using embedding search
        let retrieval =
            MemoryRetrieval::new(Arc::clone(db), Arc::clone(embedding_model), Arc::clone(config));

        // Get more candidates than needed for AI to select from
        let candidates = retrieval
            .retrieve_memories_with_limit(anchor, query, self.ai_retrieval_max_candidates as usize)
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "Failed to fetch memory candidates, returning empty");
                Vec::new()
            });

        if candidates.is_empty() {
            debug!("No memory candidates found for AI selection");
            return Ok(Vec::new());
        }

        // Use AI to select relevant memories
        let retriever = AiMemoryRetriever::new(Arc::clone(ai_provider))
            .with_timeout(self.ai_retrieval_timeout)
            .with_max_candidates(self.ai_retrieval_max_candidates)
            .with_fallback_count(self.ai_retrieval_fallback_count);

        retriever
            .retrieve(query, candidates, &self.exclusion_set)
            .await
            .map_err(|e| {
                warn!(error = %e, "AI memory selection failed");
                e
            })
    }

    /// Execute embedding-based memory retrieval
    async fn execute_embedding_retrieval(
        &self,
        db: &Arc<VectorDatabase>,
        config: &Arc<MemoryConfig>,
        embedding_model: &Arc<EmbeddingModel>,
        anchor: &MemoryContextAnchor,
        query: &str,
    ) -> Result<Vec<crate::memory::MemoryEntry>> {
        debug!("Using embedding-based memory retrieval");
        let retrieval =
            MemoryRetrieval::new(Arc::clone(db), Arc::clone(embedding_model), Arc::clone(config));
        retrieval.retrieve_memories(anchor, query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

    #[tokio::test]
    async fn test_memory_strategy_not_available() {
        let strategy = MemoryStrategy::new(None, None);
        assert!(!strategy.is_available());
    }

    #[tokio::test]
    async fn test_memory_strategy_execute_no_db() {
        let strategy = MemoryStrategy::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();
        assert!(result.context.memory_snippets.is_none());
    }
}
