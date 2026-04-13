//! Async memory context provider — fetches relevant memories before prompt assembly.
//!
//! PromptLayer::inject() is sync, so we pre-fetch SQLite results here
//! and store them in MemoryContext for the layer to format.

use super::memory_context::MemoryContext;
use crate::config::types::memory::AssemblerConfig;
use crate::memory::assembler::envelope::{ItemSource, MemoryEnvelope};
use crate::memory::assembler::hybrid::{AiProviderReranker, LlmReranker};
use crate::memory::assembler::{
    AssemblyBudget, HybridAssembler, UserProfileLoader, WorkingMemoryAssembler,
};
use crate::memory::context::{MemoryFact, NoteType};
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::types::ScoredFact;
use crate::memory::store::MemoryBackend;
use crate::memory::{EmbeddingProvider, SqliteMemoryBackend};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
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

/// No-op reranker used when no [`AiProvider`] is supplied. Always errors →
/// `HybridAssembler` transparently falls back to the deterministic skeleton.
struct NoopReranker;

#[async_trait]
impl LlmReranker for NoopReranker {
    async fn complete(
        &self,
        _prompt: &str,
        _model: Option<&str>,
    ) -> Result<String, crate::error::AlephError> {
        Err(crate::error::AlephError::config(
            "NoopReranker: no AiProvider configured".to_string(),
        ))
    }
}

/// Provides pre-fetched memory context for prompt injection.
pub struct MemoryContextProvider {
    assembler: Arc<dyn WorkingMemoryAssembler>,
    config: MemoryContextConfig,
}

impl MemoryContextProvider {
    /// Create a provider with the legacy 2-argument signature. No
    /// [`AiProvider`] supplied → the assembler falls back to the deterministic
    /// skeleton for every turn. Use [`Self::with_provider`] to wire a real
    /// LLM reranker.
    pub fn new(memory_db: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self::with_config(memory_db, embedder, MemoryContextConfig::default())
    }

    /// Create with legacy 2-arg + custom config.
    pub fn with_config(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: MemoryContextConfig,
    ) -> Self {
        Self::assemble_default(
            memory_db,
            embedder,
            None,
            AssemblerConfig::default(),
            config,
        )
    }

    /// Create with an [`AiProvider`] so the LLM re-rank path is active.
    pub fn with_provider(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        provider: Arc<dyn AiProvider>,
        assembler_config: AssemblerConfig,
        config: MemoryContextConfig,
    ) -> Self {
        Self::assemble_default(
            memory_db,
            embedder,
            Some(provider),
            assembler_config,
            config,
        )
    }

    /// Escape hatch: bring your own pre-built assembler (for tests / Spec 2+).
    pub fn with_assembler(
        assembler: Arc<dyn WorkingMemoryAssembler>,
        config: MemoryContextConfig,
    ) -> Self {
        Self { assembler, config }
    }

    fn assemble_default(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        provider: Option<Arc<dyn AiProvider>>,
        assembler_config: AssemblerConfig,
        config: MemoryContextConfig,
    ) -> Self {
        let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("aleph")
                .join("memory")
                .join("note")
        });
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), memory_db.clone()));
        let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
        // Snapshots live under ~/.aleph/data/sessions by convention; we pass
        // whatever the `session_resume` defaults produce, falling back to the
        // memory_dir/sessions subdir if the home dir resolution fails.
        let snapshot_dir = SnapshotReader::default_path()
            .map(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| std::env::temp_dir())
                    .join(".aleph/data/sessions")
            })
            .unwrap_or_else(|| {
                memory_dir
                    .parent()
                    .map(|p| p.join("sessions"))
                    .unwrap_or(memory_dir.clone())
            });
        let snapshots = Arc::new(SnapshotReader::new(snapshot_dir));
        let profile = UserProfileLoader::new(memory_dir);
        let reranker: Arc<dyn LlmReranker> = match provider {
            Some(p) => AiProviderReranker::new(p),
            None => Arc::new(NoopReranker),
        };
        let assembler: Arc<dyn WorkingMemoryAssembler> = Arc::new(HybridAssembler::new(
            retrieval,
            snapshots,
            memory_db,
            profile,
            reranker,
            assembler_config,
        ));
        Self { assembler, config }
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

        let budget = AssemblyBudget {
            total_tokens: (self.config.max_output_chars / 4) as u32,
        };
        let envelope = match self
            .assembler
            .assemble(query, agent_id, session_id, budget)
            .await
        {
            Ok(env) => env,
            Err(e) => {
                warn!(error = %e, "assembler returned Err; falling through to empty context");
                return MemoryContext::default();
            }
        };

        let ctx =
            memory_context_from_envelope(&envelope, agent_id, self.config.similarity_threshold);
        debug!(
            facts = ctx.facts.len(),
            slots = envelope.slots.len(),
            strategy = %envelope.meta.strategy,
            agent_id = agent_id,
            "Memory context assembled for prompt augmentation"
        );
        ctx
    }
}

/// Convert an assembler-produced envelope into the legacy `MemoryContext`
/// shape so `PromptLayer::inject()` can keep its current rendering. Items
/// with `relevance < similarity_threshold` are dropped to match the previous
/// behaviour of `MemoryContextProvider::search_facts`.
fn memory_context_from_envelope(
    env: &MemoryEnvelope,
    agent_id: &str,
    similarity_threshold: f32,
) -> MemoryContext {
    let mut facts: Vec<ScoredFact> = Vec::new();
    for slot in &env.slots {
        for item in &slot.items {
            if item.relevance < similarity_threshold {
                continue;
            }
            let category = match &item.source {
                ItemSource::Note { category, .. } => category.clone(),
                ItemSource::Raw { .. } => "other".to_string(),
                ItemSource::Summary { .. } => "other".to_string(),
            };
            let note_type = NoteType::from_str_or_other(&category);
            let mut fact = MemoryFact::new(item.content.clone(), note_type, Vec::new());
            fact.id = item.id.clone();
            fact.path = item.id.clone();
            fact.agent = agent_id.to_string();
            fact.updated_at = item.updated_at;
            facts.push(ScoredFact {
                fact,
                score: item.relevance,
            });
        }
    }
    MemoryContext {
        facts,
        memory_summaries: Vec::new(),
        structured_index: None,
    }
}
