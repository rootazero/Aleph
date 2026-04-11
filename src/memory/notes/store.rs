//! NoteStore trait — persistence contract for knowledge note indexes.
//!
//! The trait abstracts the index/link/FTS storage so the indexer and
//! gateway layers never depend on a concrete database implementation.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::notes::KnowledgeNote;

/// Lightweight index entry for a knowledge note (no full content).
#[derive(Debug, Clone)]
pub struct NoteIndexEntry {
    pub title: String,
    pub category: String,
    pub tags: Vec<String>,
    pub link_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
    pub content_hash: String,
}

/// Persistence contract for the notes index, link graph, and full-text search.
#[async_trait]
pub trait NoteStore: Send + Sync {
    /// Insert or update the index entry, links, and FTS content for a note.
    async fn index_note(&self, note: &KnowledgeNote) -> Result<(), AlephError>;

    /// Remove a note's index entry, links, and FTS content by title.
    async fn remove_note_index(&self, title: &str) -> Result<(), AlephError>;

    /// Look up a single note index entry by title.
    async fn get_note_index(&self, title: &str) -> Result<Option<NoteIndexEntry>, AlephError>;

    /// List all indexed notes, ordered by most recently updated first.
    async fn list_notes(&self) -> Result<Vec<NoteIndexEntry>, AlephError>;

    /// Titles of notes that this note links to.
    async fn get_outgoing_links(&self, title: &str) -> Result<Vec<String>, AlephError>;

    /// Titles of notes that link to this note.
    async fn get_incoming_links(&self, title: &str) -> Result<Vec<String>, AlephError>;

    /// Full-text search over note content.
    async fn search_notes_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError>;

    /// Top notes by link count + recency, plus edges between visible nodes.
    async fn get_graph_data(
        &self,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;

    /// BFS neighborhood around `center` up to `depth` hops.
    async fn get_neighbors(
        &self,
        center: &str,
        depth: u8,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use std::sync::Arc;
    use uuid::Uuid;

    fn create_test_db() -> Arc<SqliteMemoryBackend> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_notes_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    fn sample_note(title: &str, links: Vec<&str>) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: "preference".to_string(),
            tags: vec!["test".to_string()],
            facts: vec!["A test fact".to_string()],
            links: links.into_iter().map(|s| s.to_string()).collect(),
            created_at: 1_700_000_000,
            updated_at: 1_700_001_000,
            content_hash: format!("hash_{title}"),
        }
    }

    #[tokio::test]
    async fn indexes_and_retrieves_note() {
        let db = create_test_db();
        let note = sample_note("Editor Preferences", vec!["Vim", "Neovim"]);

        db.index_note(&note).await.unwrap();

        let entry = db
            .get_note_index("Editor Preferences")
            .await
            .unwrap()
            .expect("should exist");

        assert_eq!(entry.title, "Editor Preferences");
        assert_eq!(entry.category, "preference");
        assert_eq!(entry.tags, vec!["test"]);
        assert_eq!(entry.link_count, 2);
        assert_eq!(entry.created_at, 1_700_000_000);
        assert_eq!(entry.updated_at, 1_700_001_000);
        assert_eq!(entry.content_hash, "hash_Editor Preferences");
    }

    #[tokio::test]
    async fn stores_and_queries_links() {
        let db = create_test_db();

        let note_a = sample_note("Rust", vec!["Cargo", "Clippy"]);
        let note_b = sample_note("Cargo", vec!["Rust"]);
        let note_c = sample_note("Clippy", vec![]);

        db.index_note(&note_a).await.unwrap();
        db.index_note(&note_b).await.unwrap();
        db.index_note(&note_c).await.unwrap();

        // Outgoing from Rust
        let out = db.get_outgoing_links("Rust").await.unwrap();
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"Cargo".to_string()));
        assert!(out.contains(&"Clippy".to_string()));

        // Incoming to Rust (only Cargo links back)
        let inc = db.get_incoming_links("Rust").await.unwrap();
        assert_eq!(inc, vec!["Cargo"]);
    }
}
