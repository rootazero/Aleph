//! Memory search tool with hybrid retrieval and post-retrieval arbitration
//!
//! Implements AlephTool trait for AI agent integration.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

use super::error::ToolError;
use crate::error::Result;
use crate::memory::{
    ComptrollerConfig, ContextComptroller, FactRetrieval, FactRetrievalConfig, TokenBudget,
    TranscriptIndexer, VectorDatabase, SmartEmbedder,
};
use crate::tools::AlephTool;

/// Arguments for memory_search tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemorySearchArgs {
    /// Search query
    pub query: String,
    /// Max results to return (default 10)
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_max_results() -> usize {
    10
}

/// A single memory fact result
#[derive(Debug, Clone, Serialize)]
pub struct FactResult {
    pub content: String,
    pub fact_type: String,
    pub confidence: f32,
    pub similarity_score: f32,
}

/// A single transcript result
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptResult {
    pub user_input: String,
    pub ai_output: String,
    pub context: String,
    pub similarity_score: f32,
}

/// Output from memory_search tool
#[derive(Debug, Clone, Serialize)]
pub struct MemorySearchOutput {
    pub facts: Vec<FactResult>,
    pub transcripts: Vec<TranscriptResult>,
    pub query: String,
    pub tokens_saved: usize,
}

/// Memory search tool with hybrid retrieval
pub struct MemorySearchTool {
    database: Arc<VectorDatabase>,
    fact_retrieval: Arc<FactRetrieval>,
    comptroller: Arc<ContextComptroller>,
    _indexer: Arc<TranscriptIndexer>,
}

impl MemorySearchTool {
    /// Tool identifier
    pub const NAME: &'static str = "memory_search";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Search personal memory for relevant facts and conversation history. \
        Returns both compressed facts and raw transcripts with redundancy elimination.";

    /// Create a new MemorySearchTool instance
    pub fn new(database: Arc<VectorDatabase>) -> Self {
        // Create embedder with default cache dir and TTL
        let cache_dir = std::env::var("ALEPH_MODEL_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_models"));
        let embedder = Arc::new(SmartEmbedder::new(cache_dir, 300));

        let fact_config = FactRetrievalConfig {
            max_facts: 10,
            max_raw_fallback: 10,
            similarity_threshold: 0.5,
        };
        let fact_retrieval = Arc::new(FactRetrieval::new(
            database.clone(),
            (*embedder).clone(),
            fact_config,
        ));

        let comptroller_config = ComptrollerConfig::default();
        let comptroller = Arc::new(ContextComptroller::new(comptroller_config));

        let indexer = Arc::new(TranscriptIndexer::new(
            database.clone(),
            embedder.clone(),
        ));

        Self {
            database,
            fact_retrieval,
            comptroller,
            _indexer: indexer,
        }
    }

    /// Execute memory search (internal implementation)
    async fn call_impl(
        &self,
        args: MemorySearchArgs,
    ) -> std::result::Result<MemorySearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // Notify tool start
        let args_summary = format!("记忆搜索: {}", &args.query);
        notify_tool_start(Self::NAME, &args_summary);

        info!(query = %args.query, max_results = args.max_results, "Executing memory search");

        // Step 1: Fact-first retrieval
        debug!("Performing fact-first retrieval");
        let retrieval_result = self
            .fact_retrieval
            .retrieve(&args.query)
            .await
            .map_err(|e| ToolError::Execution(format!("Fact retrieval failed: {}", e)))?;

        debug!(
            facts_count = retrieval_result.facts.len(),
            transcripts_count = retrieval_result.raw_memories.len(),
            "Retrieval completed"
        );

        // Step 2: Post-retrieval arbitration
        debug!("Performing post-retrieval arbitration");
        let budget = TokenBudget::new(100000); // Large budget for MVP
        let arbitrated = self.comptroller.arbitrate(retrieval_result, budget);

        info!(
            facts = arbitrated.facts.len(),
            transcripts = arbitrated.raw_memories.len(),
            tokens_saved = arbitrated.tokens_saved,
            "Arbitration completed"
        );

        // Step 3: Convert to output format
        let facts: Vec<FactResult> = arbitrated
            .facts
            .into_iter()
            .map(|f| FactResult {
                content: f.content,
                fact_type: format!("{:?}", f.fact_type),
                confidence: f.confidence,
                similarity_score: f.similarity_score.unwrap_or(0.0),
            })
            .collect();

        let transcripts: Vec<TranscriptResult> = arbitrated
            .raw_memories
            .into_iter()
            .map(|t| TranscriptResult {
                user_input: t.user_input,
                ai_output: t.ai_output,
                context: format!("{} - {}", t.context.app_bundle_id, t.context.window_title),
                similarity_score: t.similarity_score.unwrap_or(0.0),
            })
            .collect();

        // Notify success
        let result_summary = format!(
            "找到 {} 条事实, {} 条对话记录 (节省 {} tokens)",
            facts.len(),
            transcripts.len(),
            arbitrated.tokens_saved
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(MemorySearchOutput {
            facts,
            transcripts,
            query: args.query,
            tokens_saved: arbitrated.tokens_saved,
        })
    }
}

impl Clone for MemorySearchTool {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            fact_retrieval: self.fact_retrieval.clone(),
            comptroller: self.comptroller.clone(),
            _indexer: self._indexer.clone(),
        }
    }
}

/// Implementation of AlephTool trait for MemorySearchTool
#[async_trait]
impl AlephTool for MemorySearchTool {
    const NAME: &'static str = "memory_search";
    const DESCRIPTION: &'static str = "Search personal memory for relevant facts and conversation history. \
        Returns both compressed facts and raw transcripts with redundancy elimination.";

    type Args = MemorySearchArgs;
    type Output = MemorySearchOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "memory_search(query='What are my coding preferences?', max_results=10)".to_string(),
            "memory_search(query='Previous discussions about Rust')".to_string(),
            "memory_search(query='My travel plans', max_results=5)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_search_args_serialization() {
        // Test that args can be serialized/deserialized
        let args = MemorySearchArgs {
            query: "test query".to_string(),
            max_results: 5,
        };

        let json = serde_json::to_string(&args).unwrap();
        let deserialized: MemorySearchArgs = serde_json::from_str(&json).unwrap();

        assert_eq!(args.query, deserialized.query);
        assert_eq!(args.max_results, deserialized.max_results);
    }

    #[test]
    fn test_default_max_results() {
        assert_eq!(default_max_results(), 10);
    }
}
