//! Async memory context provider — fetches relevant memories before prompt assembly.
//!
//! PromptLayer::inject() is sync, so we pre-fetch SQLite results here
//! and store them in MemoryContext for the layer to format.

use super::memory_context::MemoryContext;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::types::ScoredFact;
use crate::memory::store::MemoryBackend;
use crate::memory::{EmbeddingProvider, SqliteMemoryBackend};
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
    note_retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
    config: MemoryContextConfig,
}

impl MemoryContextProvider {
    /// Create a new provider.
    pub fn new(memory_db: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self::with_config(memory_db, embedder, MemoryContextConfig::default())
    }

    /// Create with custom config.
    pub fn with_config(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: MemoryContextConfig,
    ) -> Self {
        let memory_dir = crate::utils::paths::get_note_memory_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("aleph").join("memory").join("note"));
        // MemoryBackend is Arc<SqliteMemoryBackend>, so we can pass it directly.
        let indexer = Arc::new(NoteIndexer::new(memory_dir, memory_db));
        let note_retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
        Self {
            note_retrieval,
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

        let _ = session_id; // no longer used — raw memory search removed

        // Search facts via NoteFactRetrieval (scoped to agent workspace)
        let facts = self.search_facts(query, agent_id).await;

        // Build context (raw memory search removed)
        let mut ctx = MemoryContext {
            facts: facts.unwrap_or_default(),
            memory_summaries: Vec::new(),
            structured_index: None,
        };

        // Truncate to character budget
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
        query: &str,
        agent_id: &str,
    ) -> Result<Vec<ScoredFact>, ()> {
        self.note_retrieval
            .vector_retrieve(query, agent_id, self.config.max_facts)
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
