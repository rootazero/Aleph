//! Common types used by the storage traits.
//!
//! The `MemoryStore` / `SessionStore` filter types that once lived here were
//! removed with those traits — `SearchFilter`, `MemoryFilter` and their
//! `to_lance_filter` builders had zero non-test consumers.

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
