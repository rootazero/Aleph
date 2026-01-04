/// Capability Executor - Execute capabilities in priority order
///
/// This module orchestrates the execution of different capabilities (Memory, Search, MCP)
/// in a fixed priority order, enriching the AgentPayload with context data.
use crate::config::MemoryConfig;
use crate::error::{AetherError, Result};
use crate::memory::{EmbeddingModel, MemoryRetrieval, VectorDatabase};
use crate::payload::{AgentPayload, Capability};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

/// Capability executor that enriches AgentPayload with context data
///
/// Executes capabilities in priority order: Memory → Search → MCP
pub struct CapabilityExecutor {
    /// Optional memory database for vector retrieval
    memory_db: Option<Arc<VectorDatabase>>,
    /// Memory configuration
    memory_config: Option<Arc<MemoryConfig>>,
}

impl CapabilityExecutor {
    /// Create a new capability executor
    ///
    /// # Arguments
    ///
    /// * `memory_db` - Optional memory database for Memory capability
    /// * `memory_config` - Optional memory configuration
    pub fn new(
        memory_db: Option<Arc<VectorDatabase>>,
        memory_config: Option<Arc<MemoryConfig>>,
    ) -> Self {
        Self {
            memory_db,
            memory_config,
        }
    }

    /// Get embedding model directory
    fn get_embedding_model_dir() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        Ok(PathBuf::from(home_dir)
            .join(".config")
            .join("aether")
            .join("models")
            .join("all-MiniLM-L6-v2"))
    }

    /// Execute all capabilities in priority order
    ///
    /// Capabilities are sorted and executed in order: Memory → Search → MCP
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload to enrich
    ///
    /// # Returns
    ///
    /// The enriched payload with context data added
    pub async fn execute_all(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Sort capabilities by priority (Memory=0, Search=1, MCP=2)
        let capabilities = Capability::sort_by_priority(payload.config.capabilities.clone());

        info!(
            capabilities = ?capabilities,
            "Executing capabilities in priority order"
        );

        // Execute each capability in order
        for capability in capabilities {
            payload = self.execute_capability(payload, capability).await?;
        }

        Ok(payload)
    }

    /// Execute a single capability
    ///
    /// Dispatches to the appropriate executor based on capability type
    async fn execute_capability(
        &self,
        mut payload: AgentPayload,
        capability: Capability,
    ) -> Result<AgentPayload> {
        match capability {
            Capability::Memory => {
                payload = self.execute_memory(payload).await?;
            }
            Capability::Search => {
                warn!("Search capability not implemented yet (reserved for future)");
                // Future: Call search API and populate payload.context.search_results
            }
            Capability::Mcp => {
                warn!("MCP capability not implemented yet (reserved for future)");
                // Future: Call MCP client and populate payload.context.mcp_resources
            }
        }

        Ok(payload)
    }

    /// Execute Memory capability (MVP implementation)
    ///
    /// Retrieves relevant memory snippets from the vector database based on:
    /// - User input (query)
    /// - Context anchor (app + window)
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload
    ///
    /// # Returns
    ///
    /// The payload with memory_snippets populated (if any found)
    async fn execute_memory(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        use crate::memory::ContextAnchor as MemoryContextAnchor;

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
            "Retrieving memory snippets"
        );

        // Convert payload::ContextAnchor to memory::ContextAnchor
        let memory_anchor = MemoryContextAnchor::with_timestamp(
            anchor.app_bundle_id.clone(),
            anchor
                .window_title
                .clone()
                .unwrap_or_default(),
            payload.meta.timestamp,
        );

        // Initialize embedding model
        let model_dir = Self::get_embedding_model_dir()?;
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Create retrieval service
        let retrieval = MemoryRetrieval::new(
            Arc::clone(db),
            Arc::clone(&embedding_model),
            Arc::clone(config),
        );

        // Retrieve memory entries
        let memories = retrieval.retrieve_memories(&memory_anchor, query).await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

    #[tokio::test]
    async fn test_execute_all_no_capabilities() {
        let executor = CapabilityExecutor::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let result = executor.execute_all(payload).await.unwrap();
        assert!(result.context.memory_snippets.is_none());
    }

    #[tokio::test]
    async fn test_execute_all_with_search_warning() {
        let executor = CapabilityExecutor::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        // Should complete without error, just log a warning
        let result = executor.execute_all(payload).await.unwrap();
        assert!(result.context.search_results.is_none());
    }

    #[tokio::test]
    async fn test_execute_memory_no_database() {
        let executor = CapabilityExecutor::new(None, None);

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

        let result = executor.execute_all(payload).await.unwrap();
        assert!(result.context.memory_snippets.is_none());
    }

    #[tokio::test]
    async fn test_capability_priority_ordering() {
        let executor = CapabilityExecutor::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Test that capabilities are executed in order: Memory, Search, MCP
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Mcp, Capability::Memory, Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        // Should execute without error in priority order
        let result = executor.execute_all(payload).await.unwrap();

        // Verify payload structure is intact
        assert_eq!(result.user_input, "Test");
        assert_eq!(result.config.provider_name, "openai");
    }
}
