//! Memory explore tool — multi-hop knowledge exploration via `RippleTask`
//!
//! Embeds a query, retrieves seed facts by vector similarity, loads their
//! embeddings, then uses `RippleTask` BFS to discover related facts across
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

const fn default_max_hops() -> usize {
    2
}

const fn default_max_per_hop() -> usize {
    5
}

// ── Args / Output ───────────────────────────────────────────────────────────

/// Arguments for `memory_explore` tool
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
    /// Typed outgoing entity-graph edges, formatted "`type→to_note`" (Gap A).
    /// Empty for notes with no typed relations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<String>,
}

/// Output from `memory_explore` tool
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

/// Multi-hop knowledge exploration tool backed by `RippleTask`
pub struct MemoryExploreTool {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    agent_id: String,
}

impl MemoryExploreTool {
    /// Create a new `MemoryExploreTool`
    pub fn new(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            database,
            embedder,
            agent_id: DEFAULT_AGENT_ID.to_string(),
        }
    }

    /// Create a new `MemoryExploreTool` with an explicit agent ID
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

        // Per-run agent id: prefer the turn's task-local (set by the dispatch
        // chokepoint), fall back to the construction-time field on non-scoped
        // paths (direct calls, tests). Mirrors `MemorySearchTool::call_impl` — the
        // tool instance is process-wide and shared across agents, so the field is
        // only a fallback, not the truth per run.
        let agent_id =
            crate::tools::turn_context::current_agent_id().unwrap_or_else(|| self.agent_id.clone());

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
            .map_err(|e| ToolError::Execution(format!("Embedding failed: {e}")))?;

        let dim_hint = embedding.len() as u32;

        // Step 2: Vector search for seed notes via NoteStore
        let seed_results = self
            .database
            .vector_search_notes_with_content(&embedding, &agent_id, dim_hint, 3)
            .await
            .map_err(|e| ToolError::Execution(format!("Seed search failed: {e}")))?;

        debug!(seed_count = seed_results.len(), "Seed notes retrieved");

        // Convert NoteSearchResult to MemoryFact with embedding attached for BFS
        let mut seed_facts: Vec<_> = seed_results
            .iter()
            .map(|r| {
                let mut f = r.to_memory_fact(&agent_id);
                // Raw vec0 L2 distance (lower = closer) → higher-is-better
                // similarity in (0, 1], so the value the model sees as
                // `relevance_score` and the BFS similarity gate are correct.
                f.similarity_score = Some(1.0 / (1.0 + r.score.max(0.0)));
                f
            })
            .collect();

        // Step 3: Load embeddings for each seed so RippleTask can expand them
        for fact in &mut seed_facts {
            match self
                .database
                .get_embedding(&fact.id, &agent_id, dim_hint)
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
        let mut seed_output: Vec<ExploredFact> = Vec::with_capacity(seed_facts.len());
        for f in &seed_facts {
            let relations = self.edge_labels(&f.id, &agent_id).await;
            seed_output.push(ExploredFact {
                id: f.id.clone(),
                content: f.content.clone(),
                path: f.path.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
                relations,
            });
        }

        // Step 4: Create RippleTask and explore
        let config = RippleConfig {
            max_hops,
            max_facts_per_hop: max_per_hop,
            similarity_threshold: 0.7,
        };
        let ripple = RippleTask::new(self.database.clone(), config, &agent_id);

        let result = ripple
            .explore(seed_facts)
            .await
            .map_err(|e| ToolError::Execution(format!("Ripple explore failed: {e}")))?;

        // Step 5: Convert expanded facts to output format
        let mut expanded_output: Vec<ExploredFact> =
            Vec::with_capacity(result.expanded_facts.len());
        for f in &result.expanded_facts {
            let relations = self.edge_labels(&f.id, &agent_id).await;
            expanded_output.push(ExploredFact {
                id: f.id.clone(),
                content: f.content.clone(),
                path: f.path.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
                relations,
            });
        }

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

    /// Look up typed outgoing edges for a note path and format them as
    /// "`type→to_note`" labels. Best-effort: a lookup error yields no labels.
    async fn edge_labels(&self, note_path: &str, agent_id: &str) -> Vec<String> {
        match self.database.get_typed_relations(note_path, agent_id).await {
            Ok(edges) => edges
                .into_iter()
                .map(|(to, ty)| format!("{ty}→{to}"))
                .collect(),
            Err(e) => {
                debug!(note_path = %note_path, error = %e, "edge_labels lookup failed");
                Vec::new()
            }
        }
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
            relations: vec![],
        };
        let json = serde_json::to_string(&fact).unwrap();
        assert!(json.contains("abc-123"));
        assert!(json.contains("0.85"));
    }

    #[test]
    fn explored_fact_relations_default_empty_and_skipped_in_json() {
        let fact = ExploredFact {
            id: "entity/alice".to_string(),
            content: "Alice".to_string(),
            path: "entity/alice".to_string(),
            relevance_score: 0.9,
            relations: vec![],
        };
        let json = serde_json::to_string(&fact).unwrap();
        assert!(!json.contains("relations"));

        let fact2 = ExploredFact {
            id: "entity/alice".to_string(),
            content: "Alice".to_string(),
            path: "entity/alice".to_string(),
            relevance_score: 0.9,
            relations: vec!["works_at→entity/acme".to_string()],
        };
        let json2 = serde_json::to_string(&fact2).unwrap();
        assert!(json2.contains("works_at→entity/acme"));
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
