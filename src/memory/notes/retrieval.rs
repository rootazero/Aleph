//! NoteRetrieval — vector-search over knowledge notes.
//!
//! Given a query string the service embeds it, performs a vector search in the
//! notes index, reads the matching markdown files from disk, and returns them
//! as `NoteContent` items ordered by similarity.

use std::path::PathBuf;

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

/// A single note retrieved by vector similarity search.
#[derive(Debug, Clone)]
pub struct NoteContent {
    /// Relative path within the agent directory, e.g. `"wiki/rust-ownership"`.
    pub path: String,
    /// Full markdown file content.
    pub content: String,
    /// Distance/similarity score from the query embedding.
    pub score: f32,
}

/// Retrieves knowledge notes by embedding similarity.
pub struct NoteRetrieval<S: NoteStore> {
    memory_dir: PathBuf,
    store: Arc<S>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl<S: NoteStore> NoteRetrieval<S> {
    pub fn new(memory_dir: PathBuf, store: Arc<S>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            memory_dir,
            store,
            embedder,
        }
    }

    /// Retrieve the top `limit` notes most similar to `query` for the given agent.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<NoteContent>, AlephError> {
        // 1. Embed the query
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        // 2. Vector search in notes_vec
        let results = self
            .store
            .vector_search(&embedding, dim, agent_id, limit)
            .await?;

        // 3. Read markdown files for top-K paths
        let mut notes = Vec::new();
        for (path, score) in results {
            let file_path = self
                .memory_dir
                .join(agent_id)
                .join(format!("{path}.md"));
            let content = match tokio::fs::read_to_string(&file_path).await {
                Ok(c) => c,
                Err(_) => continue, // file missing on disk — skip gracefully
            };
            notes.push(NoteContent {
                path,
                content,
                score,
            });
        }

        Ok(notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::SqliteMemoryBackend;
    use tokio::fs;
    use uuid::Uuid;

    const AGENT: &str = "default";

    fn create_test_db() -> Arc<SqliteMemoryBackend> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_note_retrieval_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    #[tokio::test]
    async fn retrieve_returns_empty_when_no_embeddings() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));

        let retrieval = NoteRetrieval::new(memory_dir, db, embedder);
        let results = retrieval.retrieve("anything", AGENT, 5).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Insert an embedding but don't create the file
        let fake_embedding = vec![0.1_f32; 1024];
        db.upsert_embedding("wiki/ghost-note", AGENT, &fake_embedding, 1024)
            .await
            .unwrap();

        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));

        let retrieval = NoteRetrieval::new(memory_dir, db, embedder);
        let results = retrieval.retrieve("test query", AGENT, 5).await.unwrap();
        // File doesn't exist on disk, so it should be skipped
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_returns_content_for_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Create a note file on disk
        let note_dir = memory_dir.join(AGENT).join("wiki");
        fs::create_dir_all(&note_dir).await.unwrap();
        fs::write(note_dir.join("rust-ownership.md"), "# Rust Ownership\n\nBorrow checker rules.")
            .await
            .unwrap();

        // Insert an embedding for this note
        let fake_embedding = vec![0.1_f32; 1024];
        db.upsert_embedding("wiki/rust-ownership", AGENT, &fake_embedding, 1024)
            .await
            .unwrap();

        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));

        let retrieval = NoteRetrieval::new(memory_dir, db, embedder);
        let results = retrieval.retrieve("ownership", AGENT, 5).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "wiki/rust-ownership");
        assert!(results[0].content.contains("Borrow checker"));
    }
}
