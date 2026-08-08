//! Common types used by the storage traits.
//!
//! Provides the scoring type shared across `MemoryStore` and `SessionStore`
//! implementations.
//!
//! # Retired: `SearchFilter` / `MemoryFilter`
//!
//! Both were LanceDB-era leftovers. Their only real method — `to_lance_filter`
//! — built a `DataFusion` SQL string by interpolating escaped literals, and had
//! zero production callers once the store moved to SQLite + sqlite-vec (see the
//! tech-stack ban on a second vector backend). `escape_sql_string` went with
//! them: manual quote-doubling exists only because that string-built dialect had
//! no bind parameters. SQLite paths bind parameters instead.

use crate::memory::context::MemoryFact;

// ---------------------------------------------------------------------------
// ScoredFact — a fact with its relevance score
// ---------------------------------------------------------------------------

/// A memory fact paired with its relevance score from a search operation.
///
/// The `score` is typically a cosine-similarity or reranker score in `[0, 1]`.
#[derive(Debug, Clone)]
pub struct ScoredFact {
    /// The memory fact.
    pub fact: MemoryFact,
    /// Relevance score (higher is more relevant).
    pub score: f32,
}
