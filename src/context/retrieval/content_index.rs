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
//! into a per-session SQLite FTS5 table. The model retrieves only the
//! relevant slices via BM25 search (`ctx_search`), so a 315 KB log costs a
//! few KB to query instead of 315 KB to re-read.
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
//! # Zero new dependencies
//!
//! `rusqlite`'s `bundled` feature compiles SQLite with `SQLITE_ENABLE_FTS5`,
//! so the `bm25()` ranking function, `snippet()` helper, and the `porter` and
//! `trigram` tokenizers are all available out of the box — no extension
//! loading required.

use std::path::Path;
use std::sync::Mutex;

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

/// FTS5-backed index over offloaded tool output. One instance per session,
/// typically rooted at `~/.aleph/data/tool_results/<session_id>/index.db`,
/// so it shares the [`ToolResultStore`](crate::tools::result_store) lifecycle
/// (Drop cleanup + TTL sweep).
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
        // Two parallel FTS5 tables over the same chunks, fused at query time
        // (see [`Self::search`]). The default external-content-free layout
        // keeps UNINDEXED columns selectable on both, so a fused hit can be
        // rebuilt without a content table.
        //
        // * `chunks`     — `porter unicode61`: stemming + diacritic folding.
        // * `chunks_tri` — `trigram`: 3-gram substring matching for partial
        //   identifiers and typos.
        //
        // `IF NOT EXISTS` keeps this backward-compatible: an `index.db` written
        // by an older build (only `chunks`) simply gains an empty `chunks_tri`.
        // Its pre-existing rows stay fully searchable via the porter side, and
        // RRF degrades to porter-only ordering for them.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
                 title,
                 body,
                 source UNINDEXED,
                 chunk_no UNINDEXED,
                 tokenize = 'porter unicode61'
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS chunks_tri USING fts5(
                 title,
                 body,
                 source UNINDEXED,
                 chunk_no UNINDEXED,
                 tokenize = 'trigram'
             );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // Poison-safe per project rule P7: a panic in another holder must not
        // wedge the index.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Chunk `text` into ~[`DEFAULT_CHUNK_LINES`]-line sections and index them
    /// under `source`. `title_hint` seeds synthesized titles for chunks whose
    /// first line is blank. Returns how many sections were written plus title
    /// previews for the first few (for the model-facing marker).
    ///
    /// Empty / whitespace-only input writes nothing and returns 0 sections.
    pub fn index_text(
        &self,
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
            // fusion and double-counting sections.
            tx.execute("DELETE FROM chunks WHERE source = ?1", params![source])?;
            tx.execute("DELETE FROM chunks_tri WHERE source = ?1", params![source])?;
            let mut stmt = tx.prepare(
                "INSERT INTO chunks (title, body, source, chunk_no) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut stmt_tri = tx.prepare(
                "INSERT INTO chunks_tri (title, body, source, chunk_no) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (i, chunk) in chunks.iter().enumerate() {
                let title = chunk_title(chunk, title_hint, i);
                if previews.len() < PREVIEW_COUNT {
                    previews.push(title.clone());
                }
                // Same chunk into both indexes; the (source, chunk_no) pair is
                // the logical identity used to fuse the two result lists.
                stmt.execute(params![title, chunk, source, i as i64])?;
                stmt_tri.execute(params![title, chunk, source, i as i64])?;
            }
        }
        tx.commit()?;

        Ok(IndexOutcome {
            sections: chunks.len(),
            previews,
        })
    }

    /// Hybrid lexical search over all indexed chunks. Runs the query against
    /// both the porter-stemmed and trigram indexes, fuses the two ranked lists
    /// with Reciprocal Rank Fusion, and returns up to `limit` hits, most
    /// relevant first.
    ///
    /// RRF ranks purely on list position, so it needs no score normalization
    /// across the two BM25 spaces: a chunk that ranks well on *either*
    /// tokenizer surfaces, and one that ranks well on *both* is boosted. A
    /// query with no indexable terms (all punctuation/symbols) yields an empty
    /// result rather than an error.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, IndexError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(match_expr) = sanitize_fts_query(query) else {
            return Ok(Vec::new());
        };
        // Over-fetch from each index so the fusion has enough overlap to work
        // with; the floor keeps a small `limit` (e.g. 3) from starving it.
        let fetch = limit.saturating_mul(OVERFETCH_FACTOR).max(MIN_FETCH);
        let conn = self.lock();
        let porter = query_index(&conn, "chunks", &match_expr, fetch)?;
        // The trigram side is best-effort: a query whose every term is shorter
        // than 3 chars matches nothing there, and a trigram quirk must never
        // fail a search the porter index already answered. Degrade to
        // porter-only on any trigram error.
        let trigram = query_index(&conn, "chunks_tri", &match_expr, fetch).unwrap_or_default();
        drop(conn);
        Ok(rrf_fuse(porter, trigram, limit))
    }

    /// Total number of indexed chunks across all sources. Cheap; used by the
    /// `ctx_search` tool to tell the model whether anything is indexed yet.
    pub fn len(&self) -> Result<usize, IndexError> {
        let conn = self.lock();
        let n: i64 = conn.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// True iff no chunks are indexed.
    pub fn is_empty(&self) -> Result<bool, IndexError> {
        Ok(self.len()? == 0)
    }
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

/// One ranked row from a single FTS5 index, before fusion. Carries the display
/// fields so the fused [`SearchHit`] is built without a second SQLite round-trip.
struct RankedRow {
    source: String,
    chunk_no: i64,
    title: String,
    snippet: String,
}

/// Run `match_expr` against one FTS5 `table` with title-weighted BM25,
/// returning up to `fetch` rows in rank order (best first). `table` is an
/// internal constant (`"chunks"` / `"chunks_tri"`), never user input, so
/// interpolating it into the SQL is injection-safe.
fn query_index(
    conn: &Connection,
    table: &str,
    match_expr: &str,
    fetch: usize,
) -> Result<Vec<RankedRow>, IndexError> {
    let sql = format!(
        "SELECT source, chunk_no, title,
                snippet({table}, 1, '', '', ' … ', 14) AS snip
         FROM {table}
         WHERE {table} MATCH ?1
         ORDER BY bm25({table}, {TITLE_WEIGHT})
         LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    // Clamp to i64 so a very large `fetch` cannot truncate to a negative
    // value, which SQLite would interpret as "no limit" (unbounded scan).
    let fetch = i64::try_from(fetch).unwrap_or(i64::MAX);
    let rows = stmt.query_map(params![match_expr, fetch], |row| {
        Ok(RankedRow {
            source: row.get(0)?,
            chunk_no: row.get(1)?,
            title: row.get(2)?,
            snippet: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Fuse two ranked lists with Reciprocal Rank Fusion and return the top
/// `limit` hits. Each list contributes `1 / (RRF_K + rank)` (rank 1-based) to a
/// chunk's fused score, keyed by its `(source, chunk_no)` identity — so a chunk
/// ranked highly by either tokenizer rises, and one ranked by both rises
/// further. Display fields come from whichever list saw the chunk first
/// (porter precedes trigram), whose stemmed snippet tends to frame the match
/// better.
///
/// Pure and SQLite-free so the fusion math is unit-testable in isolation.
fn rrf_fuse(porter: Vec<RankedRow>, trigram: Vec<RankedRow>, limit: usize) -> Vec<SearchHit> {
    use std::collections::HashMap;

    // `(score, row)` per chunk; `order` preserves first-seen sequence so ties
    // resolve deterministically in porter-then-trigram precedence.
    let mut acc: HashMap<(String, i64), (f64, RankedRow)> = HashMap::new();
    let mut order: Vec<(String, i64)> = Vec::new();

    for list in [porter, trigram] {
        for (rank, row) in list.into_iter().enumerate() {
            let key = (row.source.clone(), row.chunk_no);
            let contrib = 1.0 / (RRF_K + (rank as f64 + 1.0));
            match acc.get_mut(&key) {
                Some(entry) => entry.0 += contrib,
                None => {
                    order.push(key.clone());
                    acc.insert(key, (contrib, row));
                }
            }
        }
    }

    let mut hits: Vec<SearchHit> = order
        .into_iter()
        .filter_map(|key| {
            let (score, row) = acc.remove(&key)?;
            Some(SearchHit {
                source: row.source,
                chunk_no: row.chunk_no,
                title: row.title,
                snippet: row.snippet,
                score,
            })
        })
        .collect();
    // Descending by fused score; the sort is stable, so exact ties keep the
    // porter-first ordering established in `order`.
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    hits
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
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let out = idx.index_text("call_1", "bash", &sample_log()).unwrap();
        assert!(
            out.sections >= 5,
            "expected several chunks, got {}",
            out.sections
        );

        let hits = idx.search("payment refund failed", 5).unwrap();
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
        idx.index_text("call_1", "bash", &sample_log()).unwrap();
        // "database timeout" should surface the line-77 chunk above noise.
        let hits = idx.search("database connection timeout", 3).unwrap();
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
        let out = idx.index_text("call_1", "bash", "   \n  \n").unwrap();
        assert_eq!(out.sections, 0);
        assert!(out.previews.is_empty());
        assert!(idx.is_empty().unwrap());
    }

    #[test]
    fn search_with_no_indexable_terms_is_empty_not_error() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("call_1", "bash", &sample_log()).unwrap();
        let hits = idx.search("()[]{}!!! ---", 5).unwrap();
        assert!(hits.is_empty(), "punctuation-only query must not error");
    }

    #[test]
    fn search_tolerates_punctuation_in_query() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("call_1", "bash", &sample_log()).unwrap();
        // Raw error-style query with FTS operators — must not raise.
        let hits = idx
            .search("ERROR: database connection timeout (30s)", 5)
            .unwrap();
        assert!(!hits.is_empty());
    }

    #[test]
    fn no_match_returns_empty() {
        let idx = ContentIndex::open_in_memory().unwrap();
        idx.index_text("call_1", "bash", &sample_log()).unwrap();
        let hits = idx.search("quantum chromodynamics supernova", 5).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn previews_cap_at_preview_count() {
        let idx = ContentIndex::open_in_memory().unwrap();
        let out = idx.index_text("call_1", "bash", &sample_log()).unwrap();
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
            idx.index_text("call_1", "bash", &sample_log()).unwrap();
        }
        // Reopen and confirm the data survived.
        let idx2 = ContentIndex::open(&db).unwrap();
        assert!(!idx2.is_empty().unwrap());
        let hits = idx2.search("payment refund", 3).unwrap();
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
            "call_1",
            "code",
            "fn getUserPaymentRefund() {\n    todo!()\n}\n",
        )
        .unwrap();

        let hits = idx.search("userpayment", 5).unwrap();
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
        idx.index_text("call_1", "bash", &sample_log()).unwrap();
        assert!(idx.search("payment", 0).unwrap().is_empty());
    }

    fn row(source: &str, chunk_no: i64) -> RankedRow {
        RankedRow {
            source: source.to_string(),
            chunk_no,
            title: format!("{source}#{chunk_no}"),
            snippet: String::new(),
        }
    }

    #[test]
    fn rrf_fuse_boosts_chunks_ranked_by_both_indexes() {
        // porter: [A, B]   trigram: [B, C]
        // B is the only chunk in both lists, so it must fuse to the top even
        // though it is not first in either individual list.
        let porter = vec![row("A", 0), row("B", 0)];
        let trigram = vec![row("B", 0), row("C", 0)];
        let fused = rrf_fuse(porter, trigram, 10);

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
    fn rrf_fuse_respects_limit() {
        let porter = vec![row("A", 0), row("B", 0), row("C", 0)];
        let trigram = vec![];
        let fused = rrf_fuse(porter, trigram, 2);
        assert_eq!(fused.len(), 2);
        // Porter-only: order preserved (A, B), C truncated.
        assert_eq!(fused[0].source, "A");
        assert_eq!(fused[1].source, "B");
    }
}
