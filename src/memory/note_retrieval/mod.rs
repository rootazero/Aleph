//! Note-based retrieval engine.
//!
//! Drop-in replacement for `FactRetrieval` that queries notes (markdown + `SQLite` index)
//! instead of the legacy facts table. Returns `Vec<ScoredFact>` so downstream
//! consumers don't require changes.

pub mod expansion;
mod relation_surface;
pub mod scoring;
pub mod trace;

// The engine's own methods, split by the stage of retrieval they belong to.
// `NoteFactRetrieval`'s inherent impl is free to live in several files; the
// only cost is that a method one stage calls on another is `pub(super)`
// instead of private, which is the same visibility it had when they shared
// a module.
mod builder;
mod multi_agent;
mod pipeline;
mod signals;
mod single_agent;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use self::trace::{StageTrace, TraceSink};
use crate::config::types::memory::{ExpansionConfig, RetrievalScoringConfig};
use crate::error::AlephError;
use crate::memory::notes::search_result::NoteSearchResult;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::NoteIndexer;
use crate::memory::rerank::{blend_scores, build_provider, RerankConfig, RerankProvider};
use crate::memory::store::types::ScoredFact;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use std::time::Instant;

/// When a cross-encoder reranker is active, over-fetch candidates by this factor
/// so the reranker has a meaningful pool to reorder before truncation.
const RERANK_CANDIDATE_MULTIPLIER: usize = 3;

/// Hard ceiling on the candidate pool sent to the cross-encoder, to bound the
/// remote rerank request cost regardless of the caller's `limit`.
const RERANK_MAX_CANDIDATES: usize = 50;

/// Channel label stamped on recall signals emitted automatically by the primary
/// retrieval path. Kept distinct from explicit `memory_reflect` synthesis signals
/// so the two dedup independently in `recall_signals`
/// (`UNIQUE(note_path, query_hash, day_bucket, channel)`).
const AUTO_RECALL_CHANNEL: &str = "auto-recall";

/// Notes-based retrieval engine. Drop-in replacement for `FactRetrieval`.
pub struct NoteFactRetrieval<S: NoteStore + Send + Sync + 'static> {
    indexer: Arc<NoteIndexer<S>>,
    /// `None` = FTS-only deployment (no embedding provider configured):
    /// hybrid/vector legs are skipped and every retrieval degrades to
    /// keyword (FTS) search instead of failing.
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Optional cross-encoder reranker applied as a final retrieval stage.
    /// `None` (the default) reproduces the legacy behaviour byte-for-byte.
    reranker: Option<Arc<dyn RerankProvider>>,
    /// Blend weight for the reranker score in `[0.0, 1.0]` (only used when
    /// `reranker` is `Some`).
    rerank_weight: f32,
    /// Retrieval-time recency decay + MMR diversity. Default-inactive, so the
    /// base `new()` reproduces legacy ranking byte-for-byte.
    scoring: RetrievalScoringConfig,
    /// Associative graph expansion of the candidate pool before rerank.
    /// Default-on; a cold graph cache makes it a no-op.
    expansion: ExpansionConfig,
}

/// Current Unix time in seconds (for retrieval-time recency scoring).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
