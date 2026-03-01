//! Tool Index Coordinator - synchronizes tools with Memory system
//!
//! The coordinator is responsible for:
//! - Adding/updating tools as MemoryFacts with FactType::Tool
//! - Removing tools by invalidating their facts
//! - Bulk synchronization of tools
//! - Retrieving all valid tool facts

use crate::error::AlephError;
use crate::mcp::manager::{McpManagerEvent, McpManagerHandle};
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::skills::{SkillRegistryEvent, SkillsRegistry};
use super::inference::SemanticPurposeInferrer;
use crate::sync_primitives::Arc;
use tokio::sync::broadcast;

/// Metadata for a tool to be indexed
#[derive(Debug, Clone)]
pub struct ToolMeta {
    /// Tool name (e.g., "read_file", "search_code")
    pub name: String,
    /// Tool's existing description
    pub description: Option<String>,
    /// Tool category (e.g., "file", "search", "code")
    pub category: Option<String>,
    /// Curated semantic metadata (highest quality source)
    pub structured_meta: Option<String>,
    /// Pre-computed embedding vector
    pub embedding: Option<Vec<f32>>,
}

impl ToolMeta {
    /// Create a new ToolMeta with just a name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            category: None,
            structured_meta: None,
            embedding: None,
        }
    }

    /// Set the description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the category
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Set the structured metadata
    pub fn with_structured_meta(mut self, meta: impl Into<String>) -> Self {
        self.structured_meta = Some(meta.into());
        self
    }

    /// Set the embedding
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }
}

/// Coordinates tool synchronization with Memory system
///
/// Stores tools as MemoryFacts with FactType::Tool for semantic retrieval.
/// Uses SemanticPurposeInferrer to generate rich content descriptions.
pub struct ToolIndexCoordinator {
    db: MemoryBackend,
    inferrer: Arc<SemanticPurposeInferrer>,
}

impl ToolIndexCoordinator {
    /// Create a new coordinator with a database reference
    pub fn new(db: MemoryBackend) -> Self {
        Self {
            db,
            inferrer: Arc::new(SemanticPurposeInferrer::new()),
        }
    }

    /// Create a new coordinator with LLM support for L2 optimization
    pub fn with_llm(db: MemoryBackend, llm_provider: Arc<dyn crate::providers::AiProvider>) -> Self {
        Self {
            db,
            inferrer: Arc::new(SemanticPurposeInferrer::with_llm(llm_provider)),
        }
    }

    /// Generate a tool fact ID from tool name
    ///
    /// Uses "tool:" prefix for easy identification (e.g., "tool:read_file")
    fn tool_fact_id(name: &str) -> String {
        format!("tool:{}", name)
    }

    /// Get current timestamp in Unix seconds
    fn now_timestamp() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    /// Sync a single tool to Memory as a ToolFact
    ///
    /// Creates or updates the tool fact with inferred semantic purpose.
    /// Returns the fact ID on success.
    ///
    /// # Arguments
    /// * `name` - Tool name
    /// * `description` - Tool's existing description
    /// * `category` - Tool category
    /// * `structured_meta` - Curated semantic metadata
    /// * `embedding` - Pre-computed embedding vector
    pub async fn sync_tool(
        &self,
        name: &str,
        description: Option<&str>,
        category: Option<&str>,
        structured_meta: Option<&str>,
        embedding: Option<Vec<f32>>,
    ) -> Result<String, AlephError> {
        // Infer semantic purpose using ranked strategy (L0 -> L1)
        let inferred = self.inferrer.infer(name, description, category, structured_meta);

        // Log optimization level for observability
        tracing::info!(
            tool_name = %name,
            optimization_level = %format!("L{}", inferred.level),
            confidence = %inferred.confidence,
            "Tool indexed with semantic inference"
        );

        // Build content: inferred purpose + original description for context
        let content = if let Some(desc) = description {
            if !desc.is_empty() {
                format!("{}\n\nOriginal: {}", inferred.description, desc)
            } else {
                inferred.description.clone()
            }
        } else {
            inferred.description.clone()
        };

        let fact_id = Self::tool_fact_id(name);
        let now = Self::now_timestamp();

        // Check if fact already exists
        let existing: Option<MemoryFact> = self.db.get_fact(&fact_id).await?;

        if existing.is_some() {
            // Update existing fact
            // TODO: Handle embedding update separately
            let _ = &embedding;
            self.db.update_fact_content(&fact_id, &content).await?;
        } else {
            // Create new fact
            let fact = MemoryFact {
                id: fact_id.clone(),
                content,
                fact_type: FactType::Tool,
                embedding,
                source_memory_ids: vec![], // Tools don't have source memories
                created_at: now,
                updated_at: now,
                confidence: inferred.confidence,
                is_valid: true,
                invalidation_reason: None,
                decay_invalidated_at: None,
                specificity: FactSpecificity::Principle, // Tools are principle-level knowledge
                temporal_scope: TemporalScope::Permanent, // Tools are always available
                similarity_score: None,
                path: String::new(),
                layer: MemoryLayer::L2Detail,
                category: MemoryCategory::Patterns,
                fact_source: FactSource::Extracted,
                content_hash: String::new(),
                parent_path: String::new(),
                embedding_model: String::new(),
                namespace: "owner".to_string(),
                workspace: "default".to_string(),
                tier: crate::memory::context::MemoryTier::ShortTerm,
                scope: crate::memory::context::MemoryScope::Global,
                persona_id: None,
                strength: 1.0,
                access_count: 0,
                last_accessed_at: None,
            };

            self.db.insert_fact(&fact).await?;
        }

        // Schedule L2 optimization if needed (async, non-blocking)
        if self.inferrer.should_trigger_l2(&inferred) {
            tracing::debug!(
                tool_name = %name,
                "Scheduling L2 async optimization"
            );

            let db = Arc::clone(&self.db);
            let inferrer = Arc::clone(&self.inferrer);
            let tool_name = name.to_string();
            let tool_desc = description.map(|s| s.to_string());
            let tool_cat = category.map(|s| s.to_string());
            let tool_id = fact_id.clone();

            // Spawn background task for L2 optimization
            tokio::spawn(async move {
                match inferrer.enhance_with_llm(
                    &tool_id,
                    &tool_name,
                    tool_desc.as_deref(),
                    tool_cat.as_deref(),
                ).await {
                    Ok(l2_result) => {
                        tracing::info!(
                            tool_name = %tool_name,
                            optimization_level = "L2",
                            confidence = %l2_result.confidence,
                            "L2 optimization completed"
                        );

                        // Update fact with L2-enhanced content
                        if let Err(e) = db.update_fact_content(&tool_id, &l2_result.description).await {
                            tracing::warn!(
                                tool_name = %tool_name,
                                error = %e,
                                "Failed to update fact with L2 content"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            tool_name = %tool_name,
                            error = %e,
                            "L2 optimization failed, keeping L1 description"
                        );
                    }
                }
            });
        }

        Ok(fact_id)
    }

    /// Remove a tool from Memory by invalidating its fact
    ///
    /// Uses soft delete so the fact can be recovered if needed.
    pub async fn remove_tool(&self, name: &str) -> Result<(), AlephError> {
        let fact_id = Self::tool_fact_id(name);
        self.db.invalidate_fact(&fact_id, "Tool removed from registry").await
    }

    /// Sync multiple tools in bulk
    ///
    /// Returns the list of fact IDs that were created/updated.
    pub async fn sync_all(&self, tools: Vec<ToolMeta>) -> Result<Vec<String>, AlephError> {
        let mut fact_ids = Vec::with_capacity(tools.len());

        for tool in tools {
            let fact_id = self.sync_tool(
                &tool.name,
                tool.description.as_deref(),
                tool.category.as_deref(),
                tool.structured_meta.as_deref(),
                tool.embedding,
            ).await?;
            fact_ids.push(fact_id);
        }

        Ok(fact_ids)
    }

    /// Get all valid tool facts from Memory
    ///
    /// Returns facts ordered by updated_at descending.
    /// Tool facts are system-level and use Owner namespace.
    pub async fn get_tool_facts(&self) -> Result<Vec<MemoryFact>, AlephError> {
        use crate::memory::NamespaceScope;
        // Use a large limit to get all tools (typical systems have <100 tools)
        // Tool facts are system-level, so use Owner namespace
        self.db.get_facts_by_type(FactType::Tool, &NamespaceScope::Owner, "default", 1000).await
    }

    /// Get a specific tool fact by name
    pub async fn get_tool_fact(&self, name: &str) -> Result<Option<MemoryFact>, AlephError> {
        let fact_id = Self::tool_fact_id(name);
        self.db.get_fact(&fact_id).await
    }

    /// Check if a tool fact exists and is valid
    pub async fn tool_exists(&self, name: &str) -> Result<bool, AlephError> {
        let fact = self.get_tool_fact(name).await?;
        Ok(fact.map(|f| f.is_valid).unwrap_or(false))
    }

    // ========== Event Listeners ==========

    /// Start listening to MCP Manager events
    ///
    /// This spawns a background task that listens for MCP events and
    /// automatically re-syncs tool facts when tools change on MCP servers.
    ///
    /// Events handled:
    /// - `ServerStarted`: Re-sync tools for the started server
    /// - `ToolsChanged`: Re-sync tools for the affected server
    /// - `ServerCrashed`: Invalidate tools for the crashed server
    ///
    /// # Arguments
    /// * `mcp_handle` - Handle to the MCP Manager
    /// * `tool_provider` - Callback to get tools for a server (server_id -> Vec<ToolMeta>)
    ///
    /// # Returns
    /// A JoinHandle for the spawned task (can be used to abort the listener)
    pub fn start_mcp_listener<F>(
        self: Arc<Self>,
        mcp_handle: McpManagerHandle,
        tool_provider: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn(String) -> Vec<ToolMeta> + Send + Sync + 'static,
    {
        let mut receiver = mcp_handle.subscribe();
        let coordinator = self;
        let tool_provider = Arc::new(tool_provider);

        tokio::spawn(async move {
            tracing::info!("ToolIndexCoordinator: MCP event listener started");

            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        match &event {
                            McpManagerEvent::ServerStarted { server_id, tool_count, .. } => {
                                tracing::info!(
                                    server_id = %server_id,
                                    tool_count = %tool_count,
                                    "MCP server started, syncing tools"
                                );
                                let tools = tool_provider(server_id.clone());
                                if let Err(e) = coordinator.sync_all(tools).await {
                                    tracing::error!(
                                        error = %e,
                                        server_id = %server_id,
                                        "Failed to sync tools for started MCP server"
                                    );
                                }
                            }
                            McpManagerEvent::ToolsChanged { server_id, tool_count } => {
                                tracing::info!(
                                    server_id = %server_id,
                                    tool_count = %tool_count,
                                    "MCP tools changed, re-syncing"
                                );
                                let tools = tool_provider(server_id.clone());
                                if let Err(e) = coordinator.sync_all(tools).await {
                                    tracing::error!(
                                        error = %e,
                                        server_id = %server_id,
                                        "Failed to sync tools after MCP tools changed"
                                    );
                                }
                            }
                            McpManagerEvent::ServerCrashed { server_id, error, .. } => {
                                tracing::warn!(
                                    server_id = %server_id,
                                    error = %error,
                                    "MCP server crashed, invalidating tools"
                                );
                                // We could invalidate tools here, but for now just log
                                // The tools will be re-synced when server restarts
                            }
                            McpManagerEvent::ServerRemoved { server_id, .. } => {
                                tracing::info!(
                                    server_id = %server_id,
                                    "MCP server removed"
                                );
                                // Tools from this server should be invalidated
                                // but we need to know which tools belong to which server
                                // This would require additional metadata tracking
                            }
                            _ => {
                                // Other events (ManagerReady, ManagerShutdown, etc.)
                                // don't require tool index updates
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("MCP event channel closed, stopping listener");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            lagged = n,
                            "MCP event listener lagged, some events may have been missed"
                        );
                    }
                }
            }
        })
    }

    /// Start listening to Skill Registry events
    ///
    /// This spawns a background task that listens for skill lifecycle events
    /// and automatically re-syncs tool facts when skills are loaded/removed.
    ///
    /// Events handled:
    /// - `AllReloaded`: Re-sync all skill tools
    /// - `SkillLoaded`: Sync the single skill as a tool
    /// - `SkillRemoved`: Invalidate the skill's tool fact
    ///
    /// # Arguments
    /// * `registry` - The SkillsRegistry to listen to
    /// * `skill_to_tool` - Callback to convert skill_id -> ToolMeta
    ///
    /// # Returns
    /// A JoinHandle for the spawned task
    pub fn start_skill_listener<F>(
        self: Arc<Self>,
        registry: Arc<SkillsRegistry>,
        skill_to_tool: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn(String) -> Option<ToolMeta> + Send + Sync + 'static,
    {
        let mut receiver = registry.subscribe();
        let coordinator = self;
        let skill_to_tool = Arc::new(skill_to_tool);

        tokio::spawn(async move {
            tracing::info!("ToolIndexCoordinator: Skill event listener started");

            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        match &event {
                            SkillRegistryEvent::AllReloaded { count, skill_ids } => {
                                tracing::info!(
                                    count = %count,
                                    "Skills reloaded, syncing all skill tools"
                                );

                                let tools: Vec<ToolMeta> = skill_ids
                                    .iter()
                                    .filter_map(|id| skill_to_tool(id.clone()))
                                    .collect();

                                if let Err(e) = coordinator.sync_all(tools).await {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to sync tools after skills reload"
                                    );
                                }
                            }
                            SkillRegistryEvent::SkillLoaded { skill_id, skill_name } => {
                                tracing::info!(
                                    skill_id = %skill_id,
                                    skill_name = %skill_name,
                                    "Skill loaded, syncing as tool"
                                );

                                if let Some(tool) = skill_to_tool(skill_id.clone()) {
                                    if let Err(e) = coordinator.sync_all(vec![tool]).await {
                                        tracing::error!(
                                            error = %e,
                                            skill_id = %skill_id,
                                            "Failed to sync skill as tool"
                                        );
                                    }
                                }
                            }
                            SkillRegistryEvent::SkillRemoved { skill_id } => {
                                tracing::info!(
                                    skill_id = %skill_id,
                                    "Skill removed, invalidating tool fact"
                                );

                                // Skill tools use format "skill:{skill_id}" as tool name
                                let tool_name = format!("skill:{}", skill_id);
                                if let Err(e) = coordinator.remove_tool(&tool_name).await {
                                    tracing::error!(
                                        error = %e,
                                        skill_id = %skill_id,
                                        "Failed to remove skill tool fact"
                                    );
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("Skill event channel closed, stopping listener");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            lagged = n,
                            "Skill event listener lagged, some events may have been missed"
                        );
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_fact_id() {
        assert_eq!(ToolIndexCoordinator::tool_fact_id("read_file"), "tool:read_file");
        assert_eq!(ToolIndexCoordinator::tool_fact_id("search_code"), "tool:search_code");
    }

    #[test]
    fn test_tool_meta_builder() {
        let meta = ToolMeta::new("read_file")
            .with_description("Read file contents")
            .with_category("file")
            .with_structured_meta("Read and retrieve content from local filesystem");

        assert_eq!(meta.name, "read_file");
        assert_eq!(meta.description, Some("Read file contents".to_string()));
        assert_eq!(meta.category, Some("file".to_string()));
        assert!(meta.structured_meta.is_some());
    }

    #[test]
    fn test_now_timestamp() {
        let ts = ToolIndexCoordinator::now_timestamp();
        // Should be a reasonable Unix timestamp (after 2020)
        assert!(ts > 1577836800); // 2020-01-01
    }
}
