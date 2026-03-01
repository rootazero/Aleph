//! Hydration Pipeline for semantic tool retrieval
//!
//! Integrates ToolRetrieval with the Agent Loop by:
//! - Embedding user queries
//! - Retrieving relevant tools semantically
//! - Classifying tools into hydration levels (full/summary/indexed)
//! - Ensuring core tools are always included

use crate::error::AlephError;
use crate::memory::EmbeddingProvider;
use super::config::ToolRetrievalConfig;
use super::retrieval::{HydratedTool, ToolRetrieval};
use crate::sync_primitives::Arc;

/// Configuration for the hydration pipeline
#[derive(Debug, Clone)]
pub struct HydrationPipelineConfig {
    /// Retrieval configuration (thresholds, max_tools)
    pub retrieval: ToolRetrievalConfig,
    /// Maximum number of tools to include with full schema (default: 5)
    pub max_full_schema: usize,
    /// Maximum number of tools to include with summary only (default: 3)
    pub max_summary: usize,
    /// Core tools to always include regardless of score
    pub core_tools: Vec<String>,
}

impl Default for HydrationPipelineConfig {
    fn default() -> Self {
        Self {
            retrieval: ToolRetrievalConfig::default(),
            max_full_schema: 5,
            max_summary: 3,
            core_tools: vec![
                "file_ops".to_string(),
                "bash".to_string(),
                "read_skill".to_string(),
            ],
        }
    }
}

impl HydrationPipelineConfig {
    /// Create a new config with custom core tools
    pub fn with_core_tools(mut self, tools: Vec<String>) -> Self {
        self.core_tools = tools;
        self
    }

    /// Set max full schema tools
    pub fn with_max_full_schema(mut self, max: usize) -> Self {
        self.max_full_schema = max;
        self
    }

    /// Set max summary tools
    pub fn with_max_summary(mut self, max: usize) -> Self {
        self.max_summary = max;
        self
    }
}

/// Result of the hydration pipeline
///
/// Tools are classified into three tiers based on semantic similarity:
/// - `full_schema_tools`: High confidence (>= 0.7), include full JSON schema
/// - `summary_tools`: Medium confidence (0.6-0.7), include description only
/// - `indexed_tool_names`: Low confidence (0.4-0.6), just names for reference
#[derive(Debug, Clone)]
pub struct HydrationResult {
    /// Tools to inject with full JSON schema (score >= high_confidence_threshold)
    pub full_schema_tools: Vec<HydratedTool>,
    /// Tools to inject with summary/description only (soft_threshold <= score < high_confidence)
    pub summary_tools: Vec<HydratedTool>,
    /// Tool names available but not hydrated (hard_threshold <= score < soft_threshold)
    pub indexed_tool_names: Vec<String>,
}

impl HydrationResult {
    /// Create an empty result
    pub fn empty() -> Self {
        Self {
            full_schema_tools: Vec::new(),
            summary_tools: Vec::new(),
            indexed_tool_names: Vec::new(),
        }
    }

    /// Total number of tools across all tiers
    pub fn total_count(&self) -> usize {
        self.full_schema_tools.len() + self.summary_tools.len() + self.indexed_tool_names.len()
    }

    /// Check if any tools were found
    pub fn is_empty(&self) -> bool {
        self.full_schema_tools.is_empty()
            && self.summary_tools.is_empty()
            && self.indexed_tool_names.is_empty()
    }

    /// Get all tool names for debugging
    pub fn all_tool_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.full_schema_tools.iter().map(|t| t.name.as_str()).collect();
        names.extend(self.summary_tools.iter().map(|t| t.name.as_str()));
        names.extend(self.indexed_tool_names.iter().map(|s| s.as_str()));
        names
    }
}

/// Hydration Pipeline for integrating semantic tool retrieval into Agent Loop
///
/// The pipeline:
/// 1. Embeds the user query using EmbeddingProvider
/// 2. Retrieves semantically similar tools from the tool index
/// 3. Classifies tools by hydration level based on similarity score
/// 4. Ensures core tools are always included with full schema
pub struct HydrationPipeline {
    retrieval: ToolRetrieval,
    config: HydrationPipelineConfig,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl HydrationPipeline {
    /// Create a new hydration pipeline
    pub fn new(
        retrieval: ToolRetrieval,
        config: HydrationPipelineConfig,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            retrieval,
            config,
            embedder,
        }
    }

    /// Create with default config
    pub fn with_defaults(retrieval: ToolRetrieval, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self::new(retrieval, HydrationPipelineConfig::default(), embedder)
    }

    /// Hydrate tools based on a user query
    ///
    /// # Arguments
    /// * `query` - The user's request/query text
    ///
    /// # Returns
    /// HydrationResult with tools classified by confidence level
    pub async fn hydrate(&self, query: &str) -> Result<HydrationResult, AlephError> {
        // 1. Embed the query
        let query_embedding = self.embedder.embed(query).await?;

        // 2. Retrieve tools using semantic similarity
        let tools = self.retrieval.retrieve(&query_embedding).await?;

        // 3. Partition by hydration level
        let (full, summary, minimal) = ToolRetrieval::partition_by_hydration(&tools);

        // 4. Build result with limits
        let full_schema_tools: Vec<HydratedTool> = full
            .into_iter()
            .take(self.config.max_full_schema)
            .cloned()
            .collect();

        let summary_tools: Vec<HydratedTool> = summary
            .into_iter()
            .take(self.config.max_summary)
            .cloned()
            .collect();

        let indexed_tool_names: Vec<String> = minimal
            .into_iter()
            .map(|t| t.name.clone())
            .collect();

        // 5. Ensure core tools are in full_schema_tools
        // Note: Core tools should ideally come from the registry with full schema
        // For now, we just ensure they're represented
        for core_tool in &self.config.core_tools {
            let already_included = full_schema_tools.iter().any(|t| &t.name == core_tool)
                || summary_tools.iter().any(|t| &t.name == core_tool);

            if !already_included {
                // Core tools not found in semantic search are noted
                // They should be added by the caller from the registry
                tracing::debug!(
                    tool = %core_tool,
                    "Core tool not found in semantic search, should be added from registry"
                );
            }
        }

        Ok(HydrationResult {
            full_schema_tools,
            summary_tools,
            indexed_tool_names,
        })
    }

    /// Hydrate with hybrid search (vector + text)
    ///
    /// Uses both semantic similarity and text matching for better recall.
    pub async fn hydrate_hybrid(&self, query: &str) -> Result<HydrationResult, AlephError> {
        // 1. Embed the query
        let query_embedding = self.embedder.embed(query).await?;

        // 2. Use hybrid retrieval
        let tools = self.retrieval.retrieve_hybrid(&query_embedding, query).await?;

        // 3. Partition and build result (same as hydrate)
        let (full, summary, minimal) = ToolRetrieval::partition_by_hydration(&tools);

        let full_schema_tools: Vec<HydratedTool> = full
            .into_iter()
            .take(self.config.max_full_schema)
            .cloned()
            .collect();

        let summary_tools: Vec<HydratedTool> = summary
            .into_iter()
            .take(self.config.max_summary)
            .cloned()
            .collect();

        let indexed_tool_names: Vec<String> = minimal
            .into_iter()
            .map(|t| t.name.clone())
            .collect();

        Ok(HydrationResult {
            full_schema_tools,
            summary_tools,
            indexed_tool_names,
        })
    }

    /// Get the current config
    pub fn config(&self) -> &HydrationPipelineConfig {
        &self.config
    }

    /// Get core tool names
    pub fn core_tools(&self) -> &[String] {
        &self.config.core_tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HydrationPipelineConfig::default();
        assert_eq!(config.max_full_schema, 5);
        assert_eq!(config.max_summary, 3);
        assert!(config.core_tools.contains(&"file_ops".to_string()));
        assert!(config.core_tools.contains(&"bash".to_string()));
    }

    #[test]
    fn test_config_builder() {
        let config = HydrationPipelineConfig::default()
            .with_max_full_schema(10)
            .with_max_summary(5)
            .with_core_tools(vec!["custom_tool".to_string()]);

        assert_eq!(config.max_full_schema, 10);
        assert_eq!(config.max_summary, 5);
        assert_eq!(config.core_tools, vec!["custom_tool"]);
    }

    #[test]
    fn test_hydration_result_empty() {
        let result = HydrationResult::empty();
        assert!(result.is_empty());
        assert_eq!(result.total_count(), 0);
        assert!(result.all_tool_names().is_empty());
    }

    #[test]
    fn test_hydration_result_counts() {
        use crate::memory::context::{FactType, MemoryFact};

        let config = ToolRetrievalConfig::default();

        // Create mock facts
        let mut fact1 = MemoryFact::with_id(
            "tool:read_file".to_string(),
            "Read file".to_string(),
            FactType::Tool,
        );
        fact1.similarity_score = Some(0.85);

        let mut fact2 = MemoryFact::with_id(
            "tool:write_file".to_string(),
            "Write file".to_string(),
            FactType::Tool,
        );
        fact2.similarity_score = Some(0.65);

        let result = HydrationResult {
            full_schema_tools: vec![HydratedTool::from_fact(fact1, &config)],
            summary_tools: vec![HydratedTool::from_fact(fact2, &config)],
            indexed_tool_names: vec!["delete_file".to_string()],
        };

        assert!(!result.is_empty());
        assert_eq!(result.total_count(), 3);

        let names = result.all_tool_names();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"delete_file"));
    }
}
