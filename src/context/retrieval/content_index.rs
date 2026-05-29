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
//! # Zero new dependencies
//!
//! `rusqlite`'s `bundled` feature compiles SQLite with `SQLITE_ENABLE_FTS5`,
//! so the `bm25()` ranking function and `snippet()` helper are available
//! out of the box — no extension loading required.

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
    /// Raw BM25 score (more negative = more relevant; lower sorts first).
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
        // `content=''` would make it contentless (no column readback); we keep
        // the default external-content-free table so UNINDEXED columns remain
        // selectable. porter+unicode61 gives stemming + diacritic folding.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
                 title,
                 body,
                 source UNINDEXED,
                 chunk_no UNINDEXED,
                 tokenize = 'porter unicode61'
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
            let mut stmt = tx.prepare(
                "INSERT INTO chunks (title, body, source, chunk_no) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (i, chunk) in chunks.iter().enumerate() {
                let title = chunk_title(chunk, title_hint, i);
                if previews.len() < PREVIEW_COUNT {
                    previews.push(title.clone());
                }
                stmt.execute(params![title, chunk, source, i as i64])?;
            }
        }
        tx.commit()?;

        Ok(IndexOutcome {
            sections: chunks.len(),
            previews,
        })
    }

    /// BM25 search over all indexed chunks. Returns up to `limit` hits, most
    /// relevant first. A query that contains no indexable terms (all
    /// punctuation/symbols) yields an empty result rather than an error.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, IndexError> {
        let Some(match_expr) = sanitize_fts_query(query) else {
            return Ok(Vec::new());
        };
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT source, chunk_no, title,
                    snippet(chunks, 1, '', '', ' … ', 14) AS snip,
                    bm25(chunks) AS score
             FROM chunks
             WHERE chunks MATCH ?1
             ORDER BY bm25(chunks)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![match_expr, limit as i64], |row| {
            Ok(SearchHit {
                source: row.get(0)?,
                chunk_no: row.get(1)?,
                title: row.get(2)?,
                snippet: row.get(3)?,
                score: row.get(4)?,
            })
        })?;
        let mut hits = Vec::new();
        for hit in rows {
            hits.push(hit?);
        }
        Ok(hits)
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
}
