//! Async memory context provider — fetches relevant memories before prompt assembly.
//!
//! PromptLayer::inject() is sync, so we pre-fetch LanceDB results here
//! and store them in MemoryContext for the layer to format.

use crate::memory::EmbeddingProvider;
use crate::memory::store::{MemoryBackend, MemoryStore, SessionStore};
use crate::memory::store::types::{SearchFilter, MemoryFilter, ScoredFact};
use crate::gateway::workspace::WorkspaceFilter;
use crate::sync_primitives::Arc;
use super::memory_context::{MemoryContext, MemorySummary};
use tracing::{debug, warn};

/// Configuration for memory context retrieval.
pub struct MemoryContextConfig {
    /// Maximum number of facts to retrieve.
    pub max_facts: usize,
    /// Maximum number of memories to retrieve.
    pub max_memories: usize,
    /// Minimum cosine similarity threshold.
    pub similarity_threshold: f32,
    /// Maximum characters for the formatted output.
    pub max_output_chars: usize,
}

impl Default for MemoryContextConfig {
    fn default() -> Self {
        Self {
            max_facts: 5,
            max_memories: 5,
            similarity_threshold: 0.3,
            max_output_chars: 8000, // ~2000 tokens
        }
    }
}

/// Provides pre-fetched memory context for prompt injection.
pub struct MemoryContextProvider {
    memory_db: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: MemoryContextConfig,
}

impl MemoryContextProvider {
    /// Create a new provider.
    pub fn new(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            memory_db,
            embedder,
            config: MemoryContextConfig::default(),
        }
    }

    /// Create with custom config.
    pub fn with_config(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: MemoryContextConfig,
    ) -> Self {
        Self {
            memory_db,
            embedder,
            config,
        }
    }

    /// Fetch relevant memory context for a user query.
    ///
    /// Returns empty context on any failure (never blocks LLM calls).
    pub async fn fetch(&self, query: &str, agent_id: &str) -> MemoryContext {
        if query.trim().is_empty() {
            return MemoryContext::default();
        }

        // 1. Generate query embedding
        let embedding = match self.embedder.embed(query).await {
            Ok(emb) => emb,
            Err(e) => {
                warn!(error = %e, "Memory augmentation: embedding failed, skipping");
                return MemoryContext::default();
            }
        };

        let dim = embedding.len() as u32;

        // 2. Search facts and memories in parallel (scoped to agent workspace)
        let facts_future = self.search_facts(&embedding, dim, agent_id);
        let memories_future = self.search_memories(&embedding, agent_id);

        let (facts, memories) = tokio::join!(facts_future, memories_future);

        // 3. Build context
        let mut ctx = MemoryContext {
            facts: facts.unwrap_or_default(),
            memory_summaries: memories.unwrap_or_default(),
            ..Default::default()
        };

        // 4. Truncate to character budget
        self.truncate_to_budget(&mut ctx);

        debug!(
            facts = ctx.facts.len(),
            memories = ctx.memory_summaries.len(),
            agent_id = agent_id,
            "Memory context fetched for prompt augmentation"
        );

        ctx
    }

    async fn search_facts(
        &self,
        embedding: &[f32],
        dim: u32,
        agent_id: &str,
    ) -> Result<Vec<ScoredFact>, ()> {
        let filter = SearchFilter::new()
            .with_workspace(WorkspaceFilter::Single(agent_id.to_string()));
        self.memory_db
            .vector_search(embedding, dim, &filter, self.config.max_facts)
            .await
            .map(|mut results| {
                results.retain(|sf| sf.score >= self.config.similarity_threshold);
                results
            })
            .map_err(|e| {
                warn!(error = %e, "Memory augmentation: facts search failed");
            })
    }

    async fn search_memories(
        &self,
        embedding: &[f32],
        agent_id: &str,
    ) -> Result<Vec<MemorySummary>, ()> {
        let filter = MemoryFilter {
            workspace: Some(WorkspaceFilter::Single(agent_id.to_string())),
            ..Default::default()
        };
        self.memory_db
            .search_memories(embedding, &filter, self.config.max_memories)
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .filter(|e| e.similarity_score.unwrap_or(0.0) >= self.config.similarity_threshold)
                    .map(|e| {
                        let date = chrono::DateTime::from_timestamp(e.context.timestamp, 0)
                            .map(|dt| dt.format("%Y-%m-%d").to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        MemorySummary {
                            date,
                            user_input: truncate_str(&e.user_input, 150),
                            ai_output: truncate_str(&e.ai_output, 200),
                            score: e.similarity_score.unwrap_or(0.0),
                        }
                    })
                    .collect()
            })
            .map_err(|e| {
                warn!(error = %e, "Memory augmentation: memories search failed");
            })
    }

    fn truncate_to_budget(&self, ctx: &mut MemoryContext) {
        // Remove memories first (lower priority), then facts
        while ctx.format_for_prompt().len() > self.config.max_output_chars && !ctx.memory_summaries.is_empty() {
            ctx.memory_summaries.pop();
        }
        while ctx.format_for_prompt().len() > self.config.max_output_chars && !ctx.facts.is_empty() {
            ctx.facts.pop();
        }
    }
}

/// Truncate a string to max_chars, appending "..." if truncated.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}
