//! Memory explore tool — multi-hop knowledge exploration via RippleTask
//!
//! Embeds a query, retrieves seed facts by vector similarity, loads their
//! embeddings, then uses RippleTask BFS to discover related facts across
//! configurable hops.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::error::ToolError;
use crate::error::Result;
use crate::memory::notes::store::NoteStore;
use crate::memory::ripple::{RippleConfig, RippleTask};
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::routing::DEFAULT_AGENT_ID;
use crate::tools::AlephTool;

// ── Defaults ────────────────────────────────────────────────────────────────

fn default_max_hops() -> usize {
    2
}

fn default_max_per_hop() -> usize {
    5
}

// ── Args / Output ───────────────────────────────────────────────────────────

/// Arguments for memory_explore tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryExploreArgs {
    /// Starting query to explore related knowledge from
    pub query: String,

    /// Maximum number of hops to follow (default 2, max 4)
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,

    /// Maximum facts to retrieve per hop (default 5, max 10)
    #[serde(default = "default_max_per_hop")]
    pub max_per_hop: usize,
}

/// A single explored fact
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExploredFact {
    /// Fact ID
    pub id: String,
    /// Fact content
    pub content: String,
    /// VFS path
    pub path: String,
    /// Relevance / similarity score
    pub relevance_score: f32,
}

/// Output from memory_explore tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryExploreOutput {
    /// Seed facts retrieved from the initial query
    pub seed_facts: Vec<ExploredFact>,
    /// Facts discovered through multi-hop exploration
    pub expanded_facts: Vec<ExploredFact>,
    /// Number of hops actually performed
    pub hops_performed: usize,
    /// Human-readable summary
    pub summary: String,
}

// ── Tool struct ─────────────────────────────────────────────────────────────

/// Multi-hop knowledge exploration tool backed by RippleTask
pub struct MemoryExploreTool {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    agent_id: String,
}

impl MemoryExploreTool {
    /// Create a new MemoryExploreTool
    pub fn new(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            database,
            embedder,
            agent_id: DEFAULT_AGENT_ID.to_string(),
        }
    }

    /// Create a new MemoryExploreTool with an explicit agent ID
    pub fn with_agent_id(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            database,
            embedder,
            agent_id: agent_id.into(),
        }
    }

    /// Internal implementation
    async fn call_impl(
        &self,
        args: MemoryExploreArgs,
    ) -> std::result::Result<MemoryExploreOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // Notify tool start
        let args_summary = format!("知识探索: {}", &args.query);
        notify_tool_start(Self::NAME, &args_summary);

        // Clamp parameters
        let max_hops = args.max_hops.min(4);
        let max_per_hop = args.max_per_hop.min(10);

        info!(
            query = %args.query,
            max_hops = max_hops,
            max_per_hop = max_per_hop,
            "Executing memory explore"
        );

        // Step 1: Embed query
        let embedding = self
            .embedder
            .embed(&args.query)
            .await
            .map_err(|e| ToolError::Execution(format!("Embedding failed: {}", e)))?;

        let dim_hint = embedding.len() as u32;

        // Step 2: Vector search for seed notes via NoteStore
        let seed_results = self
            .database
            .vector_search_notes_with_content(&embedding, &self.agent_id, dim_hint, 3)
            .await
            .map_err(|e| ToolError::Execution(format!("Seed search failed: {}", e)))?;

        debug!(seed_count = seed_results.len(), "Seed notes retrieved");

        // Convert NoteSearchResult to MemoryFact with embedding attached for BFS
        let mut seed_facts: Vec<_> = seed_results
            .iter()
            .map(|r| {
                let mut f = r.to_memory_fact(&self.agent_id);
                f.similarity_score = Some(r.score);
                f
            })
            .collect();

        // Step 3: Load embeddings for each seed so RippleTask can expand them
        for fact in &mut seed_facts {
            match self
                .database
                .get_embedding(&fact.id, &self.agent_id, dim_hint)
                .await
            {
                Ok(Some(emb)) => fact.embedding = Some(emb),
                Ok(None) => {}
                Err(e) => {
                    warn!(note_path = %fact.id, error = %e, "Failed to load embedding for seed note, skipping");
                }
            }
        }

        // Build output seed list before exploration (cloned for output)
        let seed_output: Vec<ExploredFact> = seed_facts
            .iter()
            .map(|f| ExploredFact {
                id: f.id.clone(),
                content: f.content.clone(),
                path: f.path.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
            })
            .collect();

        // Step 4: Create RippleTask and explore
        let config = RippleConfig {
            max_hops,
            max_facts_per_hop: max_per_hop,
            similarity_threshold: 0.7,
            ..Default::default()
        };
        let ripple = RippleTask::new(self.database.clone(), config, &self.agent_id);

        let result = ripple
            .explore(seed_facts)
            .await
            .map_err(|e| ToolError::Execution(format!("Ripple explore failed: {}", e)))?;

        // Step 5: Convert expanded facts to output format
        let expanded_output: Vec<ExploredFact> = result
            .expanded_facts
            .iter()
            .map(|f| ExploredFact {
                id: f.id.clone(),
                content: f.content.clone(),
                path: f.path.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
            })
            .collect();

        let hops_performed = result.total_hops;
        let summary = format!(
            "Explored {} seed facts → discovered {} related facts across {} hops",
            seed_output.len(),
            expanded_output.len(),
            hops_performed,
        );

        // Notify success
        let result_summary = format!(
            "种子 {} 条, 扩展 {} 条, {} 跳",
            seed_output.len(),
            expanded_output.len(),
            hops_performed,
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(MemoryExploreOutput {
            seed_facts: seed_output,
            expanded_facts: expanded_output,
            hops_performed,
            summary,
        })
    }
}

impl Clone for MemoryExploreTool {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            embedder: self.embedder.clone(),
            agent_id: self.agent_id.clone(),
        }
    }
}

// ── AlephTool impl ──────────────────────────────────────────────────────────

#[async_trait]
impl AlephTool for MemoryExploreTool {
    const NAME: &'static str = "memory_explore";
    const DESCRIPTION: &'static str =
        "Explore related knowledge by following semantic connections from a starting query. \
         Use when you need deeper context about a topic — discovers related facts across \
         multiple hops of similarity.";

    type Args = MemoryExploreArgs;
    type Output = MemoryExploreOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "memory_explore(query='Rust async patterns')".to_string(),
            "memory_explore(query='my health goals', max_hops=3, max_per_hop=8)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_defaults() {
        let json = r#"{"query": "hello"}"#;
        let args: MemoryExploreArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.query, "hello");
        assert_eq!(args.max_hops, 2);
        assert_eq!(args.max_per_hop, 5);
    }

    #[test]
    fn test_args_custom_values() {
        let json = r#"{"query": "test", "max_hops": 3, "max_per_hop": 8}"#;
        let args: MemoryExploreArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.max_hops, 3);
        assert_eq!(args.max_per_hop, 8);
    }

    #[test]
    fn test_explored_fact_serialization() {
        let fact = ExploredFact {
            id: "abc-123".to_string(),
            content: "Test content".to_string(),
            path: "aleph://user/test/".to_string(),
            relevance_score: 0.85,
        };
        let json = serde_json::to_string(&fact).unwrap();
        assert!(json.contains("abc-123"));
        assert!(json.contains("0.85"));
    }

    #[test]
    fn test_output_serialization() {
        let output = MemoryExploreOutput {
            seed_facts: vec![],
            expanded_facts: vec![],
            hops_performed: 2,
            summary: "Explored 0 seed facts".to_string(),
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("hops_performed"));
        assert!(json.contains("seed_facts"));
    }
}
