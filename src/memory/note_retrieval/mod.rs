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
}
