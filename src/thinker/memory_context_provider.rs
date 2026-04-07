//! Async memory context provider — fetches relevant memories before prompt assembly.
//!
//! PromptLayer::inject() is sync, so we pre-fetch SQLite results here
//! and store them in MemoryContext for the layer to format.

use super::memory_context::MemoryContext;
use crate::gateway::agent_env::AgentEnvFilter;
use crate::memory::store::types::{ScoredFact, SearchFilter};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use tracing::{debug, warn};

/// Configuration for memory context retrieval.
pub struct MemoryContextConfig {
    /// Maximum number of facts to retrieve.
    pub max_facts: usize,
    /// Minimum cosine similarity threshold.
    pub similarity_threshold: f32,
    /// Maximum characters for the formatted output.
    pub max_output_chars: usize,
}

impl Default for MemoryContextConfig {
    fn default() -> Self {
        Self {
            max_facts: 5,
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
    pub fn new(memory_db: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
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
    /// When `session_id` is provided, memory search is scoped to that session.
    /// Returns empty context on any failure (never blocks LLM calls).
    pub async fn fetch(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
    ) -> MemoryContext {
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

        let _ = session_id; // no longer used — raw memory search removed

        // 2. Search facts (scoped to agent workspace)
        let facts = self.search_facts(&embedding, dim, agent_id).await;

        // 3. Build context (raw memory search removed)
        let mut ctx = MemoryContext {
            facts: facts.unwrap_or_default(),
            memory_summaries: Vec::new(),
            structured_index: None,
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
        let filter =
            SearchFilter::new().with_agent_filter(AgentEnvFilter::Single(agent_id.to_string()));
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

    fn truncate_to_budget(&self, ctx: &mut MemoryContext) {
        // Remove memories first (lower priority), then facts
        while ctx.format_for_prompt().len() > self.config.max_output_chars
            && !ctx.memory_summaries.is_empty()
        {
            ctx.memory_summaries.pop();
        }
        while ctx.format_for_prompt().len() > self.config.max_output_chars && !ctx.facts.is_empty()
        {
            ctx.facts.pop();
        }
    }
}
