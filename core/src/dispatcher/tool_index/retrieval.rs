//! Tool retrieval with dual-threshold semantic search
//!
//! Implements Pre-flight Hydration with three confidence levels:
//! - High (>= 0.7): Full tool schema
//! - Medium (>= 0.6): Summary only
//! - Low (>= 0.4): Excluded or minimal

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryFact};
use crate::memory::store::{MemoryBackend, MemoryStore};
use super::config::ToolRetrievalConfig;

/// Hydration level for a retrieved tool
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HydrationLevel {
    /// Full tool schema injected into context
    Full,
    /// Only tool summary/description
    Summary,
    /// Tool is known but not hydrated
    Minimal,
}

/// A tool retrieved with hydration level information
#[derive(Debug, Clone)]
pub struct HydratedTool {
    /// Tool name (extracted from fact ID, e.g., "tool:read_file" -> "read_file")
    pub name: String,
    /// Semantic description from the tool fact
    pub description: String,
    /// Similarity score from search
    pub score: f32,
    /// Hydration level based on confidence
    pub hydration_level: HydrationLevel,
    /// The underlying memory fact
    pub fact: MemoryFact,
    /// Cached JSON schema for the tool (populated from ToolRegistry)
    pub cached_schema: Option<String>,
}

impl HydratedTool {
    /// Extract tool name from fact ID
    fn name_from_fact_id(fact_id: &str) -> String {
        fact_id.strip_prefix("tool:").unwrap_or(fact_id).to_string()
    }

    /// Create from a MemoryFact with calculated hydration level
    pub fn from_fact(fact: MemoryFact, config: &ToolRetrievalConfig) -> Self {
        let score = fact.similarity_score.unwrap_or(0.0);
        let hydration_level = Self::calculate_hydration_level(score, config);
        let name = Self::name_from_fact_id(&fact.id);

        Self {
            name,
            description: fact.content.clone(),
            score,
            hydration_level,
            fact,
            cached_schema: None,
        }
    }

    /// Set the cached schema (typically from ToolRegistry lookup)
    pub fn with_schema(mut self, schema: String) -> Self {
        self.cached_schema = Some(schema);
        self
    }

    /// Get the JSON schema, returning cached value if available
    pub fn schema_json(&self) -> Option<&str> {
        self.cached_schema.as_deref()
    }

    /// Check if schema is cached
    pub fn has_schema(&self) -> bool {
        self.cached_schema.is_some()
    }

    /// Calculate hydration level based on score and config thresholds
    fn calculate_hydration_level(score: f32, config: &ToolRetrievalConfig) -> HydrationLevel {
        if score >= config.high_confidence_threshold {
            HydrationLevel::Full
        } else if score >= config.soft_threshold {
            HydrationLevel::Summary
        } else {
            HydrationLevel::Minimal
        }
    }
}

/// Retrieves tools using semantic search with dual-threshold logic
pub struct ToolRetrieval {
    db: MemoryBackend,
    config: ToolRetrievalConfig,
}

impl ToolRetrieval {
    /// Create a new ToolRetrieval with custom config
    pub fn new(db: MemoryBackend, config: ToolRetrievalConfig) -> Self {
        Self { db, config }
    }

    /// Create with default config
    pub fn with_defaults(db: MemoryBackend) -> Self {
        Self::new(db, ToolRetrievalConfig::default())
    }

    /// Retrieve relevant tools for a query embedding
    ///
    /// Returns tools above the hard threshold, classified by hydration level.
    /// Results are sorted by score descending.
    pub async fn retrieve(
        &self,
        query_embedding: &[f32],
    ) -> Result<Vec<HydratedTool>, AlephError> {
        // Search for facts using vector similarity
        // We fetch more than max_tools to allow for filtering by type
        let candidate_limit = self.config.max_tools * 3;
        let filter = crate::memory::store::types::SearchFilter::valid_only(
            Some(crate::memory::NamespaceScope::Owner),
        );
        let scored = self.db.vector_search(
            query_embedding,
            crate::memory::EMBEDDING_DIM as u32,
            &filter,
            candidate_limit,
        ).await?;
        let facts: Vec<MemoryFact> = scored.into_iter().map(|sf| {
            let mut fact = sf.fact;
            fact.similarity_score = Some(sf.score);
            fact
        }).collect();

        // Filter to only tool facts and apply hard threshold
        let mut tools: Vec<HydratedTool> = facts
            .into_iter()
            .filter(|f| f.fact_type == FactType::Tool)
            .filter(|f| f.similarity_score.unwrap_or(0.0) >= self.config.hard_threshold)
            .map(|f| HydratedTool::from_fact(f, &self.config))
            .collect();

        // Sort by score descending (highest confidence first)
        tools.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Limit to max_tools
        tools.truncate(self.config.max_tools);

        Ok(tools)
    }

    /// Retrieve tools with hybrid search (vector + text)
    ///
    /// Useful when you have both embedding and natural language query
    pub async fn retrieve_hybrid(
        &self,
        query_embedding: &[f32],
        query_text: &str,
    ) -> Result<Vec<HydratedTool>, AlephError> {
        // Use hybrid search from StateDatabase
        let filter = crate::memory::store::types::SearchFilter::valid_only(
            Some(crate::memory::NamespaceScope::Owner),
        );
        let scored = self.db.hybrid_search(
            query_embedding,
            crate::memory::EMBEDDING_DIM as u32,
            query_text,
            0.7, // vector weight
            0.3, // text weight
            &filter,
            self.config.max_tools * 3,
        ).await?;
        let facts: Vec<MemoryFact> = scored.into_iter().map(|sf| {
            let mut fact = sf.fact;
            fact.similarity_score = Some(sf.score);
            fact
        }).collect();

        let mut tools: Vec<HydratedTool> = facts
            .into_iter()
            .filter(|f| f.fact_type == FactType::Tool)
            .map(|f| HydratedTool::from_fact(f, &self.config))
            .collect();

        tools.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        tools.truncate(self.config.max_tools);

        Ok(tools)
    }

    /// Get tools at each hydration level
    pub fn partition_by_hydration(tools: &[HydratedTool]) -> (Vec<&HydratedTool>, Vec<&HydratedTool>, Vec<&HydratedTool>) {
        let full: Vec<_> = tools.iter().filter(|t| t.hydration_level == HydrationLevel::Full).collect();
        let summary: Vec<_> = tools.iter().filter(|t| t.hydration_level == HydrationLevel::Summary).collect();
        let minimal: Vec<_> = tools.iter().filter(|t| t.hydration_level == HydrationLevel::Minimal).collect();
        (full, summary, minimal)
    }

    /// Get the current config
    pub fn config(&self) -> &ToolRetrievalConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hydration_level_full() {
        let config = ToolRetrievalConfig::default();
        let level = HydratedTool::calculate_hydration_level(0.75, &config);
        assert_eq!(level, HydrationLevel::Full);
    }

    #[test]
    fn test_hydration_level_summary() {
        let config = ToolRetrievalConfig::default();
        let level = HydratedTool::calculate_hydration_level(0.65, &config);
        assert_eq!(level, HydrationLevel::Summary);
    }

    #[test]
    fn test_hydration_level_minimal() {
        let config = ToolRetrievalConfig::default();
        let level = HydratedTool::calculate_hydration_level(0.45, &config);
        assert_eq!(level, HydrationLevel::Minimal);
    }

    #[test]
    fn test_hydration_at_boundaries() {
        let config = ToolRetrievalConfig::default();

        // At high threshold (0.7)
        assert_eq!(HydratedTool::calculate_hydration_level(0.7, &config), HydrationLevel::Full);

        // Just below high threshold
        assert_eq!(HydratedTool::calculate_hydration_level(0.69, &config), HydrationLevel::Summary);

        // At soft threshold (0.6)
        assert_eq!(HydratedTool::calculate_hydration_level(0.6, &config), HydrationLevel::Summary);

        // Just below soft threshold
        assert_eq!(HydratedTool::calculate_hydration_level(0.59, &config), HydrationLevel::Minimal);
    }

    #[test]
    fn test_name_from_fact_id() {
        assert_eq!(HydratedTool::name_from_fact_id("tool:read_file"), "read_file");
        assert_eq!(HydratedTool::name_from_fact_id("tool:search_code"), "search_code");
        assert_eq!(HydratedTool::name_from_fact_id("not_a_tool"), "not_a_tool");
    }

    #[test]
    fn test_partition_by_hydration() {
        let config = ToolRetrievalConfig::default();

        // Create mock facts with different scores
        let mut fact_full = MemoryFact::with_id(
            "tool:read_file".to_string(),
            "Read contents of a file".to_string(),
            FactType::Tool,
        );
        fact_full.similarity_score = Some(0.85);

        let mut fact_summary = MemoryFact::with_id(
            "tool:write_file".to_string(),
            "Write contents to a file".to_string(),
            FactType::Tool,
        );
        fact_summary.similarity_score = Some(0.65);

        let mut fact_minimal = MemoryFact::with_id(
            "tool:delete_file".to_string(),
            "Delete a file".to_string(),
            FactType::Tool,
        );
        fact_minimal.similarity_score = Some(0.45);

        let tools = vec![
            HydratedTool::from_fact(fact_full, &config),
            HydratedTool::from_fact(fact_summary, &config),
            HydratedTool::from_fact(fact_minimal, &config),
        ];

        let (full, summary, minimal) = ToolRetrieval::partition_by_hydration(&tools);

        assert_eq!(full.len(), 1);
        assert_eq!(full[0].name, "read_file");

        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].name, "write_file");

        assert_eq!(minimal.len(), 1);
        assert_eq!(minimal[0].name, "delete_file");
    }

    #[test]
    fn test_hydrated_tool_from_fact() {
        let config = ToolRetrievalConfig::default();

        let mut fact = MemoryFact::with_id(
            "tool:execute_shell".to_string(),
            "Execute shell commands in a sandboxed environment".to_string(),
            FactType::Tool,
        );
        fact.similarity_score = Some(0.72);

        let tool = HydratedTool::from_fact(fact.clone(), &config);

        assert_eq!(tool.name, "execute_shell");
        assert_eq!(tool.description, "Execute shell commands in a sandboxed environment");
        assert_eq!(tool.score, 0.72);
        assert_eq!(tool.hydration_level, HydrationLevel::Full);
        assert_eq!(tool.fact.id, "tool:execute_shell");
    }
}
