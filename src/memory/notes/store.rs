//! NoteStore trait — persistence contract for knowledge note indexes.
//!
//! The trait abstracts the index/link/FTS storage so the indexer and
//! gateway layers never depend on a concrete database implementation.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::notes::{FactProvenance, KnowledgeNote};

/// One row from `notes_review_queue` — async LLM review pending decision.
#[derive(Debug, Clone)]
pub struct ReviewQueueRow {
    pub id: String,
    pub agent_id: String,
    pub candidate_json: String,
    pub severity: String,
    pub confidence: f32,
    pub reason: String,
    pub status: String,
    pub retry_count: i64,
    pub created_at: i64,
}

/// Lightweight index entry for a knowledge note (no full content).
#[derive(Debug, Clone)]
pub struct NoteIndexEntry {
    pub path: String,     // "reference/rust-ownership" (relative within agent)
    pub filename: String, // "rust-ownership" (for global wikilink resolution)
    pub agent_id: String, // "default"
    pub category: String, // "reference"
    pub tags: Vec<String>,
    pub link_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
    pub content_hash: String,
}

/// Persistence contract for the notes index, link graph, and full-text search.
///
/// All methods are scoped by `agent_id` to support the `memory/{agent_id}/{category}/`
/// hierarchy.  Notes are identified by `path` (e.g. `"reference/rust-ownership"`).
#[async_trait]
pub trait NoteStore: Send + Sync {
    /// Insert or update the index entry, links, and FTS content for a note.
    ///
    /// `path` is computed as `"{category}/{note.title}"` inside the implementation.
    async fn index_note(
        &self,
        note: &KnowledgeNote,
        agent_id: &str,
        category: &str,
    ) -> Result<(), AlephError>;

    /// Remove a note's index entry, links, and FTS content by path.
    async fn remove_note_index(&self, path: &str, agent_id: &str) -> Result<(), AlephError>;

    /// Look up a single note index entry by path.
    async fn get_note_index(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Option<NoteIndexEntry>, AlephError>;

    /// List all indexed notes for an agent, ordered by most recently updated first.
    async fn list_notes(&self, agent_id: &str) -> Result<Vec<NoteIndexEntry>, AlephError>;

    /// Paths of notes that this note links to.
    async fn get_outgoing_links(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError>;

    /// Paths of notes that link to this note.
    async fn get_incoming_links(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError>;

    /// Full-text search over note content.
    async fn search_notes_fts(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError>;

    /// Top notes by link count + recency, plus edges between visible nodes.
    async fn get_graph_data(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;

    /// BFS neighborhood around `center` up to `depth` hops.
    async fn get_neighbors(
        &self,
        center: &str,
        agent_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;

    /// Count all notes across all agents.
    async fn count_all_notes(&self) -> Result<i64, AlephError>;

    /// Find all note paths that share the given filename (for wikilink resolution).
    async fn find_by_filename(
        &self,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError>;

    /// Store or update the embedding vector for a note.  Stub for now.
    async fn upsert_embedding(
        &self,
        path: &str,
        agent_id: &str,
        embedding: &[f32],
        dim: u32,
    ) -> Result<(), AlephError>;

    /// Search notes by embedding similarity.  Stub for now.
    async fn vector_search(
        &self,
        embedding: &[f32],
        dim: u32,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, AlephError>;

    /// Vector + FTS hybrid search with RRF fusion, returning full content.
    async fn hybrid_search_notes(
        &self,
        embedding: &[f32],
        query_text: &str,
        agent_id: &str,
        dim_hint: u32,
        limit: usize,
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError>;

    /// Vector search returning full content (not just path+score).
    async fn vector_search_notes_with_content(
        &self,
        embedding: &[f32],
        agent_id: &str,
        dim_hint: u32,
        limit: usize,
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError>;

    /// Batch fetch note index metadata by category.
    async fn get_notes_by_category(
        &self,
        agent_id: &str,
        category: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError>;

    /// Get the stored embedding vector for a note path.
    async fn get_embedding(
        &self,
        path: &str,
        agent_id: &str,
        dim_hint: u32,
    ) -> Result<Option<Vec<f32>>, AlephError>;

    /// Retry resolution for any links where `to_note == to_raw` and `to_raw`
    /// has no '/'. Updates `to_note` to the resolved path when filename is
    /// unique.
    ///
    /// Returns the number of rows updated.
    async fn relink_unresolved(&self, agent_id: &str) -> Result<usize, AlephError> {
        let _ = agent_id;
        Ok(0)
    }

    // -----------------------------------------------------------------
    // Phase C2.9.2 governance: per-fact provenance + async review queue.
    // Default impls return empty/no-op so existing test mocks keep
    // compiling; the real bodies live on `SqliteMemoryBackend`.
    // -----------------------------------------------------------------

    /// Replace stored provenance rows for `(agent_id, note_path)` with `provs`,
    /// one row per fact in declaration order.
    async fn upsert_provenance(
        &self,
        agent_id: &str,
        note_path: &str,
        provs: &[FactProvenance],
    ) -> Result<(), AlephError> {
        let _ = (agent_id, note_path, provs);
        Ok(())
    }

    /// Read all stored provenance rows for `(agent_id, note_path)`, ordered by
    /// fact_idx ascending.
    async fn get_provenance(
        &self,
        agent_id: &str,
        note_path: &str,
    ) -> Result<Vec<FactProvenance>, AlephError> {
        let _ = (agent_id, note_path);
        Ok(Vec::new())
    }

    /// Enqueue a candidate for async LLM review. Returns the new queue row id.
    async fn enqueue_review(
        &self,
        agent_id: &str,
        candidate_json: &str,
        severity: &str,
        confidence: f32,
        reason: &str,
    ) -> Result<String, AlephError> {
        let _ = (agent_id, candidate_json, severity, confidence, reason);
        Ok(uuid::Uuid::new_v4().to_string())
    }

    /// List pending review rows older than `earlier_than` (epoch seconds),
    /// scoped by agent.
    async fn list_pending_review(
        &self,
        agent_id: &str,
        earlier_than: i64,
    ) -> Result<Vec<ReviewQueueRow>, AlephError> {
        let _ = (agent_id, earlier_than);
        Ok(Vec::new())
    }

    /// Mark a queued review as decided (status + actor recorded), keeping the
    /// row in `notes_review_queue` until `archive_review` moves it.
    async fn mark_review_decided(
        &self,
        queue_id: &str,
        new_status: &str,
        decision_actor: &str,
    ) -> Result<(), AlephError> {
        let _ = (queue_id, new_status, decision_actor);
        Ok(())
    }

    /// Move a decided review row from `notes_review_queue` to
    /// `notes_review_archive` atomically.
    async fn archive_review(
        &self,
        queue_id: &str,
        final_status: &str,
    ) -> Result<(), AlephError> {
        let _ = (queue_id, final_status);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use uuid::Uuid;

    const AGENT: &str = "default";

    fn create_test_db() -> Arc<SqliteMemoryBackend> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_notes_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    fn sample_note(title: &str, category: &str, links: Vec<&str>) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: category.to_string(),
            tags: vec!["test".to_string()],
            facts: vec!["A test fact".to_string()],
            links: links.into_iter().map(|s| s.to_string()).collect(),
            created_at: 1_700_000_000,
            updated_at: 1_700_001_000,
            content_hash: format!("hash_{title}"),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn indexes_and_retrieves_note() {
        let db = create_test_db();
        let note = sample_note("Editor Preferences", "preference", vec!["Vim", "Neovim"]);

        db.index_note(&note, AGENT, "preference").await.unwrap();

        let entry = db
            .get_note_index("preference/Editor Preferences", AGENT)
            .await
            .unwrap()
            .expect("should exist");

        assert_eq!(entry.path, "preference/Editor Preferences");
        assert_eq!(entry.filename, "Editor Preferences");
        assert_eq!(entry.agent_id, AGENT);
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

        let note_a = sample_note("Rust", "reference", vec!["Cargo", "Clippy"]);
        let note_b = sample_note("Cargo", "reference", vec!["Rust"]);
        let note_c = sample_note("Clippy", "reference", vec![]);

        db.index_note(&note_a, AGENT, "reference").await.unwrap();
        db.index_note(&note_b, AGENT, "reference").await.unwrap();
        db.index_note(&note_c, AGENT, "reference").await.unwrap();

        // Outgoing from Rust: Cargo and Clippy are indexed AFTER Rust, so at
        // write time the bare targets cannot resolve — they fall back to raw.
        // (Task A2.3's lint stage will repair these on a later pass.)
        let out = db
            .get_outgoing_links("reference/Rust", AGENT)
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"Cargo".to_string()));
        assert!(out.contains(&"Clippy".to_string()));

        // Incoming to Rust: Cargo is indexed AFTER Rust, so its link `Rust`
        // resolves to the canonical `reference/Rust` at write time.
        let inc = db
            .get_incoming_links("reference/Rust", AGENT)
            .await
            .unwrap();
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0], "reference/Cargo");
    }

    #[tokio::test]
    async fn find_by_filename_returns_paths() {
        let db = create_test_db();

        let note = sample_note("rust-ownership", "reference", vec![]);
        db.index_note(&note, AGENT, "reference").await.unwrap();

        let paths = db.find_by_filename("rust-ownership", AGENT).await.unwrap();
        assert_eq!(paths, vec!["reference/rust-ownership"]);
    }

    #[tokio::test]
    async fn list_notes_scoped_by_agent() {
        let db = create_test_db();

        let note_a = sample_note("NoteA", "reference", vec![]);
        let note_b = sample_note("NoteB", "reference", vec![]);

        db.index_note(&note_a, "agent1", "reference").await.unwrap();
        db.index_note(&note_b, "agent2", "reference").await.unwrap();

        let agent1_notes = db.list_notes("agent1").await.unwrap();
        assert_eq!(agent1_notes.len(), 1);
        assert_eq!(agent1_notes[0].filename, "NoteA");

        let agent2_notes = db.list_notes("agent2").await.unwrap();
        assert_eq!(agent2_notes.len(), 1);
        assert_eq!(agent2_notes[0].filename, "NoteB");
    }
}
