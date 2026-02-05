//! Tool Index Coordinator - synchronizes tools with Memory system
//!
//! The coordinator is responsible for:
//! - Adding/updating tools as MemoryFacts with FactType::Tool
//! - Removing tools by invalidating their facts
//! - Bulk synchronization of tools
//! - Retrieving all valid tool facts

use crate::error::AlephError;
use crate::memory::context::{FactSpecificity, FactType, MemoryFact, TemporalScope};
use crate::memory::database::VectorDatabase;
use super::inference::SemanticPurposeInferrer;
use std::sync::Arc;

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
    /// Pre-computed embedding vector (384-dim)
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
    db: Arc<VectorDatabase>,
    inferrer: SemanticPurposeInferrer,
}

impl ToolIndexCoordinator {
    /// Create a new coordinator with a database reference
    pub fn new(db: Arc<VectorDatabase>) -> Self {
        Self {
            db,
            inferrer: SemanticPurposeInferrer::new(),
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
            self.db.update_fact_content(&fact_id, &content, embedding.as_deref()).await?;
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
            };

            self.db.insert_fact(fact).await?;
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
    pub async fn get_tool_facts(&self) -> Result<Vec<MemoryFact>, AlephError> {
        // Use a large limit to get all tools (typical systems have <100 tools)
        self.db.get_facts_by_type(FactType::Tool, 1000).await
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
