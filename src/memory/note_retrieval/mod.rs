//! Note-based retrieval engine.
//!
//! Drop-in replacement for `FactRetrieval` that queries notes (markdown + SQLite index)
//! instead of the legacy facts table. Returns `Vec<ScoredFact>` so downstream
//! consumers don't require changes.

pub mod hybrid;

use crate::error::AlephError;
use crate::memory::context::{MemoryFact, MemoryScope, MemoryTier, NoteType};
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::notes::NoteIndexer;
use crate::memory::store::types::ScoredFact;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

/// Notes-based retrieval engine. Drop-in replacement for FactRetrieval.
pub struct NoteFactRetrieval<S: NoteStore + Send + Sync + 'static> {
    indexer: Arc<NoteIndexer<S>>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl<S: NoteStore + Send + Sync + 'static> NoteFactRetrieval<S> {
    pub fn new(
        indexer: Arc<NoteIndexer<S>>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self { indexer, embedder }
    }

    /// Hybrid vector + FTS search with RRF fusion.
    /// Returns ScoredFact for downstream compatibility.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        let results = self.indexer.store()
            .hybrid_search_notes(&embedding, query, agent_id, dim, limit)
            .await?;

        Ok(results.iter().map(|r| r.to_scored_fact(agent_id)).collect())
    }

    /// Pure vector similarity search.
    pub async fn vector_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        let results = self.indexer.store()
            .vector_search_notes_with_content(&embedding, agent_id, dim, limit)
            .await?;

        Ok(results.iter().map(|r| r.to_scored_fact(agent_id)).collect())
    }

    /// FTS-only search (no embedding required).
    /// Note: FTS results don't carry scores natively — rank-based scores are assigned.
    pub async fn text_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let entries = self.indexer.store()
            .search_notes_fts(query, agent_id, limit)
            .await?;

        let total = entries.len() as f32;
        Ok(entries.iter().enumerate().map(|(i, entry)| {
            scored_fact_from_index_entry(entry, agent_id, 1.0 - (i as f32 / total.max(1.0)))
        }).collect())
    }

    /// Hybrid search across multiple agents. Results from each agent are
    /// collected, merged, sorted by score, and truncated to `limit`.
    ///
    /// Used for "smart recall" — queries that should span multiple workspaces.
    pub async fn retrieve_multi_agent(
        &self,
        query: &str,
        agent_ids: &[String],
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        if agent_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Embed once, reuse across agents
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        let mut all_results: Vec<ScoredFact> = Vec::new();
        // Over-fetch per agent so merged top-k is well-populated
        let per_agent_limit = limit.max(10);

        for agent_id in agent_ids {
            let results = self
                .indexer
                .store()
                .hybrid_search_notes(&embedding, query, agent_id, dim, per_agent_limit)
                .await?;
            for r in results {
                all_results.push(r.to_scored_fact(agent_id));
            }
        }

        // Sort by score DESC, take top-k
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(limit);
        Ok(all_results)
    }

    /// Discover all agent IDs by listing subdirectories of the memory dir,
    /// then retrieve across all of them.
    ///
    /// Returns empty if no agents or memory dir is unreadable.
    pub async fn retrieve_all_agents(
        &self,
        query: &str,
        memory_dir: &std::path::Path,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let agent_ids = discover_agent_ids(memory_dir).await;
        if agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.retrieve_multi_agent(query, &agent_ids, limit).await
    }
}

/// Discover agent IDs by reading directory names under memory_dir.
async fn discover_agent_ids(memory_dir: &std::path::Path) -> Vec<String> {
    let mut agents = Vec::new();
    let mut dir = match tokio::fs::read_dir(memory_dir).await {
        Ok(d) => d,
        Err(_) => return agents,
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if let Ok(ft) = entry.file_type().await {
            if ft.is_dir() {
                agents.push(name);
            }
        }
    }
    agents
}

/// Build a `ScoredFact` from a lightweight `NoteIndexEntry`.
///
/// Content is not available in index entries — the `content` field is left empty.
/// Callers that need full content should use the vector or hybrid search paths.
fn scored_fact_from_index_entry(entry: &NoteIndexEntry, agent_id: &str, score: f32) -> ScoredFact {
    let note_type = NoteType::from_str_or_other(&entry.category);
    let mut fact = MemoryFact::new(
        String::new(), // content not stored in index entries
        note_type,
        entry.tags.clone(),
    );
    fact.id = entry.path.clone();
    fact.path = format!("note://{}", entry.path);
    fact.agent = agent_id.to_string();
    fact.created_at = entry.created_at;
    fact.updated_at = entry.updated_at;
    fact.confidence = score;
    fact.is_valid = true;
    fact.tier = MemoryTier::LongTerm;
    fact.scope = MemoryScope::Global;
    fact.strength = 1.0;
    ScoredFact { fact, score }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use tempfile::tempdir;

    // MockEmbeddingProvider lives in a #[cfg(test)] mod inside embedding_provider.rs
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;

    async fn create_retrieval() -> (NoteFactRetrieval<SqliteMemoryBackend>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> = Arc::new(
            SqliteMemoryBackend::new(dir.path()).unwrap()
        );
        let indexer = Arc::new(NoteIndexer::new(
            dir.path().to_path_buf(),
            backend.clone(),
        ));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            MockEmbeddingProvider::new(1024, "mock")
        );
        (NoteFactRetrieval::new(indexer, embedder), dir)
    }

    #[tokio::test]
    async fn retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval.retrieve("test query", "default", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn vector_retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval.vector_retrieve("test", "default", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn text_retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval.text_retrieve("query", "default", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_multi_agent_empty_agents_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval.retrieve_multi_agent("query", &[], 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_multi_agent_unknown_agents_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let agents = vec!["agent-a".to_string(), "agent-b".to_string()];
        let results = retrieval.retrieve_multi_agent("query", &agents, 10).await.unwrap();
        assert!(results.is_empty(), "No notes indexed yet → no results");
    }

    #[tokio::test]
    async fn retrieve_all_agents_empty_dir_returns_empty() {
        let (retrieval, dir) = create_retrieval().await;
        let results = retrieval.retrieve_all_agents("query", dir.path(), 10).await.unwrap();
        assert!(results.is_empty());
    }
}
