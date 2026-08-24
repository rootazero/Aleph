//! `NoteStore` trait — persistence contract for knowledge note indexes.
//!
//! The trait abstracts the index/link/FTS storage so the indexer and
//! gateway layers never depend on a concrete database implementation.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::notes::KnowledgeNote;

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

/// One decided row from `notes_review_archive` — the governance gate's verdict
/// on a note candidate that never became a note (or became one late).
///
/// This table was write-only for its whole life: `archive_review` inserted into
/// it and nothing in the repo, tests included, ever ran a `SELECT` against it.
/// That is load-bearing for the `rejected` verdict in particular — a rejected
/// candidate's knowledge is not written to any note, so the archive row *is*
/// the only surviving record of what was proposed and why it was turned down.
/// "The knowledge is preserved" is a claim about a table someone has to be able
/// to read.
#[derive(Debug, Clone)]
pub struct ReviewArchiveRow {
    pub id: String,
    pub candidate_json: String,
    /// `approved` | `rejected` | `rewritten` | `timeout` | `max_retries_exceeded`.
    pub final_status: String,
    pub reason: String,
    pub created_at: i64,
    pub archived_at: i64,
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

/// Full active edge row for graph feeds (query/canvas).
#[derive(Debug, Clone)]
pub struct GraphEdgeRow {
    pub from: String,
    pub to: String,
    pub relation: Option<String>,
    pub label: Option<String>,
    pub confidence: f32,
}

/// Full outgoing link row for node detail (includes non-active rows so the
/// panel can show dangling/tombstone links — they are invisible in the graph
/// but must be visible in detail).
#[derive(Debug, Clone)]
pub struct OutgoingLinkRow {
    pub to_note: String,
    pub to_raw: String,
    pub relation: Option<String>,
    pub label: Option<String>,
    pub confidence: f32,
    pub resolved_by: Option<String>,
    pub status: String,
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

/// Resolve a note's on-disk path under a given note-memory root.
///
/// The single derivation of "where does this note live". Readers that built the
/// path by hand with `format!("{filename}.md")` bypassed [`strip_md_ext`] and so
/// resolved a legacy `.md`-suffixed index row to `*.md.md` — a miss that surfaces
/// as an empty body rather than an error.
///
/// The *root* is deliberately a parameter: it belongs to whoever owns the
/// indexer, and is not always the process-global
/// `utils::paths::get_note_memory_dir()`.
#[must_use]
pub fn note_content_path(
    memory_dir: &std::path::Path,
    agent_id: &str,
    category: &str,
    filename: &str,
) -> std::path::PathBuf {
    memory_dir
        .join(agent_id)
        .join(category)
        .join(note_md_filename(filename))
}

/// Result of a hybrid (vector + full-text) note search, with each leg's
/// contribution reported alongside the fused ranking.
///
/// The counts exist so a caller can describe what actually ran instead of what
/// it asked for: "hybrid" with `vector_candidates == 0` means the semantic half
/// did not participate at all — an empty or dimension-mismatched vector index —
/// which is a different situation from a semantic search that simply found
/// nothing, and the two used to be indistinguishable from the outside.
#[derive(Debug, Clone, Default)]
pub struct HybridSearchOutcome {
    /// Fused, ranked results (already truncated to the requested limit).
    pub results: Vec<crate::memory::notes::NoteSearchResult>,
    /// Candidates the vector leg contributed to fusion.
    pub vector_candidates: usize,
    /// Candidates the full-text leg contributed to fusion.
    pub fts_candidates: usize,
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

    /// [`Self::list_notes`] across several partitions at once, merged back into
    /// one `updated_at DESC` ordering.
    ///
    /// The gateway's enumerating readers resolve a base persona id into the
    /// union `[org tier, this session's partition]`
    /// (`gateway::handlers::memory_scope::read_partitions`), because that is
    /// where the writers wrote. The merge has to happen HERE rather than in the
    /// caller: the ordering is what pagination slices, and two separately
    /// ordered lists concatenated are not one ordered list — the second page
    /// would interleave partitions arbitrarily.
    ///
    /// Rows are NOT deduplicated by `path`. The same `category/filename` can
    /// exist in two partitions and they are two different notes; each row
    /// carries its own `agent_id`, which is the half of the note's identity
    /// that tells them apart and the value every addressing verb must be
    /// handed back.
    async fn list_notes_multi(
        &self,
        agent_ids: &[String],
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let mut all = Vec::new();
        for agent_id in agent_ids {
            all.extend(self.list_notes(agent_id).await?);
        }
        all.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
        Ok(all)
    }

    /// [`Self::count_notes`] summed across several partitions.
    ///
    /// Partitions are disjoint by construction (`notes_index.agent_id` is the
    /// partition), so the sum is a count and not an over-count — the property
    /// that lets this pair with [`Self::list_notes_multi`] as a pager total.
    async fn count_notes_multi(&self, agent_ids: &[String]) -> Result<i64, AlephError> {
        let mut total = 0i64;
        for agent_id in agent_ids {
            total = total.saturating_add(self.count_notes(agent_id).await?);
        }
        Ok(total)
    }

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
    /// target — a full path when the links resolver matched a unique filename,
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

    /// [`Self::search_notes_fts`] across several partitions, merged and capped.
    ///
    /// Each partition is searched to the full `limit` and the merged pool is
    /// then truncated — over-fetching rather than dividing the budget, so a
    /// union whose hits all live in one partition returns the same rows a
    /// single-partition search would.
    ///
    /// The merge orders by `updated_at DESC` rather than by relevance: fts5
    /// ranks within one query and there is no cross-query score to compare, so
    /// interleaving on rank would be inventing an ordering. Recency is the
    /// ordering the note list already uses, which keeps the two agreeing.
    async fn search_notes_fts_multi(
        &self,
        query: &str,
        agent_ids: &[String],
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let mut all = Vec::new();
        for agent_id in agent_ids {
            all.extend(self.search_notes_fts(query, agent_id, limit).await?);
        }
        all.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
        all.truncate(limit);
        Ok(all)
    }

    /// Top notes by link count + recency, plus active edges between visible
    /// nodes (`status = 'active'` only — dangling/tombstone rows never
    /// surface in the graph feed).
    async fn get_graph_data(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<GraphEdgeRow>), AlephError>;

    /// BFS neighborhood around `center` up to `depth` hops.
    ///
    /// Returns `(nodes, edges, truncated)`. `truncated` is `true` when the BFS
    /// hit `limit` and stopped early — the graph may hold more neighbors than
    /// returned. It is derived from the BFS `visited` frontier, NOT the returned
    /// node count, because `visited` includes dangling link endpoints that are
    /// not indexed nodes (inferring truncation from `nodes.len()` would
    /// under-report a cap whenever an endpoint dangles).
    async fn get_neighbors(
        &self,
        center: &str,
        agent_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>, bool), AlephError>;

    /// Shortest edge-path from `from` to `to` over `notes_links`, traversing
    /// edges in **both** directions (the knowledge graph is conceptually
    /// undirected for connection queries), bounded by `max_depth` hops. Returns
    /// `Some(path)` — the ordered walk as `(node, next_node, connecting-edge
    /// relation)` hops from `from` to `to` — or `None` if the two notes are not
    /// connected within `max_depth`. The trailing bool is `truncated`: the BFS
    /// frontier hit the depth (or a hard visit cap) before reaching `to`, so a
    /// longer path may exist beyond what was explored — this distinguishes "no
    /// path within the limit" from "provably disconnected". Answers "how is note
    /// A connected to note B": a deterministic retrieval query, not reasoning
    /// (R7). A note is connected to itself by the empty path.
    ///
    /// Default impl returns `(None, false)` so non-SQLite stores and test mocks
    /// compile unchanged; the real body lives on `SqliteMemoryBackend`.
    async fn find_path(
        &self,
        from: &str,
        to: &str,
        agent_id: &str,
        max_depth: u8,
    ) -> Result<(Option<Vec<(String, String, Option<String>)>>, bool), AlephError> {
        let _ = (from, to, agent_id, max_depth);
        Ok((None, false))
    }

    /// All outgoing link rows for one note, including dangling and tombstone
    /// (unlike the graph feed, node detail must surface every lifecycle
    /// state). Default impl returns empty so non-SQLite stores and test
    /// mocks compile unchanged; the real body lives on `SqliteMemoryBackend`.
    async fn get_outgoing_link_rows(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<OutgoingLinkRow>, AlephError> {
        let _ = (path, agent_id);
        Ok(Vec::new())
    }

    /// Count all notes across all agents.
    async fn count_all_notes(&self) -> Result<i64, AlephError>;

    /// Count notes for a single agent (scoped, unlike `count_all_notes`).
    /// Lets the graph panel show "showing top N of M" when a query truncates.
    async fn count_notes(&self, agent_id: &str) -> Result<i64, AlephError>;

    /// Map every cached note path to its community id for one agent
    /// (`notes_graph_cache`). Empty before the first dream graph-recompute.
    async fn community_ids(
        &self,
        agent_id: &str,
    ) -> Result<std::collections::HashMap<String, i64>, AlephError>;

    /// Find all note paths that share the given filename (for wikilink resolution).
    async fn find_by_filename(
        &self,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError>;

    /// Store or update the embedding vector for a note. Backed by sqlite-vec
    /// (`notes_vec_{dim}` + `notes_vec_map`) in `SqliteMemoryBackend`.
    ///
    /// `content_hash` is the note's [`KnowledgeNote::content_hash`] for the
    /// text that was embedded; it is recorded alongside the vector so
    /// [`Self::stale_vector_paths`] can tell a current vector from one left
    /// behind by a swallowed embed failure. Pass `""` when the caller does not
    /// know it — that records "provenance unknown", which reads as *stale*, so
    /// an under-informed caller errs toward re-embedding rather than toward
    /// claiming freshness it cannot vouch for.
    ///
    /// Deliberately one method rather than two: a second, hash-less entry point
    /// is a second path every future writer can forget to keep in step.
    ///
    /// [`KnowledgeNote::content_hash`]: crate::memory::notes::KnowledgeNote::content_hash
    async fn upsert_embedding(
        &self,
        path: &str,
        agent_id: &str,
        embedding: &[f32],
        dim: u32,
        content_hash: &str,
    ) -> Result<(), AlephError>;

    /// Note paths whose vector is missing or was computed from a different
    /// version of the note than the one currently indexed.
    ///
    /// This is the observable half of embed-on-write: that path logs and
    /// swallows failures on purpose (the note is already on disk), which left
    /// no way at all to find out afterwards that a note had silently dropped
    /// out of vector search. A path is stale when it has no `notes_vec_map`
    /// row, or when the recorded `embedded_hash` differs from the note's
    /// current `content_hash` (including the empty hash written by
    /// pre-freshness rows and by callers that did not supply one).
    ///
    /// Default impl returns empty for backends without a vector index.
    async fn stale_vector_paths(&self, _agent_id: &str) -> Result<Vec<String>, AlephError> {
        Ok(Vec::new())
    }

    /// Search notes by embedding similarity (sqlite-vec KNN over the
    /// dimension-matched `notes_vec_{dim}` table).
    async fn vector_search(
        &self,
        embedding: &[f32],
        dim: u32,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, AlephError>;

    /// Vector + FTS hybrid search with RRF fusion, returning full content
    /// alongside how much each leg contributed.
    async fn hybrid_search_notes(
        &self,
        embedding: &[f32],
        query_text: &str,
        agent_id: &str,
        dim_hint: u32,
        limit: usize,
    ) -> Result<HybridSearchOutcome, AlephError>;

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

    /// Re-resolve dangling/tombstone inbound rows whose `to_raw` matches any of
    /// `keys` (the new/renamed note's filename, full path, and aliases).
    /// Returns the number of rows revived. Targeted via `idx_notes_links_to_raw`
    /// — NOT a full-table relink sweep (unlike [`relink_unresolved`](Self::relink_unresolved)).
    ///
    /// Matching is a literal-string lookup against `to_raw` — it only catches
    /// rows whose raw wikilink text is byte-identical to one of `keys`. A raw
    /// link that would only resolve via the tier-4 *normalized* strategy
    /// (case-folding / full-width variants, see `links::resolve`) is NOT
    /// revived by this targeted pass; it self-heals only when its owning note
    /// is next re-indexed or via a full [`relink_unresolved`](Self::relink_unresolved) sweep.
    /// Default impl returns `Ok(0)` so non-SQLite stores and test mocks compile
    /// unchanged; the real body lives on `SqliteMemoryBackend`.
    async fn backfill_inbound_links(
        &self,
        agent_id: &str,
        keys: &[String],
    ) -> Result<usize, AlephError> {
        let _ = (agent_id, keys);
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

    /// Distinct edge relation-type vocabulary with counts over `status='active'`
    /// links for one agent, most frequent first. A NULL relation (a plain body
    /// wikilink, not a dream-stage typed edge) is labelled `"link"`. Powers the
    /// `note_graph_query` schema-introspection op so the model can discover the
    /// graph's de-facto edge taxonomy before querying. Default impl returns
    /// empty so non-SQLite stores and test mocks compile unchanged.
    async fn relation_type_counts(&self, agent_id: &str) -> Result<Vec<(String, i64)>, AlephError> {
        let _ = agent_id;
        Ok(vec![])
    }

    /// Replace the materialized 5-signal relatedness edges for `agent_id`.
    /// Rows are `(node_path, related_path, score)`.
    async fn replace_graph_related(
        &self,
        agent_id: &str,
        rows: &[(String, String, f32)],
    ) -> Result<(), AlephError> {
        let _ = (agent_id, rows);
        Ok(())
    }

    /// Top related peers for `node_path` by materialized 5-signal score
    /// (descending), capped at `limit`. Returns `(related_path, score)`.
    ///
    /// Complements `community_peers`: where community membership gives coarse
    /// cluster recall, this gives the scored 5-signal relatedness. A cold cache
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

    /// Materialized 5-signal relatedness edges (`notes_graph_related`) whose
    /// endpoints are both in `visible`, top `per_node` by score per
    /// `node_path`. Batch/visibility-filtered counterpart to `related_peers`,
    /// used by `graph.query` to surface similarity edges alongside real
    /// links. Default impl returns empty so non-SQLite stores and test mocks
    /// compile unchanged; the real body lives on `SqliteMemoryBackend`.
    async fn related_edges_between(
        &self,
        agent_id: &str,
        visible: &std::collections::HashSet<String>,
        per_node: usize,
    ) -> Result<Vec<(String, String, f32)>, AlephError> {
        let _ = (agent_id, visible, per_node);
        Ok(Vec::new())
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

    /// Full refresh of `relation='mention'` rows (recomputed each dream
    /// cycle, spec M1). Implementations must never overwrite an existing
    /// semantic link for the pair — an existing link always wins over the
    /// mention soft edge. Default impl is a no-op so non-`SQLite` stores and
    /// test mocks compile unchanged.
    async fn replace_mention_links(
        &self,
        agent_id: &str,
        rows: &[(String, String)],
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
    // Phase C2.9.2 governance: provenance reads + async review queue.
    // Default impls return empty/no-op so existing test mocks keep
    // compiling; the real bodies live on `SqliteMemoryBackend`.
    //
    // Per-*fact* provenance is deliberately not readable here. Its two
    // methods (`upsert_provenance` / `get_provenance`) shipped with zero
    // callers and were removed on 2026-08-23: the write was a byte-for-byte
    // copy of the loop already inlined in `index_note`, and the read asked a
    // question the note itself answers better — the table holds no fact text,
    // so any caller wanting both would pair text from the file with
    // provenance from the index. `notes_provenance` is read on the *reverse*
    // axis instead, inside `notes_citing`, which is the one thing no single
    // file can answer.
    // -----------------------------------------------------------------

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
    /// note path), deduped and ordered by path.
    ///
    /// Both citation levels count. `notes_sources` records note-level citation
    /// (the `source_notes` frontmatter list); `notes_provenance` records
    /// fact-level citation (an inline `<!-- src: ... -->` marker on one line).
    /// Neither contains the other — a note can quote a raw in a single fact
    /// without that raw reaching its frontmatter — so a reader of only the
    /// first reports "not cited" for a note that visibly quotes the row. The
    /// raw-retention invariant in `RAW_MEMORY.md` ("never delete a row while
    /// this is non-empty") is stated against the union, and is only true of it.
    ///
    /// Backed by `idx_notes_sources_ref` and `idx_prov_agent_source`.
    /// Default: empty.
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

    /// Increment a queued review row's `retry_count` (transient failure that
    /// left it queued). Returns the new count so the caller can enforce a retry
    /// ceiling and evict rows that never adjudicate. Default no-op returns 0 so
    /// existing test mocks compile unchanged.
    async fn increment_review_retry(&self, queue_id: &str) -> Result<i64, AlephError> {
        let _ = queue_id;
        Ok(0)
    }

    /// Move a decided review row from `notes_review_queue` to
    /// `notes_review_archive` atomically.
    async fn archive_review(&self, queue_id: &str, final_status: &str) -> Result<(), AlephError> {
        let _ = (queue_id, final_status);
        Ok(())
    }

    /// Read the most recently decided governance verdicts, newest first.
    ///
    /// The consumer half of [`Self::archive_review`]. Default impl returns
    /// empty so non-SQLite stores and test mocks compile unchanged.
    async fn list_review_archive(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ReviewArchiveRow>, AlephError> {
        let _ = (agent_id, limit);
        Ok(Vec::new())
    }

    /// Delete archive rows older than `older_than_secs`, returning how many
    /// went. Returns the count so a caller can log a real number rather than
    /// assert one.
    ///
    /// An append-only decision log with no retention is a table that grows for
    /// the life of the install; it is written on every gated candidate and the
    /// rows carry a full candidate payload each.
    async fn prune_review_archive(
        &self,
        agent_id: &str,
        older_than_secs: i64,
    ) -> Result<usize, AlephError> {
        let _ = (agent_id, older_than_secs);
        Ok(0)
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
        agent_id: &str,
        note_paths: &[String],
    ) -> Result<std::collections::HashMap<String, i64>, AlephError> {
        let _ = (agent_id, note_paths);
        Ok(std::collections::HashMap::new())
    }

    /// Record recall ("retrieval") hits for the notes a search actually surfaced,
    /// so frequently-recalled notes accrue reinforcement salience ("popular memories float to top")
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
        agent_id: &str,
    ) -> Result<usize, AlephError> {
        let _ = (query, channel, hits, agent_id);
        Ok(0)
    }

    /// Remove embedding rows whose note no longer exists in `notes_index`.
    /// Historical deletes did not clear `notes_vec_map` / `notes_vec_{dim}`,
    /// so ghost vectors kept occupying KNN slots forever. Called from
    /// `full_rebuild`'s reconcile pass. Returns the number of pruned mappings.
    /// Default impl is a no-op so non-SQLite stores and mocks compile
    /// unchanged.
    async fn prune_orphan_vectors(&self, agent_id: &str) -> Result<usize, AlephError> {
        let _ = agent_id;
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    const AGENT: &str = "default";

    fn create_test_db() -> (tempfile::TempDir, Arc<SqliteMemoryBackend>) {
        let (scratch, db_path) = crate::utils::scratch::scratch_root();
        (
            scratch,
            Arc::new(SqliteMemoryBackend::new(&db_path).unwrap()),
        )
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
        let (_scratch, db) = create_test_db();
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
        let (_scratch, db) = create_test_db();
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
        let (_scratch, db) = create_test_db();

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
        let (_scratch, db) = create_test_db();
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
    async fn replace_mention_links_full_refresh_and_semantic_wins() {
        let (_scratch, db) = create_test_db();
        let a = sample_note("a", "x", vec![]);
        let b = sample_note("b", "x", vec![]);
        db.index_note(&a, AGENT, "x").await.unwrap();
        db.index_note(&b, AGENT, "x").await.unwrap();
        // Pre-existing semantic link a→b.
        db.add_link_with_relation(AGENT, "x/a", "x/b", "related")
            .await
            .unwrap();

        db.replace_mention_links(
            AGENT,
            &[
                ("x/a".to_string(), "x/b".to_string()), // conflicts with semantic → must not clobber
                ("x/b".to_string(), "x/a".to_string()), // fresh mention
            ],
        )
        .await
        .unwrap();

        let a_rows = db.get_outgoing_link_rows("x/a", AGENT).await.unwrap();
        assert_eq!(
            a_rows[0].relation.as_deref(),
            Some("related"),
            "semantic wins"
        );

        let b_rows = db.get_outgoing_link_rows("x/b", AGENT).await.unwrap();
        let m = b_rows.iter().find(|r| r.to_note == "x/a").unwrap();
        assert_eq!(m.relation.as_deref(), Some("mention"));
        assert!((m.confidence - 0.35).abs() < 1e-6);

        // Second refresh with empty set clears stale mention rows.
        db.replace_mention_links(AGENT, &[]).await.unwrap();
        let b_rows = db.get_outgoing_link_rows("x/b", AGENT).await.unwrap();
        assert!(!b_rows
            .iter()
            .any(|r| r.relation.as_deref() == Some("mention")));
    }

    #[tokio::test]
    async fn find_by_filename_returns_paths() {
        let (_scratch, db) = create_test_db();

        let note = sample_note("rust-ownership", "reference", vec![]);
        db.index_note(&note, AGENT, "reference").await.unwrap();

        let paths = db.find_by_filename("rust-ownership", AGENT).await.unwrap();
        assert_eq!(paths, vec!["reference/rust-ownership"]);
    }

    #[tokio::test]
    async fn list_notes_scoped_by_agent() {
        let (_scratch, db) = create_test_db();

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

    /// Seed one resolvable link (`preference/a` -> `reference/rust`, alias
    /// "The Rust Note") and one dangling link (`preference/a` -> `ghost`).
    /// Shared by `index_note_writes_resolution_provenance` and
    /// `graph_reads_exclude_non_active_rows`.
    async fn seed_rust_and_ghost_links(db: &SqliteMemoryBackend) {
        db.index_note(
            &KnowledgeNote {
                title: "rust".into(),
                category: "reference".into(),
                facts: vec!["body".into()],
                content_hash: "h0".into(),
                ..Default::default()
            },
            AGENT,
            "reference",
        )
        .await
        .unwrap();
        db.index_note(
            &KnowledgeNote {
                title: "a".into(),
                category: "preference".into(),
                body: Some("see [[rust|The Rust Note]] and [[ghost]]".into()),
                facts: vec![],
                links: vec!["rust".into(), "ghost".into()],
                content_hash: "h1".into(),
                ..Default::default()
            },
            AGENT,
            "preference",
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn index_note_writes_resolution_provenance() {
        let (_scratch, db) = create_test_db();
        seed_rust_and_ghost_links(&db).await;

        let rows = db
            .get_outgoing_link_rows("preference/a", AGENT)
            .await
            .unwrap();
        let rust = rows.iter().find(|r| r.to_note == "reference/rust").unwrap();
        assert_eq!(rust.status, "active");
        assert_eq!(rust.resolved_by.as_deref(), Some("exact_filename"));
        assert!((rust.confidence - 0.95).abs() < 1e-6);
        assert_eq!(rust.label.as_deref(), Some("The Rust Note"));

        let ghost = rows.iter().find(|r| r.to_note == "ghost").unwrap();
        assert_eq!(ghost.status, "dangling");
        assert_eq!(ghost.confidence, 0.0);
        assert!(ghost.resolved_by.is_none());
    }

    #[tokio::test]
    async fn graph_reads_exclude_non_active_rows() {
        let (_scratch, db) = create_test_db();
        seed_rust_and_ghost_links(&db).await;

        let snap = db.load_graph_snapshot(AGENT).await.unwrap();
        assert!(
            snap.edges.iter().all(|e| e.to == "reference/rust"),
            "snapshot must contain only active resolved edges"
        );
        let (_, edges) = db.get_graph_data(AGENT, 50).await.unwrap();
        assert!(edges.iter().all(|e| e.to == "reference/rust"));
        assert_eq!(edges[0].label.as_deref(), Some("The Rust Note"));
    }

    /// Build a note with a verbatim body and no wikilinks (target of a link,
    /// not a source). Mirrors the literal-struct style of
    /// `seed_rust_and_ghost_links` above.
    fn note_with_body(title: &str, category: &str, body: &str) -> KnowledgeNote {
        KnowledgeNote {
            title: title.into(),
            category: category.into(),
            body: Some(body.into()),
            content_hash: format!("hash_{title}_{body}"),
            ..Default::default()
        }
    }

    /// Build a note with a verbatim body and explicit outgoing `[[links]]`.
    fn note_with_body_links(
        title: &str,
        category: &str,
        body: &str,
        links: &[&str],
    ) -> KnowledgeNote {
        KnowledgeNote {
            title: title.into(),
            category: category.into(),
            body: Some(body.into()),
            links: links.iter().map(|s| s.to_string()).collect(),
            content_hash: format!("hash_{title}_{body}"),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn delete_tombstones_inbound_and_recreate_revives() {
        let (_scratch, db) = create_test_db();
        // b links to a.
        db.index_note(
            &note_with_body("a", "reference", "target body"),
            AGENT,
            "reference",
        )
        .await
        .unwrap();
        db.index_note(
            &note_with_body_links("b", "plan", "see [[a]]", &["a"]),
            AGENT,
            "plan",
        )
        .await
        .unwrap();

        // Delete a → inbound row survives as tombstone, outgoing rows of a gone.
        db.remove_note_index("reference/a", AGENT).await.unwrap();
        let rows = db.get_outgoing_link_rows("plan/b", AGENT).await.unwrap();
        let t = rows
            .iter()
            .find(|r| r.to_raw == "a")
            .expect("row must survive");
        assert_eq!(t.status, "tombstone");
        // Tombstone must NOT appear in graph feeds.
        let snap = db.load_graph_snapshot(AGENT).await.unwrap();
        assert!(snap.edges.is_empty());

        // Recreate a (same filename) → backfill revives the edge.
        db.index_note(
            &note_with_body("a", "reference", "reborn"),
            AGENT,
            "reference",
        )
        .await
        .unwrap();
        let revived = db
            .backfill_inbound_links(AGENT, &["a".into(), "reference/a".into()])
            .await
            .unwrap();
        assert_eq!(revived, 1);
        let rows = db.get_outgoing_link_rows("plan/b", AGENT).await.unwrap();
        let r = rows.iter().find(|r| r.to_raw == "a").unwrap();
        assert_eq!(r.status, "active");
        assert_eq!(r.to_note, "reference/a");
    }
}
