/// Memory retrieval module
///
/// Tries note-based retrieval first (knowledge notes with vector search).
/// If no notes are found, falls back to `FactRetrieval` for backward
/// compatibility with callers that still use the `MemoryEntry` type.
use crate::config::MemoryConfig;
use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::dreaming::record_activity;
use crate::memory::fact_retrieval::{FactRetrieval, FactRetrievalConfig};
use crate::memory::notes::{NoteContent, NoteRetrieval};
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use crate::utils::paths::get_data_dir;
use tracing::debug;

/// Memory retrieval service — delegates to `FactRetrieval`
#[derive(Clone)]
pub struct MemoryRetrieval {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<MemoryConfig>,
}

impl MemoryRetrieval {
    /// Create new retrieval service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Arc<MemoryConfig>,
    ) -> Self {
        Self {
            database,
            embedder,
            config,
        }
    }

    /// Retrieve memories — tries knowledge notes first, falls back to facts.
    pub async fn retrieve_memories(
        &self,
        _context: &ContextAnchor,
        query: &str,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();

        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        // Try note-based retrieval first
        if let Some(entries) = self.try_note_retrieval(query, 5).await {
            if !entries.is_empty() {
                debug!("Returning {} notes from NoteRetrieval", entries.len());
                return Ok(entries);
            }
        }

        // Fall back to fact retrieval
        let fact_retrieval = FactRetrieval::with_defaults(
            self.database.clone(),
            self.embedder.clone(),
        )
        .with_rerank_config(self.config.rerank.clone());
        let result = fact_retrieval.retrieve(query).await?;
        Ok(result.facts.into_iter().map(fact_to_entry).collect())
    }

    /// Retrieve memories with custom limit — tries notes first, falls back to facts.
    pub async fn retrieve_memories_with_limit(
        &self,
        _context: &ContextAnchor,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();

        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        // Try note-based retrieval first
        if let Some(entries) = self.try_note_retrieval(query, limit).await {
            if !entries.is_empty() {
                debug!("Returning {} notes from NoteRetrieval (limit={})", entries.len(), limit);
                return Ok(entries);
            }
        }

        // Fall back to fact retrieval
        let fact_retrieval = FactRetrieval::new(
            self.database.clone(),
            self.embedder.clone(),
            FactRetrievalConfig {
                max_facts: limit as u32,
                ..FactRetrievalConfig::default()
            },
        )
        .with_rerank_config(self.config.rerank.clone());
        let result = fact_retrieval.retrieve(query).await?;
        Ok(result.facts.into_iter().map(fact_to_entry).collect())
    }

    /// Attempt note retrieval; returns `None` on any error (graceful fallback).
    async fn try_note_retrieval(&self, query: &str, limit: usize) -> Option<Vec<MemoryEntry>> {
        let memory_dir = get_data_dir().ok()?.join("memory");
        let note_retrieval = NoteRetrieval::new(
            memory_dir,
            self.database.clone(),
            self.embedder.clone(),
        );
        let notes = note_retrieval
            .retrieve(query, "default", limit)
            .await
            .ok()?;
        Some(notes.into_iter().map(note_to_entry).collect())
    }
}

/// Convert a `NoteContent` into a `MemoryEntry` for backward compatibility.
fn note_to_entry(note: NoteContent) -> MemoryEntry {
    MemoryEntry {
        id: note.path.clone(),
        context: ContextAnchor::now(String::new()),
        user_input: note.content,
        ai_output: String::new(),
        embedding: None,
        namespace: "owner".to_string(),
        agent: "default".to_string(),
        similarity_score: Some(note.score),
    }
}

/// Convert a `MemoryFact` into a `MemoryEntry` for backward compatibility.
fn fact_to_entry(fact: crate::memory::context::MemoryFact) -> MemoryEntry {
    MemoryEntry {
        id: fact.id,
        context: ContextAnchor::with_timestamp(String::new(), fact.created_at),
        user_input: fact.content,
        ai_output: String::new(),
        embedding: None,
        namespace: fact.namespace,
        agent: fact.agent,
        similarity_score: fact.similarity_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use uuid::Uuid;

    fn create_test_db() -> MemoryBackend {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_retrieval_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    fn create_test_model() -> Arc<dyn EmbeddingProvider> {
        use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
        Arc::new(MockEmbeddingProvider::new(1024, "mock-model"))
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_retrieve_when_disabled() {
        let db = create_test_db();
        let model = create_test_model();
        let config = MemoryConfig {
            enabled: false,
            ..MemoryConfig::default()
        };
        let config = Arc::new(config);

        let retrieval = MemoryRetrieval::new(db, model, config);

        let context = ContextAnchor::now("Test.txt".to_string());
        let memories = retrieval
            .retrieve_memories(&context, "any query")
            .await
            .unwrap();

        assert!(memories.is_empty());
    }
}
