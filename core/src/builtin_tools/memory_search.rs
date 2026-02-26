//! Memory search tool with hybrid retrieval and post-retrieval arbitration
//!
//! Implements AlephTool trait for AI agent integration.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use super::error::ToolError;
use crate::error::Result;
use crate::memory::store::MemoryBackend;
use crate::memory::{
    ComptrollerConfig, ContextComptroller, EmbeddingProvider, FactRetrieval, FactRetrievalConfig,
    TokenBudget, TranscriptIndexer,
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
    /// Workspace to search in (defaults to "default")
    #[serde(default)]
    pub workspace: Option<String>,
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
    pub path: String,
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
    pub path_clusters: Vec<PathCluster>,
}

/// A cluster of facts under the same VFS path
#[derive(Debug, Clone, Serialize)]
pub struct PathCluster {
    pub path: String,
    pub l1_overview: Option<String>,
    pub fact_count: usize,
    pub top_score: f32,
}

/// Group facts by path, returning clusters where count >= threshold
fn cluster_facts_by_path(facts: &[FactResult], threshold: usize) -> Vec<PathCluster> {
    use std::collections::HashMap;

    let mut groups: HashMap<&str, (usize, f32)> = HashMap::new();
    for fact in facts {
        if fact.path.is_empty() {
            continue;
        }
        let entry = groups.entry(&fact.path).or_insert((0, 0.0));
        entry.0 += 1;
        if fact.similarity_score > entry.1 {
            entry.1 = fact.similarity_score;
        }
    }

    groups.into_iter()
        .filter(|(_, (count, _))| *count >= threshold)
        .map(|(path, (count, top_score))| PathCluster {
            path: path.to_string(),
            l1_overview: None,
            fact_count: count,
            top_score,
        })
        .collect()
}

/// Memory search tool with hybrid retrieval
pub struct MemorySearchTool {
    database: MemoryBackend,
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
    pub fn new_with_embedder(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        let fact_config = FactRetrievalConfig {
            max_facts: 10,
            max_raw_fallback: 10,
            similarity_threshold: 0.5,
        };
        let fact_retrieval = Arc::new(FactRetrieval::new(
            database.clone(),
            Arc::clone(&embedder),
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

        let workspace = args.workspace.as_deref().unwrap_or("default");

        // Notify tool start
        let args_summary = format!("记忆搜索: {}", &args.query);
        notify_tool_start(Self::NAME, &args_summary);

        info!(query = %args.query, max_results = args.max_results, workspace = workspace, "Executing memory search");

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
                path: f.path.clone(),
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

        // Step 3b: Compute path clusters
        let mut path_clusters = cluster_facts_by_path(&facts, 3);
        for cluster in &mut path_clusters {
            // Try to load L1 overview from store via get_by_path
            if let Ok(Some(l1)) = crate::memory::store::MemoryStore::get_by_path(
                &*self.database,
                &cluster.path,
                &crate::memory::NamespaceScope::Owner,
                workspace,
            ).await {
                if l1.fact_source == crate::memory::FactSource::Summary {
                    cluster.l1_overview = Some(l1.content);
                }
            }
        }

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
            path_clusters,
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
            workspace: None,
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

    #[test]
    fn test_path_cluster_serialization() {
        let cluster = PathCluster {
            path: "aleph://user/preferences/coding/".to_string(),
            l1_overview: Some("Overview text".to_string()),
            fact_count: 5,
            top_score: 0.85,
        };
        let json = serde_json::to_string(&cluster).unwrap();
        assert!(json.contains("aleph://user/preferences/coding/"));
        assert!(json.contains("Overview text"));
    }

    #[test]
    fn test_cluster_facts_by_path() {
        let facts = vec![
            FactResult {
                content: "Fact 1".into(),
                fact_type: "Preference".into(),
                confidence: 0.9,
                similarity_score: 0.8,
                path: "aleph://user/preferences/coding/".into(),
            },
            FactResult {
                content: "Fact 2".into(),
                fact_type: "Preference".into(),
                confidence: 0.85,
                similarity_score: 0.75,
                path: "aleph://user/preferences/coding/".into(),
            },
            FactResult {
                content: "Fact 3".into(),
                fact_type: "Preference".into(),
                confidence: 0.8,
                similarity_score: 0.7,
                path: "aleph://user/preferences/coding/".into(),
            },
            FactResult {
                content: "Fact 4".into(),
                fact_type: "Learning".into(),
                confidence: 0.9,
                similarity_score: 0.6,
                path: "aleph://knowledge/learning/".into(),
            },
        ];

        let clusters = cluster_facts_by_path(&facts, 3);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].path, "aleph://user/preferences/coding/");
        assert_eq!(clusters[0].fact_count, 3);
    }
}
