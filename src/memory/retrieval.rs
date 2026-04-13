/// Memory retrieval module
///
/// Retrieves memories using note-based vector search (knowledge notes).
use crate::config::MemoryConfig;
use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::dreaming::record_activity;
use crate::memory::notes::{NoteContent, NoteRetrieval};
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use tracing::debug;

/// Memory retrieval service — delegates to `NoteRetrieval`.
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

    /// Retrieve memories using note-based vector search.
    pub async fn retrieve_memories(
        &self,
        _context: &ContextAnchor,
        query: &str,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        self.retrieve_memories_with_limit(_context, query, 5).await
    }

    /// Retrieve memories with custom limit using note-based vector search.
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

        let memory_dir = match crate::utils::paths::get_note_memory_dir() {
            Ok(dir) => dir,
            Err(e) => {
                debug!(error = %e, "Note memory dir unavailable, returning empty");
                return Ok(Vec::new());
            }
        };

        let note_retrieval =
            NoteRetrieval::new(memory_dir, self.database.clone(), self.embedder.clone());

        let notes = note_retrieval
            .retrieve(query, "default", limit)
            .await
            .unwrap_or_else(|e| {
                debug!(error = %e, "NoteRetrieval failed, returning empty");
                Vec::new()
            });

        debug!(count = notes.len(), limit, "NoteRetrieval returned notes");
        Ok(notes.into_iter().map(note_to_entry).collect())
    }
}

/// Convert a `NoteContent` into a `MemoryEntry`.
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
