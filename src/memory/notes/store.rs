//! `NoteStore` trait — persistence contract for knowledge note indexes.
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

/// Strip a single trailing `.md` extension from a note title or filename.
///
/// Note titles/filenames are stored extensionless by convention (see
/// [`NoteIndexEntry::filename`]). Some writers historically passed a title that
/// already carried `.md`, which corrupted the index and made disk reads compute
/// a doubled `*.md.md` path. Normalizing at the write chokepoint and tolerating
/// it on read keeps both paths single-extension.
#[must_use]
pub fn strip_md_ext(s: &str) -> &str {
    s.strip_suffix(".md").unwrap_or(s)
}

/// Build the on-disk note filename (`<stem>.md`) tolerating a stem that already
/// carries a trailing `.md`, so legacy `.md`-suffixed index rows resolve to the
/// correct single-extension file rather than `*.md.md`.
#[must_use]
pub fn note_md_filename(filename: &str) -> String {
    format!("{}.md", strip_md_ext(filename))
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

    /// Upsert a directed link `from_note -> to_note` and set its `relation` label.
    ///
    /// Used by keyword linking after the body `[[ ]]` link is written, so the
    /// connecting keyword is recorded on the edge. The plain-link path leaves
    /// `relation` NULL; this method sets it explicitly.
    async fn add_link_with_relation(
        &self,
        agent_id: &str,
        from_note: &str,
        to_note: &str,
        relation: &str,
    ) -> Result<(), AlephError>;

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

    /// Paths of notes that link to this note, matching either the resolved
    /// full path or the bare filename. `notes_links.to_note` holds the resolved
    /// target — a full path when `resolve_target` matched a unique filename,
    /// otherwise the bare wikilink text. Dream stages walking `category/title`
    /// notes pass both forms so resolved and legacy rows are counted alike.
    async fn get_incoming_links_any(
        &self,
        path: &str,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError>;

    /// Outgoing **typed** edges for a note: `(to_note, relation_type)` for every
    /// row whose `relation` column is non-NULL. Untyped body wikilinks are
    /// excluded. Used to surface entity-graph edge labels (Gap A).
    async fn get_typed_relations(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<(String, String)>, AlephError>;

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
    // Knowledge-graph materialization (Phase 4). Default impls keep any
    // non-SQLite store compiling; the real bodies live on
    // `SqliteMemoryBackend`.
    // -----------------------------------------------------------------

    /// Load the full note graph for `agent_id`: every node (`path`/`category`)
    /// plus its `source_notes`, and every resolved edge from `notes_links`.
    async fn load_graph_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<crate::memory::notes::graph::GraphSnapshot, AlephError> {
        let _ = agent_id;
        Ok(crate::memory::notes::graph::GraphSnapshot::default())
    }

    /// Replace the materialized graph cache (community/cohesion/degree) for
    /// `agent_id`. Rows are `(node_path, community_id, cohesion, degree)`.
    async fn replace_graph_cache(
        &self,
        agent_id: &str,
        rows: &[(String, usize, f32, usize)],
    ) -> Result<(), AlephError> {
        let _ = (agent_id, rows);
        Ok(())
    }

    /// Replace materialized insights for `agent_id`. Rows are
    /// `(kind, payload_json)`.
    async fn replace_graph_insights(
        &self,
        agent_id: &str,
        rows: &[(String, String)],
    ) -> Result<(), AlephError> {
        let _ = (agent_id, rows);
        Ok(())
    }

    /// Read materialized insights for `agent_id`, optionally filtered by `kind`.
    /// Returns `(kind, payload_json)` rows.
    async fn read_graph_insights(
        &self,
        agent_id: &str,
        kind: Option<&str>,
    ) -> Result<Vec<(String, String)>, AlephError> {
        let _ = (agent_id, kind);
        Ok(Vec::new())
    }

    /// Node paths sharing `node_path`'s materialized community (from
    /// `notes_graph_cache`), excluding `node_path` itself, capped at `limit`.
    ///
    /// Used by note retrieval to expand a seed hit with its strongest-relatedness
    /// peers. Default impl (and a cold cache before the first dream recompute)
    /// returns an empty vec, so callers degrade to their existing expansion.
    async fn community_peers(
        &self,
        agent_id: &str,
        node_path: &str,
        limit: usize,
    ) -> Result<Vec<String>, AlephError> {
        let _ = (agent_id, node_path, limit);
        Ok(vec![])
    }

    /// Replace the materialized 4-signal relatedness edges for `agent_id`.
    /// Rows are `(node_path, related_path, score)`.
    async fn replace_graph_related(
        &self,
        agent_id: &str,
        rows: &[(String, String, f32)],
    ) -> Result<(), AlephError> {
        let _ = (agent_id, rows);
        Ok(())
    }

    /// Top related peers for `node_path` by materialized 4-signal score
    /// (descending), capped at `limit`. Returns `(related_path, score)`.
    ///
    /// Complements `community_peers`: where community membership gives coarse
    /// cluster recall, this gives the scored 4-signal relatedness. A cold cache
    /// (and the default impl) returns an empty vec, so callers degrade to their
    /// existing expansion.
    async fn related_peers(
        &self,
        agent_id: &str,
        node_path: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, AlephError> {
        let _ = (agent_id, node_path, limit);
        Ok(vec![])
    }

    /// Replace the behavioral `co_recalled` edge set for `agent_id` with
    /// `rows` (`(note_a, note_b, confidence)`). Implementations must never
    /// overwrite an existing semantic link for the same note pair — the
    /// behavioral relation only fills gaps in the graph. Default impl is a
    /// no-op so non-`SQLite` stores and test mocks compile unchanged.
    async fn replace_co_recall_links(
        &self,
        agent_id: &str,
        rows: &[(String, String, f32)],
    ) -> Result<(), AlephError> {
        let _ = (agent_id, rows);
        Ok(())
    }

    /// Batch-fetch notes (with full content) by exact path, for `agent_id`.
    /// Unknown/deleted paths are omitted; order follows `paths`. The `score`
    /// field is `0.0` (callers assign their own). Mirrors the row ->
    /// `NoteSearchResult` shape of `hybrid_search_notes`. Default impl returns
    /// empty so non-`SQLite` stores / test mocks keep compiling.
    async fn get_notes_with_content(
        &self,
        agent_id: &str,
        paths: &[String],
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
        let _ = (agent_id, paths);
        Ok(Vec::new())
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
    /// `fact_idx` ascending.
    async fn get_provenance(
        &self,
        agent_id: &str,
        note_path: &str,
    ) -> Result<Vec<FactProvenance>, AlephError> {
        let _ = (agent_id, note_path);
        Ok(Vec::new())
    }

    /// Source refs (raw-memory ids or prior-note paths) a note was distilled
    /// from — forward provenance. Default: empty.
    async fn sources_of(
        &self,
        _agent_id: &str,
        _note_path: &str,
    ) -> Result<Vec<String>, AlephError> {
        Ok(Vec::new())
    }

    /// Reverse provenance: note paths that cite a given source ref (raw id or
    /// note path). Backed by `idx_notes_sources_ref`. Default: empty.
    async fn notes_citing(
        &self,
        _agent_id: &str,
        _source_ref: &str,
    ) -> Result<Vec<String>, AlephError> {
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
    async fn archive_review(&self, queue_id: &str, final_status: &str) -> Result<(), AlephError> {
        let _ = (queue_id, final_status);
        Ok(())
    }

    /// Phase C2.7 — return the `MAX(created_at)` recall signal for
    /// `(agent_id, note_path)`, or `None` if the note has never been recalled.
    /// Default impl returns `Ok(None)` so existing test mocks compile unchanged.
    async fn recall_signals_last_hit(
        &self,
        agent_id: &str,
        note_path: &str,
    ) -> Result<Option<i64>, AlephError> {
        let _ = (agent_id, note_path);
        Ok(None)
    }

    /// Total recall-signal count per note path, for retrieval-time reinforcement
    /// salience. Returns a map `note_path -> hit_count` for the requested paths
    /// that have at least one recorded recall signal (absent paths score neutral).
    ///
    /// Default impl returns an empty map so existing test mocks and non-SQLite
    /// stores compile unchanged and simply contribute no reinforcement boost.
    async fn recall_hit_counts(
        &self,
        note_paths: &[String],
    ) -> Result<std::collections::HashMap<String, i64>, AlephError> {
        let _ = note_paths;
        Ok(std::collections::HashMap::new())
    }

    /// Record recall ("retrieval") hits for the notes a search actually surfaced,
    /// so frequently-recalled notes accrue reinforcement salience ("热门记忆浮顶")
    /// and bubble up via [`recall_hit_counts`](Self::recall_hit_counts). Each hit
    /// is `(note_path, score)`. The backing store dedups per
    /// `(note_path, query, day, channel)`, so repeated recalls of the same note
    /// for the same query on the same day count once. Returns the number of newly
    /// recorded (non-duplicate) signals.
    ///
    /// This is the producer half of the hot-floating loop; `recall_hit_counts` is
    /// the consumer half. Default impl is a no-op returning `Ok(0)` so non-SQLite
    /// stores and test mocks compile unchanged and simply record nothing.
    async fn record_recall_hits(
        &self,
        query: &str,
        channel: &str,
        hits: &[(String, f32)],
        namespace: &str,
    ) -> Result<usize, AlephError> {
        let _ = (query, channel, hits, namespace);
        Ok(0)
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

    #[test]
    fn strip_md_ext_removes_single_trailing_extension() {
        assert_eq!(strip_md_ext("rust-ownership"), "rust-ownership");
        assert_eq!(strip_md_ext("toolchain.md"), "toolchain");
        // Only one extension is stripped; an embedded ".md" mid-name is left.
        assert_eq!(strip_md_ext("a.md.md"), "a.md");
        assert_eq!(strip_md_ext("notes.markdown"), "notes.markdown");
    }

    #[test]
    fn note_md_filename_never_doubles_extension() {
        assert_eq!(note_md_filename("rust-ownership"), "rust-ownership.md");
        assert_eq!(note_md_filename("toolchain.md"), "toolchain.md");
    }

    #[tokio::test]
    async fn index_note_normalizes_md_suffixed_title() {
        let db = create_test_db();
        // A writer that mistakenly passes a title carrying ".md" must still
        // produce an extensionless index row (path + filename), so disk reads
        // compute "<stem>.md" rather than "<stem>.md.md".
        let note = sample_note("toolchain.md", "preferences", vec![]);

        db.index_note(&note, AGENT, "preferences").await.unwrap();

        let entry = db
            .get_note_index("preferences/toolchain", AGENT)
            .await
            .unwrap()
            .expect("normalized path should exist");
        assert_eq!(entry.path, "preferences/toolchain");
        assert_eq!(entry.filename, "toolchain");
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
    async fn co_recall_links_fill_gaps_without_clobbering_semantic_links() {
        let db = create_test_db();
        // Index targets first so SourceA's wikilink resolves to a full path.
        let b = sample_note("TargetB", "reference", vec![]);
        let c = sample_note("TargetC", "reference", vec![]);
        let a = sample_note("SourceA", "reference", vec!["TargetB"]);
        db.index_note(&b, AGENT, "reference").await.unwrap();
        db.index_note(&c, AGENT, "reference").await.unwrap();
        db.index_note(&a, AGENT, "reference").await.unwrap();

        let rows = vec![
            (
                "reference/SourceA".to_string(),
                "reference/TargetB".to_string(),
                0.5_f32,
            ),
            (
                "reference/SourceA".to_string(),
                "reference/TargetC".to_string(),
                0.8_f32,
            ),
        ];
        db.replace_co_recall_links(AGENT, &rows).await.unwrap();

        let typed = db
            .get_typed_relations("reference/SourceA", AGENT)
            .await
            .unwrap();
        // The behavioral A→C edge landed…
        assert!(typed
            .iter()
            .any(|(to, rel)| to == "reference/TargetC" && rel == "co_recalled"));
        // …but the existing semantic A→B wikilink was NOT converted.
        assert!(!typed
            .iter()
            .any(|(to, rel)| to == "reference/TargetB" && rel == "co_recalled"));

        // Full refresh: an empty replace clears behavioral edges only.
        db.replace_co_recall_links(AGENT, &[]).await.unwrap();
        let typed = db
            .get_typed_relations("reference/SourceA", AGENT)
            .await
            .unwrap();
        assert!(typed.iter().all(|(_, rel)| rel != "co_recalled"));
        let out = db
            .get_outgoing_links("reference/SourceA", AGENT)
            .await
            .unwrap();
        assert!(
            out.contains(&"reference/TargetB".to_string()),
            "semantic wikilink must survive the behavioral refresh"
        );
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
