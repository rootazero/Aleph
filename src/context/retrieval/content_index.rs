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
    /// with Reciprocal Rank Fusion, applies a proximity rerank for multi-term
    /// queries, and returns up to `limit` hits, most relevant first.
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

        let mut fused = rrf_fuse(porter, trigram);
        // Proximity rerank is meaningful only when there are ≥2 distinct terms
        // to measure the span between; a single term leaves RRF order intact.
        let terms = query_terms(query);
        if terms.len() >= 2 {
            proximity_rerank(&mut fused, &terms);
        }
        Ok(finalize(fused, limit))
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

    /// Drop every indexed chunk from both FTS tables, leaving the schema (and
    /// the live SQLite connection) intact so the index stays usable afterwards.
    ///
    /// Used by the sandbox reference-bypass defense: when a session trips the
    /// denial circuit-breaker, the offloaded-output index is wiped so the agent
    /// cannot mine previously-cached results via `ctx_search`. Mirrors the
    /// per-source `DELETE FROM chunks` already used by [`Self::index_text`].
    pub fn clear(&self) -> Result<(), IndexError> {
        let conn = self.lock();
        conn.execute_batch("DELETE FROM chunks; DELETE FROM chunks_tri;")?;
        Ok(())
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
/// fields so the fused [`SearchHit`] is built without a second SQLite round-trip,
/// plus the full chunk `body` used by the proximity reranker (never surfaced to
/// callers — it is dropped when the fused list is finalized into `SearchHit`s).
struct RankedRow {
    source: String,
    chunk_no: i64,
    title: String,
    snippet: String,
    body: String,
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
                snippet({table}, 1, '', '', ' … ', 14) AS snip,
                body
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
            match acc.get_mut(&key) {
                Some(entry) => entry.0 += contrib,
                None => {
                    order.push(key.clone());
                    acc.insert(key, (contrib, row));
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
/// Caller guarantees `terms.len() >= 2`. Pure; no SQLite, unit-testable.
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
        assert_eq!(proximity_relevance("totally unrelated text here", &terms), 0.0);
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
        idx.index_text("call_1", "bash", &sample_log()).unwrap();
        let a = idx.search("payment", 3).unwrap();
        let b = idx.search("payment", 3).unwrap();
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
            text.push_str(&format!("database mention {i} with assorted filler words\n"));
        }
        // Chunk 1: the three query terms together on one line.
        for _ in 0..20 {
            text.push_str("the database connection timeout occurred here\n");
        }
        idx.index_text("call_1", "bash", &text).unwrap();
        let hits = idx.search("database connection timeout", 5).unwrap();
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
        assert_eq!(terms, vec!["database".to_string(), "connection".to_string()]);
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
}
