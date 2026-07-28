//! `ContentIndex` — an FTS5-backed, BM25-ranked index over large tool
//! outputs that have been offloaded out of the context window.
//!
//! # Why this exists
//!
//! [`crate::tools::result_store::ToolResultStore`] already evicts oversized
//! tool results to disk and hands the model a `[Full output persisted: …]`
//! marker. But the only way the model could pull that data back was
//! `read_file`, which dumps the *entire* blob into context — a 315 KB build
//! log would cost 315 KB to re-read, defeating the offload.
//!
//! `ContentIndex` closes that gap: the offloaded text is chunked and indexed
//! into a `SQLite` FTS5 table, tagged with the owning session (every read and
//! delete carries that scope predicate — see [`ContentIndex`]). The model
//! retrieves only the relevant slices via BM25 search (`ctx_search`), so a
//! 315 KB log costs a few KB to query instead of 315 KB to re-read.
//!
//! # Hybrid lexical retrieval (porter + trigram, fused with RRF)
//!
//! Offloaded output is a mix of prose (build logs, web fetches) and code
//! (identifiers, paths, JSON keys). No single tokenizer is good at both, so
//! the same chunks are indexed into two FTS5 tables — a `porter unicode61`
//! index (stemming + diacritic folding, strong on natural language) and a
//! `trigram` index (3-gram substring matching, strong on partial identifiers
//! and typos the stemmer whiffs). A query runs against both and the two
//! ranked lists are merged with Reciprocal Rank Fusion (no cross-space score
//! normalization needed). This lifts recall over a single-tokenizer BM25
//! index while keeping the `search` contract unchanged.
//!
//! # Proximity reranking (multi-term queries)
//!
//! RRF, like the BM25 it fuses, is a bag-of-words signal — it scores a chunk by
//! how each term ranks, blind to whether the terms sit *together*. So a chunk
//! where `database`, `connection`, `timeout` land on one line fuses identically
//! to one where they scatter across 20 unrelated lines. For queries with ≥2
//! distinct terms, a pure post-fusion pass ([`proximity_relevance`]) boosts each
//! candidate by how tightly its body clusters the matched terms (smallest word
//! span covering the most distinct terms) and how many it covers, then re-sorts.
//! The boost is bounded so it reorders near-ties without overriding a decisive
//! RRF lead; single-term queries skip it and keep RRF order byte-for-byte.
//!
//! # Zero new dependencies
//!
//! `rusqlite`'s `bundled` feature compiles `SQLite` with `SQLITE_ENABLE_FTS5`,
//! so the `bm25()` ranking function, `snippet()` helper, and the `porter` and
//! `trigram` tokenizers are all available out of the box — no extension
//! loading required.

use crate::sync_primitives::Mutex;
use std::path::Path;

use rusqlite::{params, Connection};

/// Number of source lines per indexed chunk. Mirrors context-mode's 20-line
/// chunking: small enough that a BM25 hit returns a focused slice, large
/// enough that a single logical section (a stack trace, a test failure) is
/// usually self-contained.
const DEFAULT_CHUNK_LINES: usize = 20;

/// Max characters retained for a chunk title (UTF-8-safe truncation).
const MAX_TITLE_CHARS: usize = 100;

/// Errors surfaced by the content index. Kept local (not `AlephError`) so the
/// module stays decoupled — callers translate or log-and-fall-back as needed.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Result of indexing one blob of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOutcome {
    /// Number of chunks (sections) written.
    pub sections: usize,
    /// Short title previews for the first few sections, in document order.
    /// Used to build the compact marker the model sees in place of the blob.
    pub previews: Vec<String>,
}

/// A single BM25 search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Source label the chunk was indexed under (e.g. a tool name or call id).
    pub source: String,
    /// Zero-based chunk ordinal within its source.
    pub chunk_no: i64,
    /// Chunk title (first meaningful line, or a synthesized label).
    pub title: String,
    /// A short excerpt of the body around the match.
    pub snippet: String,
    /// Fused relevance score from Reciprocal Rank Fusion across the porter and
    /// trigram indexes. Higher is more relevant; hits are returned pre-sorted
    /// descending, so callers can rely on order without re-sorting by score.
    pub score: f64,
}

/// FTS5-backed index over offloaded tool output.
///
/// The database is shared by every concurrent session (one `index.db` under
/// the process's `tool_results/` root); **rows are tagged with the owning
/// `session_id` and every read/delete is scoped by it**. The index used to be
/// documented as "one instance per session" while boot in fact installed a
/// single `"global"` one, so `ctx_search` could surface another agent's tool
/// output and a purge in one session wiped every other session's rows. The
/// scope predicate — not the file layout — is what enforces INV-ISO now.
pub struct ContentIndex {
    conn: Mutex<Connection>,
}

impl ContentIndex {
    /// Open (creating if absent) an index at `db_path` and ensure the schema.
    pub fn open(db_path: &Path) -> Result<Self, IndexError> {
        let conn = Connection::open(db_path)?;
        Self::from_conn(conn)
    }

    /// Open an ephemeral in-memory index. Used by tests and by callers that
    /// want indexing without touching disk.
    pub fn open_in_memory() -> Result<Self, IndexError> {
        let conn = Connection::open_in_memory()?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Self, IndexError> {
        drop_pre_scope_tables(&conn)?;
        // Two parallel FTS5 tables over the same chunks, fused at query time
        // (see [`Self::search`]). The default external-content-free layout
        // keeps UNINDEXED columns selectable on both, so a fused hit can be
        // rebuilt without a content table.
        //
        // * `chunks`     — `porter unicode61`: stemming + diacritic folding.
        // * `chunks_tri` — `trigram`: 3-gram substring matching for partial
        //   identifiers and typos.
        //
        // `session_id` is the isolation key: it is UNINDEXED (never tokenized,
        // never matched by BM25) but still a real column, so it can carry the
        // `WHERE session_id = ?` predicate every read and delete applies.
        //
        // `IF NOT EXISTS` keeps this backward-compatible: an `index.db` written
        // by an older build (only `chunks`) simply gains an empty `chunks_tri`.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
                 title,
                 body,
                 source UNINDEXED,
                 chunk_no UNINDEXED,
                 session_id UNINDEXED,
                 tokenize = 'porter unicode61'
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS chunks_tri USING fts5(
                 title,
                 body,
                 source UNINDEXED,
                 chunk_no UNINDEXED,
                 session_id UNINDEXED,
                 tokenize = 'trigram'
             );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> crate::sync_primitives::MutexGuard<'_, Connection> {
        // Poison-safe per project rule P7: a panic in another holder must not
        // wedge the index.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Chunk `text` into ~[`DEFAULT_CHUNK_LINES`]-line sections and index them
    /// under `source`, owned by `session_id`. `title_hint` seeds synthesized
    /// titles for chunks whose first line is blank. Returns how many sections
    /// were written plus title previews for the first few (for the model-facing
    /// marker).
    ///
    /// Empty / whitespace-only input writes nothing and returns 0 sections.
    pub fn index_text(
        &self,
        session_id: &str,
        source: &str,
        title_hint: &str,
        text: &str,
    ) -> Result<IndexOutcome, IndexError> {
        let chunks = chunk_lines(text, DEFAULT_CHUNK_LINES);
        if chunks.is_empty() {
            return Ok(IndexOutcome {
                sections: 0,
                previews: Vec::new(),
            });
        }

        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut previews = Vec::new();
        {
            // Re-indexing the same `source` replaces its prior chunks rather
            // than appending. `(source, chunk_no)` is the logical identity used
            // by RRF fusion; a second insert under the same source (e.g. a tool
            // result replayed on retry) would otherwise produce duplicate
            // `chunk_no`s, silently merging physically distinct chunks during
            // fusion and double-counting sections. The replace is session-scoped
            // too — two sessions can legitimately hold the same `source` label
            // (tool names repeat), and one must not evict the other's chunks.
            tx.execute(
                "DELETE FROM chunks WHERE source = ?1 AND session_id = ?2",
                params![source, session_id],
            )?;
            tx.execute(
                "DELETE FROM chunks_tri WHERE source = ?1 AND session_id = ?2",
                params![source, session_id],
            )?;
            let mut stmt = tx.prepare(
                "INSERT INTO chunks (title, body, source, chunk_no, session_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            let mut stmt_tri = tx.prepare(
                "INSERT INTO chunks_tri (title, body, source, chunk_no, session_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (i, chunk) in chunks.iter().enumerate() {
                let title = chunk_title(chunk, title_hint, i);
                if previews.len() < PREVIEW_COUNT {
                    previews.push(title.clone());
                }
                // Same chunk into both indexes; the (source, chunk_no) pair is
                // the logical identity used to fuse the two result lists.
                stmt.execute(params![title, chunk, source, i as i64, session_id])?;
                stmt_tri.execute(params![title, chunk, source, i as i64, session_id])?;
            }
        }
        tx.commit()?;

        Ok(IndexOutcome {
            sections: chunks.len(),
            previews,
        })
    }

    /// Hybrid lexical search over the chunks owned by `session_id`. Runs the
    /// query against both the porter-stemmed and trigram indexes, fuses the two
    /// ranked lists with Reciprocal Rank Fusion, applies a proximity rerank for
    /// multi-term queries, and returns up to `limit` hits, most relevant first.
    ///
    /// The `session_id` predicate is the isolation boundary: the database is
    /// shared across concurrent sessions, so a search that omitted it would let
    /// one agent read another agent's offloaded tool output.
    ///
    /// RRF ranks purely on list position, so it needs no score normalization
    /// across the two BM25 spaces: a chunk that ranks well on *either*
    /// tokenizer surfaces, and one that ranks well on *both* is boosted. A
    /// query with no indexable terms (all punctuation/symbols) yields an empty
    /// result rather than an error.
    ///
    /// # Proximity rerank (multi-term queries)
    ///
    /// BM25 (and therefore the RRF ordering built from it) is a bag-of-words
    /// signal: a chunk where `database`, `connection`, `timeout` land scattered
    /// across 20 unrelated lines fuses identically to one where they sit on the
    /// same line. For queries with ≥2 distinct terms, a pure post-fusion pass
    /// boosts each candidate by how tightly its body clusters the matched terms
    /// (smallest word-span covering the most distinct terms) and how many it
    /// covers — see [`proximity_relevance`]. The boost is bounded so it reorders
    /// near-ties without letting proximity override a decisive RRF lead.
    /// Single-term queries skip the pass entirely, leaving RRF order byte-for-
    /// byte unchanged.
    pub fn search(
        &self,
        session_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, IndexError> {
        self.search_sessions(&[session_id], query, limit)
    }

    /// [`Self::search`] over a *set* of session ids, ranked as one pool.
    ///
    /// Exists for epoch-aware retrieval: a compaction-driven session split
    /// moves the run to `epoch + 1` but seeds the child with the parent's
    /// `[Full output persisted: …]` markers verbatim, so the child's
    /// `ctx_search` must also see rows keyed to earlier epochs of the same
    /// base session key. Callers own the trust boundary — every id passed
    /// must belong to one trust domain (epochs of one key do; two different
    /// agents' keys do not). An empty set matches nothing, never everything.
    pub fn search_sessions(
        &self,
        session_ids: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, IndexError> {
        if limit == 0 || session_ids.is_empty() {
            return Ok(Vec::new());
        }
        let Some(match_expr) = sanitize_fts_query(query) else {
            return Ok(Vec::new());
        };
        // Over-fetch from each index so the fusion has enough overlap to work
        // with; the floor keeps a small `limit` (e.g. 3) from starving it.
        let fetch = limit.saturating_mul(OVERFETCH_FACTOR).max(MIN_FETCH);
        let conn = self.lock();
        let porter = query_index(&conn, "chunks", session_ids, &match_expr, fetch)?;
        // The trigram side is best-effort: a query whose every term is shorter
        // than 3 chars matches nothing there, and a trigram quirk must never
        // fail a search the porter index already answered. Degrade to
        // porter-only on any trigram error.
        let trigram =
            query_index(&conn, "chunks_tri", session_ids, &match_expr, fetch).unwrap_or_default();
        drop(conn);

        let mut fused = rrf_fuse(porter, trigram);
        // Proximity rerank is meaningful only when there are ≥2 distinct terms
        // to measure the span between; a single term leaves RRF order intact.
        let terms = query_terms(query);
        if terms.len() >= 2 {
            proximity_rerank(&mut fused, &terms);
        }
        Ok(finalize(fused, limit))
    }

    /// Number of chunks indexed by `session_id`, across all its sources. Cheap;
    /// used by the `ctx_search` tool to tell the model whether anything of *its
    /// own* is indexed yet — a global count would leak the existence of other
    /// sessions' output.
    pub fn len(&self, session_id: &str) -> Result<usize, IndexError> {
        self.len_sessions(&[session_id])
    }

    /// [`Self::len`] over a set of session ids (see [`Self::search_sessions`]
    /// for why a set: epoch-aware retrieval after a session split). An empty
    /// set counts nothing.
    pub fn len_sessions(&self, session_ids: &[&str]) -> Result<usize, IndexError> {
        if session_ids.is_empty() {
            return Ok(0);
        }
        let placeholders: Vec<String> = (1..=session_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT count(*) FROM chunks WHERE session_id IN ({})",
            placeholders.join(", ")
        );
        let conn = self.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = session_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let n: i64 = stmt.query_row(params.as_slice(), |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Distinct session ids that currently own rows in either FTS table. Used
    /// by the tool-result TTL sweeper to map removed blob directories back to
    /// their index rows: the dir name is a sanitized, non-invertible form of
    /// the key, so the sweep matches *forward* — list ids, sanitize each,
    /// compare against the removed dir names.
    pub fn list_sessions(&self) -> Result<Vec<String>, IndexError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT session_id FROM chunks UNION SELECT session_id FROM chunks_tri")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// True iff `session_id` has no chunks indexed.
    pub fn is_empty(&self, session_id: &str) -> Result<bool, IndexError> {
        Ok(self.len(session_id)? == 0)
    }

    /// Drop every chunk owned by `session_id` from both FTS tables, leaving the
    /// schema (and the live `SQLite` connection) intact so the index stays
    /// usable afterwards.
    ///
    /// Used by the sandbox reference-bypass defense: when a session trips the
    /// denial circuit-breaker, *that session's* offloaded-output index is wiped
    /// so the agent cannot mine previously-cached results via `ctx_search`. The
    /// scope predicate is load-bearing — an unscoped `DELETE` here wiped every
    /// concurrent session's index, leaving their `[Full output persisted: …]`
    /// markers pointing at rows (and blobs) that no longer exist.
    pub fn clear(&self, session_id: &str) -> Result<(), IndexError> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM chunks WHERE session_id = ?1",
            params![session_id],
        )?;
        conn.execute(
            "DELETE FROM chunks_tri WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }
}

/// Drop `chunks` / `chunks_tri` written by a build that predates the
/// `session_id` column.
///
/// FTS5 virtual tables have no `ALTER TABLE … ADD COLUMN`, so a pre-scope
/// `index.db` cannot be upgraded in place — and every statement below would
/// fail against it, which on the daemon's boot path means the whole retrieval
/// layer errors out on upgrade. We therefore **recreate**: the table is dropped
/// and rebuilt empty. Only BM25 recall over a previous process's results is
/// lost; the offloaded `.txt` blobs those rows pointed at are untouched and
/// still reachable through their `[Full output persisted: …]` markers.
fn drop_pre_scope_tables(conn: &Connection) -> Result<(), IndexError> {
    for table in ["chunks", "chunks_tri"] {
        let exists: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |r| r.get(0),
        )?;
        if exists == 0 {
            continue;
        }
        // `prepare` is the cheapest column probe that works on a virtual table
        // (`PRAGMA table_info` would too, but needs row parsing). It fails iff
        // the column is absent.
        if conn
            .prepare(&format!("SELECT session_id FROM {table} LIMIT 0"))
            .is_err()
        {
            tracing::warn!(
                table,
                "content index predates session scoping; recreating it empty \
                 (offloaded blobs remain readable via their markers)"
            );
            conn.execute_batch(&format!("DROP TABLE {table};"))?;
        }
    }
    Ok(())
}

/// Number of section titles surfaced in the offload marker preview.
const PREVIEW_COUNT: usize = 5;

/// Reciprocal Rank Fusion constant (Cormack et al. 2009). 60 is the
/// widely-cited default: it damps deep ranks so the head of each list
/// dominates, without letting either list veto the other.
const RRF_K: f64 = 60.0;

/// BM25 column-weight multiplier for the `title` column. A keyword in a
/// chunk's heading is a stronger relevance signal than the same keyword buried
/// in the body, so titles weigh 5×. Unspecified columns default to 1.0.
const TITLE_WEIGHT: f64 = 5.0;

/// Per-index over-fetch multiple of the caller's `limit`, giving RRF enough
/// candidates from each tokenizer to find genuine overlap before truncating.
const OVERFETCH_FACTOR: usize = 4;

/// Lower bound on per-index fetch, so a tiny `limit` still pulls a usable
/// candidate pool from each side.
const MIN_FETCH: usize = 20;

/// Ceiling on the multiplicative proximity boost. A hit with perfect
/// term-coverage *and* perfect adjacency (`relevance == 1`) has its fused score
/// lifted by this fraction — chosen to overtake a near-tie without leapfrogging
/// a chunk whose RRF lead is decisive. RRF head scores cluster within ~`RRF_K`,
/// so a ~35% lift reorders the contested band only.
const PROXIMITY_BOOST: f64 = 0.35;

/// Weight of the coverage signal (how many distinct query terms appear) in the
/// blended proximity relevance. Coverage and proximity weights sum to 1.0.
const COVERAGE_WEIGHT: f64 = 0.5;

/// Weight of the tightness signal (how close the matched terms sit) in the
/// blended proximity relevance.
const PROXIMITY_WEIGHT: f64 = 0.5;

/// Upper bound on the number of distinct query terms the proximity reranker
/// tracks, set by the `u64` bitmask width used in [`min_window_span`]. Real
/// `ctx_search` queries are a handful of keywords, so this is never reached in
/// practice; terms beyond it simply do not contribute to the proximity signal.
const MAX_PROX_TERMS: usize = 64;

/// One ranked row from a single FTS5 index, before fusion. Carries the display
/// fields so the fused [`SearchHit`] is built without a second `SQLite` round-trip,
/// plus the full chunk `body` used by the proximity reranker (never surfaced to
/// callers — it is dropped when the fused list is finalized into `SearchHit`s).
struct RankedRow {
    source: String,
    chunk_no: i64,
    title: String,
    snippet: String,
    body: String,
}

/// Run `match_expr` against one FTS5 `table` with title-weighted BM25, over the
/// rows owned by any of `session_ids`, returning up to `fetch` rows in rank
/// order (best first). `table` is an internal constant (`"chunks"` /
/// `"chunks_tri"`), never user input, so interpolating it into the SQL is
/// injection-safe; the session ids are bound, not interpolated.
fn query_index(
    conn: &Connection,
    table: &str,
    session_ids: &[&str],
    match_expr: &str,
    fetch: usize,
) -> Result<Vec<RankedRow>, IndexError> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    // `?1` is the MATCH expression, `?2..` the session ids, the last
    // placeholder the LIMIT.
    let id_placeholders: Vec<String> = (0..session_ids.len())
        .map(|i| format!("?{}", i + 2))
        .collect();
    let limit_pos = session_ids.len() + 2;
    let sql = format!(
        "SELECT source, chunk_no, title,
                snippet({table}, 1, '', '', ' … ', 14) AS snip,
                body
         FROM {table}
         WHERE {table} MATCH ?1 AND session_id IN ({ids})
         ORDER BY bm25({table}, {TITLE_WEIGHT})
         LIMIT ?{limit_pos}",
        ids = id_placeholders.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    // Clamp to i64 so a very large `fetch` cannot truncate to a negative
    // value, which SQLite would interpret as "no limit" (unbounded scan).
    let fetch = i64::try_from(fetch).unwrap_or(i64::MAX);
    let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(session_ids.len() + 2);
    bound.push(&match_expr);
    for id in session_ids {
        bound.push(id);
    }
    bound.push(&fetch);
    let rows = stmt.query_map(bound.as_slice(), |row| {
        Ok(RankedRow {
            source: row.get(0)?,
            chunk_no: row.get(1)?,
            title: row.get(2)?,
            snippet: row.get(3)?,
            body: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// A chunk after RRF fusion, before finalization. Holds the fused `score` plus
/// the full `body`, which the proximity reranker needs but callers never see
/// (it is dropped in [`finalize`]).
#[derive(Debug)]
struct FusedHit {
    source: String,
    chunk_no: i64,
    title: String,
    snippet: String,
    body: String,
    score: f64,
}

/// Fuse two ranked lists with Reciprocal Rank Fusion, returning *all* fused
/// candidates sorted by score (best first) — truncation to the caller's limit
/// happens later in [`finalize`] so the proximity reranker can see the whole
/// candidate pool first. Each list contributes `1 / (RRF_K + rank)` (rank
/// 1-based) to a chunk's fused score, keyed by its `(source, chunk_no)`
/// identity — so a chunk ranked highly by either tokenizer rises, and one
/// ranked by both rises further. Display fields come from whichever list saw
/// the chunk first (porter precedes trigram), whose stemmed snippet tends to
/// frame the match better.
///
/// Pure and SQLite-free so the fusion math is unit-testable in isolation.
fn rrf_fuse(porter: Vec<RankedRow>, trigram: Vec<RankedRow>) -> Vec<FusedHit> {
    use std::collections::HashMap;

    // `(score, row)` per chunk; `order` preserves first-seen sequence so ties
    // resolve deterministically in porter-then-trigram precedence.
    let mut acc: HashMap<(String, i64), (f64, RankedRow)> = HashMap::new();
    let mut order: Vec<(String, i64)> = Vec::new();

    for list in [porter, trigram] {
        for (rank, row) in list.into_iter().enumerate() {
            let key = (row.source.clone(), row.chunk_no);
            let contrib = 1.0 / (RRF_K + (rank as f64 + 1.0));
            match acc.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().0 += contrib;
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    order.push(entry.key().clone());
                    entry.insert((contrib, row));
                }
            }
        }
    }

    let mut hits: Vec<FusedHit> = order
        .into_iter()
        .filter_map(|key| {
            let (score, row) = acc.remove(&key)?;
            Some(FusedHit {
                source: row.source,
                chunk_no: row.chunk_no,
                title: row.title,
                snippet: row.snippet,
                body: row.body,
                score,
            })
        })
        .collect();
    sort_by_score_desc(&mut hits);
    hits
}

/// Stable descending sort by fused `score`. Ties keep their incoming order, so
/// after [`rrf_fuse`] exact ties preserve porter-first precedence.
fn sort_by_score_desc(hits: &mut [FusedHit]) {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Boost each fused hit by how tightly and how completely its body clusters the
/// query `terms`, then re-sort. The boost is multiplicative and bounded:
/// `score *= 1 + PROXIMITY_BOOST * relevance`, with `relevance ∈ [0, 1]`. A
/// perfectly adjacent full-coverage match lifts a hit by at most
/// `PROXIMITY_BOOST`, enough to overtake a near-tie but not to leapfrog a hit
/// with a decisively higher RRF lead. Hits whose body contains none of the
/// terms (the snippet matched via a stem/trigram the raw body lacks verbatim)
/// keep `relevance == 0` and are left exactly where RRF put them.
///
/// Caller guarantees `terms.len() >= 2`. Pure; no `SQLite`, unit-testable.
fn proximity_rerank(hits: &mut [FusedHit], terms: &[String]) {
    for hit in hits.iter_mut() {
        let relevance = proximity_relevance(&hit.body, terms);
        hit.score *= 1.0 + PROXIMITY_BOOST * relevance;
    }
    sort_by_score_desc(hits);
}

/// Drop the internal `body` and truncate to `limit`, producing the public
/// [`SearchHit`] list. Kept separate from fusion so truncation runs *after* any
/// proximity rerank.
fn finalize(hits: Vec<FusedHit>, limit: usize) -> Vec<SearchHit> {
    hits.into_iter()
        .take(limit)
        .map(|h| SearchHit {
            source: h.source,
            chunk_no: h.chunk_no,
            title: h.title,
            snippet: h.snippet,
            score: h.score,
        })
        .collect()
}

/// Proximity-and-coverage relevance of `body` for the distinct query `terms`,
/// in `[0, 1]`. Combines two signals:
///
/// * **coverage** — fraction of distinct query terms that appear in the body.
/// * **tightness** — `matched / span`, where `span` is the width (in body
///   words) of the smallest window covering all matched terms. `1.0` when the
///   matched terms are perfectly adjacent, shrinking as they spread apart.
///
/// `relevance = COVERAGE_WEIGHT * coverage + PROXIMITY_WEIGHT * tightness`
/// (weights sum to 1). Term matching is substring containment on lowercased
/// body words, so it aligns with the trigram index (a query `userpayment`
/// matches the body word `getuserpaymentrefund`). Returns `0.0` when no term
/// matches.
///
/// Pure and allocation-light; the dominant cost is one lowercasing pass over
/// the body words. Single-pass min-window via two pointers — `O(words)`.
fn proximity_relevance(body: &str, terms: &[String]) -> f64 {
    let total_terms = terms.len();
    if total_terms == 0 {
        return 0.0;
    }
    // Bitmask of which terms each body word contains. Cap at the bit width so a
    // pathologically long query can't overflow the mask; extra terms simply do
    // not contribute (queries are tiny in practice).
    let n_terms = total_terms.min(MAX_PROX_TERMS);

    // For each matching body word, its word ordinal (counting only non-empty
    // words, so runs of delimiters don't inflate spans) and the mask of terms
    // it contains. Non-matching words are skipped but still advance `pos`.
    let mut matched: Vec<(usize, u64)> = Vec::new();
    let mut seen_any: u64 = 0;
    let mut pos = 0usize;
    for raw in body.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        let word = raw.to_ascii_lowercase();
        let mut mask: u64 = 0;
        for (ti, term) in terms.iter().take(n_terms).enumerate() {
            if word.contains(term.as_str()) {
                mask |= 1u64 << ti;
            }
        }
        if mask != 0 {
            seen_any |= mask;
            matched.push((pos, mask));
        }
        pos += 1;
    }

    let covered = seen_any.count_ones() as usize;
    if covered == 0 {
        return 0.0;
    }
    let coverage = covered as f64 / n_terms as f64;

    // Smallest window (in word positions) covering all `covered` distinct terms.
    // Two-pointer scan over the matched-word list, expanding `right` until the
    // window holds every distinct term, then contracting `left` while it still
    // does — tracking the minimum width seen.
    let span = min_window_span(&matched, covered);
    // `matched.len() == 1` ⇒ a single matched word ⇒ span 1 ⇒ tightness 1.
    let tightness = if span == 0 {
        0.0
    } else {
        (covered as f64) / (span as f64)
    };

    (COVERAGE_WEIGHT * coverage + PROXIMITY_WEIGHT * tightness).clamp(0.0, 1.0)
}

/// Width (in body words, inclusive) of the smallest contiguous window that
/// covers `target` distinct term bits across `matched` `(position, mask)`
/// pairs (positions strictly ascending). Classic sliding-window minimum-cover.
/// Returns `0` when `matched` is empty.
fn min_window_span(matched: &[(usize, u64)], target: usize) -> usize {
    if matched.is_empty() || target == 0 {
        return 0;
    }
    // Per-distinct-term counts inside the current window; a term is "present"
    // while its count > 0. `distinct` tracks how many terms are present.
    let mut counts = [0u32; MAX_PROX_TERMS];
    let mut distinct = 0usize;
    let mut best = usize::MAX;
    let mut left = 0usize;

    for right in 0..matched.len() {
        for ti in iter_bits(matched[right].1) {
            if counts[ti] == 0 {
                distinct += 1;
            }
            counts[ti] += 1;
        }
        // Contract from the left while the window still covers every term.
        while distinct == target {
            let width = matched[right].0 - matched[left].0 + 1;
            if width < best {
                best = width;
            }
            for ti in iter_bits(matched[left].1) {
                counts[ti] -= 1;
                if counts[ti] == 0 {
                    distinct -= 1;
                }
            }
            left += 1;
        }
    }
    best
}

/// Yield the set-bit indices of `mask` (LSB first). Allocation-free.
fn iter_bits(mut mask: u64) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        if mask == 0 {
            None
        } else {
            let ti = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            Some(ti)
        }
    })
}

/// Split `text` into chunks of at most `lines_per_chunk` source lines.
/// Whitespace-only chunks are dropped. Returns owned `String`s ready to index.
fn chunk_lines(text: &str, lines_per_chunk: usize) -> Vec<String> {
    let lines_per_chunk = lines_per_chunk.max(1);
    let lines: Vec<&str> = text.lines().collect();
    lines
        .chunks(lines_per_chunk)
        .map(|group| group.join("\n"))
        .filter(|chunk| !chunk.trim().is_empty())
        .collect()
}

/// Derive a chunk title: the first non-blank line (UTF-8-safe truncated to
/// [`MAX_TITLE_CHARS`]), or a synthesized `"{hint} #{ordinal}"` fallback.
fn chunk_title(chunk: &str, title_hint: &str, ordinal: usize) -> String {
    let first_line = chunk.lines().map(str::trim).find(|l| !l.is_empty());
    match first_line {
        Some(line) => truncate_chars(line, MAX_TITLE_CHARS),
        None => format!("{title_hint} #{}", ordinal + 1),
    }
}

/// UTF-8-safe truncation to at most `max` characters (project rule P7).
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => format!("{}…", &s[..byte_idx]),
        None => s.to_string(),
    }
}

/// Split a raw query into its alphanumeric (Unicode) word tokens — the single
/// tokenizer both [`sanitize_fts_query`] and [`query_terms`] build on, so the
/// FTS `MATCH` expression and the proximity reranker can never disagree on what
/// counts as a term. Tokens are yielded in query order, non-empty, case
/// preserved.
fn split_terms(query: &str) -> impl Iterator<Item = &str> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
}

/// Turn an arbitrary user/LLM query into a safe FTS5 `MATCH` expression.
///
/// FTS5's query grammar treats `(`, `)`, `"`, `*`, `:`, `-`, `^` etc. as
/// operators, so passing a raw tool-error string like `error: foo()` would
/// raise a syntax error. We extract alphanumeric (Unicode) word tokens, wrap
/// each in double quotes, and join them with `OR` so any term can match.
///
/// Returns `None` when the query has no indexable tokens.
///
/// `pub(crate)` so the session-event FTS index (`session::store`) reuses the
/// same hardening instead of duplicating it (rule of three).
pub(crate) fn sanitize_fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = split_terms(query).map(|t| format!("\"{t}\"")).collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Distinct query terms (lowercased, in first-occurrence order) for the
/// proximity reranker. Built on the same [`split_terms`] tokenizer as
/// [`sanitize_fts_query`], so the reranker measures exactly the terms the FTS
/// `MATCH` searched for. Lowercasing aligns substring tests with the
/// case-insensitive porter/trigram indexes; dedup keeps a repeated keyword from
/// inflating the coverage denominator.
fn query_terms(query: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for t in split_terms(query) {
        let lc = t.to_ascii_lowercase();
        if seen.insert(lc.clone()) {
            out.push(lc);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Session scope every single-session test writes and reads under.
    const SESS: &str = "agent:main:s1";

    fn sample_log() -> String {
        let mut s = String::new();
        for i in 0..100 {
            if i == 42 {
                s.push_str("FAILED: test_payment_refund panicked at assert_eq\n");
            } else if i == 77 {
                s.push_str("ERROR: database connection timeout after 30s\n");
            } else {
                s.push_str(&format!("ok line {i} running normally\n"));
            }
        }
        s
    }

    #[test]
    fn index_and_search_finds_relevant_chunk() {
        let idx = ContentIndex::open_in_memory().unwrap();
        let out = idx
            .index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        assert!(
            out.sections >= 5,
            "expected several chunks, got {}",
            out.sections
        );

        let hits = idx.search(SESS, "payment refund failed", 5).unwrap();
        assert!(!hits.is_empty(), "should find the failing-test chunk");
        assert!(
            hits[0].snippet.to_lowercase().contains("payment")
                || hits[0].title.to_lowercase().contains("payment"),
            "top hit should be about payment, got title={:?} snippet={:?}",
            hits[0].title,
            hits[0].snippet
        );
    }

    #[test]
    fn search_ranks_best_match_first() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        // "database timeout" should surface the line-77 chunk above noise.
        let hits = idx.search(SESS, "database connection timeout", 3).unwrap();
        assert!(!hits.is_empty());
        let top = &hits[0];
        assert!(
            top.snippet.to_lowercase().contains("timeout")
                || top.title.to_lowercase().contains("timeout"),
            "top hit should mention timeout, got {top:?}"
        );
    }

    #[test]
    fn empty_text_indexes_nothing() {
        let idx = ContentIndex::open_in_memory().unwrap();
        let out = idx.index_text(SESS, "call_1", "bash", "   \n  \n").unwrap();
        assert_eq!(out.sections, 0);
        assert!(out.previews.is_empty());
        assert!(idx.is_empty(SESS).unwrap());
    }

    #[test]
    fn search_with_no_indexable_terms_is_empty_not_error() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        let hits = idx.search(SESS, "()[]{}!!! ---", 5).unwrap();
        assert!(hits.is_empty(), "punctuation-only query must not error");
    }

    #[test]
    fn search_tolerates_punctuation_in_query() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        // Raw error-style query with FTS operators — must not raise.
        let hits = idx
            .search(SESS, "ERROR: database connection timeout (30s)", 5)
            .unwrap();
        assert!(!hits.is_empty());
    }

    #[test]
    fn no_match_returns_empty() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        let hits = idx
            .search(SESS, "quantum chromodynamics supernova", 5)
            .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn previews_cap_at_preview_count() {
        let idx = ContentIndex::open_in_memory().unwrap();
        let out = idx
            .index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        assert!(out.previews.len() <= PREVIEW_COUNT);
        assert!(!out.previews.is_empty());
    }

    #[test]
    fn chunk_title_uses_first_nonblank_line() {
        let title = chunk_title("\n\n  real heading here\nmore", "bash", 0);
        assert_eq!(title, "real heading here");
    }

    #[test]
    fn chunk_title_falls_back_to_hint() {
        let title = chunk_title("   \n  \n", "bash", 2);
        assert_eq!(title, "bash #3");
    }

    #[test]
    fn truncate_chars_is_utf8_safe() {
        let s = "日本語テキストがとても長い場合の切り詰め確認用テキスト";
        let out = truncate_chars(s, 5);
        assert!(out.ends_with('…'));
        // Must not panic on multi-byte boundaries; prefix is 5 chars + ellipsis.
        assert_eq!(out.chars().count(), 6);
    }

    #[test]
    fn persists_to_disk_and_reopens() {
        let dir = std::env::temp_dir().join("aleph_content_index_test_disk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("index.db");
        {
            let idx = ContentIndex::open(&db).unwrap();
            idx.index_text(SESS, "call_1", "bash", &sample_log())
                .unwrap();
        }
        // Reopen and confirm the data survived.
        let idx2 = ContentIndex::open(&db).unwrap();
        assert!(!idx2.is_empty(SESS).unwrap());
        let hits = idx2.search(SESS, "payment refund", 3).unwrap();
        assert!(!hits.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trigram_matches_substring_that_porter_misses() {
        // `getUserPaymentRefund` is a single token: the porter stemmer indexes
        // it whole, so a substring query like "userpayment" never matches it
        // on the porter side. The trigram index matches it by 3-grams, and RRF
        // surfaces the hit. This is the headline gain of the hybrid upgrade.
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text(
            SESS,
            "call_1",
            "code",
            "fn getUserPaymentRefund() {\n    todo!()\n}\n",
        )
        .unwrap();

        let hits = idx.search(SESS, "userpayment", 5).unwrap();
        assert!(
            !hits.is_empty(),
            "trigram index should match the substring inside the identifier"
        );
        assert!(
            hits[0].snippet.to_lowercase().contains("payment")
                || hits[0].title.to_lowercase().contains("payment"),
            "hit should be the identifier chunk, got {hits:?}"
        );
    }

    #[test]
    fn search_limit_zero_returns_empty() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        assert!(idx.search(SESS, "payment", 0).unwrap().is_empty());
    }

    fn row(source: &str, chunk_no: i64) -> RankedRow {
        RankedRow {
            source: source.to_string(),
            chunk_no,
            title: format!("{source}#{chunk_no}"),
            snippet: String::new(),
            body: String::new(),
        }
    }

    #[test]
    fn rrf_fuse_boosts_chunks_ranked_by_both_indexes() {
        // porter: [A, B]   trigram: [B, C]
        // B is the only chunk in both lists, so it must fuse to the top even
        // though it is not first in either individual list.
        let porter = vec![row("A", 0), row("B", 0)];
        let trigram = vec![row("B", 0), row("C", 0)];
        let fused = rrf_fuse(porter, trigram);

        assert_eq!(fused.len(), 3, "three distinct chunks");
        assert_eq!(
            (fused[0].source.as_str(), fused[0].chunk_no),
            ("B", 0),
            "chunk present in both lists fuses highest, got {fused:?}"
        );
        // A (porter rank 1) outranks C (trigram rank 2): 1/61 > 1/62.
        assert_eq!(fused[1].source, "A");
        assert_eq!(fused[2].source, "C");
    }

    #[test]
    fn finalize_truncates_to_limit_and_drops_body() {
        let porter = vec![row("A", 0), row("B", 0), row("C", 0)];
        let trigram = vec![];
        // `rrf_fuse` no longer truncates; `finalize` does. Porter-only keeps
        // first-seen order (A, B, C), so a limit of 2 yields A, B.
        let hits = finalize(rrf_fuse(porter, trigram), 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].source, "A");
        assert_eq!(hits[1].source, "B");
    }

    // ---- proximity reranking ----

    #[test]
    fn proximity_relevance_zero_when_no_term_present() {
        let terms = vec!["payment".to_string(), "refund".to_string()];
        assert_eq!(
            proximity_relevance("totally unrelated text here", &terms),
            0.0
        );
    }

    #[test]
    fn proximity_relevance_rewards_adjacency() {
        let terms = vec!["payment".to_string(), "refund".to_string()];
        // Adjacent: smallest window spanning both terms is 2 words → tightness 1.
        let tight = proximity_relevance("the payment refund succeeded", &terms);
        // Scattered: same two terms but 8 words apart → much lower tightness.
        let loose = proximity_relevance(
            "payment was issued then much later a separate refund arrived",
            &terms,
        );
        assert!(
            tight > loose,
            "adjacent terms must score above scattered ones (tight={tight}, loose={loose})"
        );
        // Full coverage + perfect adjacency saturates to the max relevance.
        assert!((tight - 1.0).abs() < 1e-9, "expected 1.0, got {tight}");
    }

    #[test]
    fn proximity_relevance_partial_coverage_below_full() {
        let terms = vec!["payment".to_string(), "refund".to_string()];
        // Only one of two terms present: coverage 0.5, single-word span ⇒
        // tightness 1.0 ⇒ relevance = 0.5*0.5 + 0.5*1.0 = 0.75 < full coverage.
        let partial = proximity_relevance("the payment cleared", &terms);
        assert!(
            (partial - 0.75).abs() < 1e-9,
            "expected 0.75 for half coverage, got {partial}"
        );
    }

    #[test]
    fn proximity_relevance_substring_aligns_with_trigram() {
        // Mirrors the trigram path: query term `userpayment` is a substring of
        // the body word `getuserpaymentrefund`, so it must register a match.
        let terms = vec!["userpayment".to_string()];
        let rel = proximity_relevance("fn getUserPaymentRefund() {}", &terms);
        assert!(rel > 0.0, "substring term should match, got {rel}");
    }

    #[test]
    fn min_window_span_finds_tightest_cover() {
        // positions: term0 at 0 and 9, term1 at 8. Tightest cover of {0,1} is
        // the window [8,9] → width 2, not the [0,8] window of width 9.
        let matched = vec![(0usize, 0b01u64), (8, 0b10), (9, 0b01)];
        assert_eq!(min_window_span(&matched, 2), 2);
    }

    #[test]
    fn proximity_rerank_lifts_clustered_chunk_over_scattered_tie() {
        // Two chunks fused to the *same* RRF score; only proximity separates
        // them. The clustered body must end up first after reranking.
        let mut fused = vec![
            FusedHit {
                source: "scattered".to_string(),
                chunk_no: 0,
                title: String::new(),
                snippet: String::new(),
                body: "payment happened and unrelated lines then finally refund".to_string(),
                score: 0.1,
            },
            FusedHit {
                source: "clustered".to_string(),
                chunk_no: 0,
                title: String::new(),
                snippet: String::new(),
                body: "payment refund pair".to_string(),
                score: 0.1,
            },
        ];
        let terms = vec!["payment".to_string(), "refund".to_string()];
        proximity_rerank(&mut fused, &terms);
        assert_eq!(
            fused[0].source, "clustered",
            "tighter term cluster must rerank above the scattered tie"
        );
    }

    #[test]
    fn search_single_term_skips_proximity_and_is_deterministic() {
        // A single-term query has <2 distinct terms, so the proximity pass is
        // skipped and RRF order stands. Assert the single-term path returns a
        // stable, non-empty result across repeated calls.
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text(SESS, "call_1", "bash", &sample_log())
            .unwrap();
        let a = idx.search(SESS, "payment", 3).unwrap();
        let b = idx.search(SESS, "payment", 3).unwrap();
        assert_eq!(a.len(), b.len());
        assert!(!a.is_empty());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(
                (x.source.as_str(), x.chunk_no),
                (y.source.as_str(), y.chunk_no)
            );
        }
    }

    #[test]
    fn search_multi_term_prefers_chunk_with_terms_together() {
        // One chunk has "database connection timeout" on a single line; another
        // mentions only "database" amid noise. A multi-term query must surface
        // the co-located chunk first thanks to the proximity rerank.
        let idx = ContentIndex::open_in_memory().unwrap();
        let mut text = String::new();
        // Chunk 0: only the word "database" scattered with filler (20 lines).
        for i in 0..20 {
            text.push_str(&format!(
                "database mention {i} with assorted filler words\n"
            ));
        }
        // Chunk 1: the three query terms together on one line.
        for _ in 0..20 {
            text.push_str("the database connection timeout occurred here\n");
        }
        idx.index_text(SESS, "call_1", "bash", &text).unwrap();
        let hits = idx.search(SESS, "database connection timeout", 5).unwrap();
        assert!(!hits.is_empty());
        assert!(
            hits[0].snippet.to_lowercase().contains("timeout")
                || hits[0].title.to_lowercase().contains("timeout"),
            "co-located terms should rank first, got {hits:?}"
        );
    }

    #[test]
    fn query_terms_lowercases_and_dedups() {
        let terms = query_terms("Database DATABASE  connection!! database");
        assert_eq!(
            terms,
            vec!["database".to_string(), "connection".to_string()]
        );
    }

    #[test]
    fn sanitize_fts_query_unchanged_by_refactor() {
        // The MATCH expression must stay byte-identical after sharing the
        // `split_terms` tokenizer (case preserved, OR-joined, quoted).
        assert_eq!(
            sanitize_fts_query("ERROR: db Timeout").unwrap(),
            "\"ERROR\" OR \"db\" OR \"Timeout\""
        );
        assert!(sanitize_fts_query("()[]{}").is_none());
    }

    // ---- session scoping (INV-ISO) ----

    #[test]
    fn search_never_crosses_session_boundary() {
        // Both sessions share one index.db (the process installs a single
        // store). Session A must not be able to read B's offloaded output.
        // Sentinels must be single alphanumeric tokens: `split_terms` splits on
        // every non-alphanumeric char and OR-joins, so an underscored sentinel
        // like `secret_from_b` would search for "secret" OR "from" OR "b" and
        // match the *other* session's text on the shared words alone.
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("sess-a", "call_1", "bash", "alpha secretalpha alpha\n")
            .unwrap();
        idx.index_text("sess-b", "call_1", "bash", "beta secretbeta beta\n")
            .unwrap();

        let a = idx.search("sess-a", "secretbeta", 5).unwrap();
        assert!(a.is_empty(), "session A must not see B's output: {a:?}");
        let b = idx.search("sess-b", "secretalpha", 5).unwrap();
        assert!(b.is_empty(), "session B must not see A's output: {b:?}");
        // Each still finds its own.
        assert!(!idx.search("sess-a", "secretalpha", 5).unwrap().is_empty());
        assert_eq!(idx.len("sess-a").unwrap(), 1);
        assert_eq!(idx.len("sess-b").unwrap(), 1);
    }

    #[test]
    fn clear_only_wipes_the_named_session() {
        // The denial circuit-breaker calls this. Before scoping it was an
        // unscoped `DELETE`, so one session's three refusals wiped every
        // concurrent session's index.
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("sess-a", "call_1", "bash", "alpha payload here\n")
            .unwrap();
        idx.index_text("sess-b", "call_1", "bash", "beta payload here\n")
            .unwrap();

        idx.clear("sess-a").unwrap();

        assert!(idx.is_empty("sess-a").unwrap(), "A must be wiped");
        assert_eq!(idx.len("sess-b").unwrap(), 1, "B must survive A's purge");
        assert!(!idx.search("sess-b", "beta payload", 5).unwrap().is_empty());
    }

    #[test]
    fn search_sessions_spans_epochs_of_one_key() {
        // Epoch-aware retrieval: rows written under the parent epoch key must
        // be reachable when the caller searches the child + parent key set.
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("agent:a:main", "call_1", "bash", "epochalpha payload\n")
            .unwrap();
        idx.index_text("agent:a:main:s1", "call_2", "bash", "epochbeta payload\n")
            .unwrap();

        let both = ["agent:a:main:s1", "agent:a:main"];
        assert!(
            !idx.search_sessions(&both, "epochalpha", 5)
                .unwrap()
                .is_empty(),
            "parent-epoch rows must be visible to the widened scope"
        );
        assert!(!idx
            .search_sessions(&both, "epochbeta", 5)
            .unwrap()
            .is_empty());
        assert_eq!(idx.len_sessions(&both).unwrap(), 2);
        // A single-key search stays scoped (no cross-epoch bleed by default).
        assert!(idx
            .search("agent:a:main:s1", "epochalpha", 5)
            .unwrap()
            .is_empty());
        // An empty set matches nothing — never an unscoped scan.
        assert!(idx
            .search_sessions(&[], "epochalpha", 5)
            .unwrap()
            .is_empty());
        assert_eq!(idx.len_sessions(&[]).unwrap(), 0);
    }

    #[test]
    fn list_sessions_returns_distinct_owning_sessions() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("sess-a", "call_1", "bash", "alpha body\n")
            .unwrap();
        idx.index_text("sess-a", "call_2", "bash", "alpha two\n")
            .unwrap();
        idx.index_text("sess-b", "call_1", "bash", "beta body\n")
            .unwrap();
        let mut sessions = idx.list_sessions().unwrap();
        sessions.sort();
        assert_eq!(sessions, vec!["sess-a".to_string(), "sess-b".to_string()]);
    }

    #[test]
    fn same_source_label_in_two_sessions_does_not_evict() {
        // `index_text` replaces prior chunks for a `source`; tool names /
        // call ids repeat across sessions, so the replace must be scoped too.
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("sess-a", "bash", "bash", "alpha unique_a\n")
            .unwrap();
        idx.index_text("sess-b", "bash", "bash", "beta unique_b\n")
            .unwrap();
        assert_eq!(idx.len("sess-a").unwrap(), 1, "B's write must not evict A");
        assert!(!idx.search("sess-a", "unique_a", 5).unwrap().is_empty());
    }

    #[test]
    fn opening_a_pre_scope_index_recreates_it_instead_of_erroring() {
        // An `index.db` from a build before the `session_id` column exists on
        // disk after upgrade. FTS5 cannot ALTER a virtual table, so `open` must
        // recreate it — not propagate a "no such column" error out of boot.
        let dir = std::env::temp_dir().join("aleph_content_index_migration_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("index.db");
        {
            // Hand-build the legacy schema (no `session_id`) and seed a row.
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE VIRTUAL TABLE chunks USING fts5(
                     title, body, source UNINDEXED, chunk_no UNINDEXED,
                     tokenize = 'porter unicode61'
                 );
                 CREATE VIRTUAL TABLE chunks_tri USING fts5(
                     title, body, source UNINDEXED, chunk_no UNINDEXED,
                     tokenize = 'trigram'
                 );
                 INSERT INTO chunks (title, body, source, chunk_no)
                     VALUES ('t', 'legacy body', 'call_1', 0);",
            )
            .unwrap();
        }

        let idx = ContentIndex::open(&db).expect("pre-scope index must not fail open");
        // Recreated empty — the legacy rows are gone, but the store is usable
        // and the offloaded blobs they pointed at are untouched on disk.
        assert!(idx.is_empty(SESS).unwrap());
        idx.index_text(SESS, "call_2", "bash", "fresh body here\n")
            .unwrap();
        assert!(!idx.search(SESS, "fresh body", 5).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
