//! Storage abstraction layer for the memory system.
//!
//! Provides the core storage trait definitions (`MemoryStore`)
//! and supporting types used by SQLite-backed storage implementations.
//!
//! ## Trait Overview
//!
//! - **`MemoryStore`** -- CRUD, vector/text/hybrid search, VFS path operations
//!   for `MemoryFact` (Layer 2 compressed facts).

pub mod sqlite;
pub mod types;

pub use sqlite::SqliteMemoryBackend;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::context::{CompressionSession, FactStats, NoteType, MemoryFact};
use crate::memory::dreaming::{DailyInsight, DreamStatus};
use crate::memory::namespace::NamespaceScope;

use types::{ScoredFact, SearchFilter};

/// Parameters for hybrid (vector + text) search.
pub struct HybridSearchParams<'a> {
    /// Vector embedding for ANN search.
    pub embedding: &'a [f32],
    /// Dimensionality hint for selecting the correct vector column.
    pub dim_hint: u32,
    /// Text query for full-text search.
    pub query_text: &'a str,
    /// Weight applied to vector search scores.
    pub vector_weight: f32,
    /// Weight applied to text search scores.
    pub text_weight: f32,
    /// Additional filter predicates.
    pub filter: &'a SearchFilter,
    /// Maximum number of results to return.
    pub limit: usize,
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Statistics about the memory store.
#[derive(Debug, Clone, Default)]
pub struct StoreStats {
    /// Total number of facts (including invalidated).
    pub total_facts: usize,
    /// Number of currently valid facts.
    pub valid_facts: usize,
}

/// A VFS path entry returned by directory listing operations.
#[derive(Debug, Clone)]
pub struct PathEntry {
    /// Full `aleph://` path.
    pub path: String,
    /// `true` if this entry is a leaf (fact), `false` if it is a directory.
    pub is_leaf: bool,
    /// Number of direct children (facts or sub-directories).
    pub child_count: usize,
}

// ---------------------------------------------------------------------------
// MemoryStore -- Layer 2 (compressed facts) storage trait
// ---------------------------------------------------------------------------

/// Abstraction over fact storage for the memory system.
///
/// Implementors provide CRUD operations, multi-modal search (vector, text,
/// hybrid), VFS path queries, and bulk operations on `MemoryFact` records.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    // -- CRUD ---------------------------------------------------------------

    /// Insert a new fact into the store.
    async fn insert_fact(&self, fact: &MemoryFact) -> Result<(), AlephError>;

    /// Retrieve a fact by its unique ID, or `None` if not found.
    async fn get_fact(&self, id: &str) -> Result<Option<MemoryFact>, AlephError>;

    /// Update an existing fact (full replace by ID).
    async fn update_fact(&self, fact: &MemoryFact) -> Result<(), AlephError>;

    /// Hard-delete a fact by ID.
    async fn delete_fact(&self, id: &str) -> Result<(), AlephError>;

    /// Batch-insert multiple facts in a single operation.
    async fn batch_insert_facts(&self, facts: &[MemoryFact]) -> Result<(), AlephError>;

    // -- Search -------------------------------------------------------------

    /// Pure vector (ANN) search over fact embeddings.
    async fn vector_search(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    /// Full-text (BM25-style) search over fact content.
    async fn text_search(
        &self,
        query: &str,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    /// Hybrid search combining vector similarity and text relevance.
    async fn hybrid_search(
        &self,
        params: &HybridSearchParams<'_>,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    // -- VFS path operations ------------------------------------------------

    /// List child entries under the given VFS parent path.
    async fn list_by_path(
        &self,
        parent_path: &str,
        ns: &NamespaceScope,
        workspace: &str,
    ) -> Result<Vec<PathEntry>, AlephError>;

    /// Get a single fact by its exact VFS path within a namespace.
    async fn get_by_path(
        &self,
        path: &str,
        ns: &NamespaceScope,
        workspace: &str,
    ) -> Result<Option<MemoryFact>, AlephError>;

    /// Retrieve facts by VFS path prefix with additional filters.
    async fn get_facts_by_path_prefix(
        &self,
        path_prefix: &str,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError>;

    // -- Statistics & bulk --------------------------------------------------

    /// Count facts matching the given filter.
    async fn count_facts(&self, filter: &SearchFilter) -> Result<usize, AlephError>;

    /// Retrieve facts of a specific type within a namespace.
    async fn get_facts_by_type(
        &self,
        note_type: NoteType,
        ns: &NamespaceScope,
        workspace: &str,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError>;

    /// Retrieve all facts, optionally including invalidated ones.
    ///
    /// When `workspace` is `Some`, only facts in that workspace are returned.
    async fn get_all_facts(
        &self,
        include_invalid: bool,
        workspace: Option<&str>,
    ) -> Result<Vec<MemoryFact>, AlephError>;

    /// Load the embedding vector for a fact from the vector store.
    ///
    /// Returns the fact with its `embedding` field populated, or unchanged
    /// if no embedding is stored. Default implementation is a no-op.
    async fn load_embedding_for_fact(
        &self,
        fact: &mut MemoryFact,
    ) -> Result<(), AlephError> {
        let _ = fact;
        Ok(())
    }

    // -- Mutation helpers ---------------------------------------------------

    /// Soft-delete a fact by marking it invalid with a reason.
    async fn invalidate_fact(&self, id: &str, reason: &str) -> Result<(), AlephError>;

    /// Close a fact's validity window by setting `valid_to` to the given timestamp.
    /// The fact remains valid (`is_valid = true`) but becomes historical.
    async fn close_fact_validity(&self, id: &str, valid_to: i64) -> Result<(), AlephError>;

    /// Set `valid_from` on a fact.
    async fn set_fact_valid_from(&self, id: &str, valid_from: i64) -> Result<(), AlephError>;

    /// Update only the textual content of a fact (preserving other fields).
    async fn update_fact_content(&self, id: &str, new_content: &str) -> Result<(), AlephError>;

    /// Find facts whose embeddings are within `threshold` similarity.
    async fn find_similar_facts(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        filter: &SearchFilter,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    /// Apply decay to all valid facts by multiplying their decay score.
    ///
    /// Facts whose score falls below `min_score` are invalidated.
    /// Returns the number of facts that were updated or invalidated.
    async fn apply_fact_decay(
        &self,
        decay_factor: f32,
        min_score: f32,
    ) -> Result<usize, AlephError>;

    /// Get aggregate statistics about stored facts.
    async fn get_fact_stats(&self) -> Result<FactStats, AlephError>;

    /// Soft-delete a fact with a given reason (alias for `invalidate_fact`).
    ///
    /// Used by the lazy decay engine.
    async fn soft_delete_fact(&self, id: &str, reason: &str) -> Result<(), AlephError>;

    /// Count facts in a given topic that belong to a domain other than `exclude_domain`.
    async fn count_facts_by_topic_excluding_domain(
        &self,
        topic: &str,
        exclude_domain: &str,
    ) -> Result<u64, AlephError>;

    /// Mark a fact as a tunnel candidate.
    ///
    /// TODO(palace-evolution): Wire this into the fact write path (compression service
    /// or session compactor) so that newly inserted facts are checked for cross-domain
    /// tunnel candidacy. Currently the TunnelDiscoveryStage is dormant because nothing
    /// sets tunnel_pending = true.
    async fn set_tunnel_pending(&self, id: &str, pending: bool) -> Result<(), AlephError>;

    /// Check if any tunnel candidates exist.
    async fn has_tunnel_pending(&self) -> Result<bool, AlephError>;

    /// Get tunnel candidate facts, up to `limit`.
    async fn get_tunnel_candidates(&self, limit: usize) -> Result<Vec<MemoryFact>, AlephError>;

    /// Clear tunnel_pending flag for all facts with the given topic.
    async fn clear_tunnel_pending_by_topic(&self, topic: &str) -> Result<(), AlephError>;
}

// ---------------------------------------------------------------------------
// DreamStore -- Dream daemon persistence trait
// ---------------------------------------------------------------------------

/// Abstraction over dream daemon state persistence.
///
/// Provides storage for dream run status and daily insight summaries
/// generated during idle-time memory consolidation.
#[async_trait]
pub trait DreamStore: Send + Sync {
    /// Get the current dream daemon status.
    async fn get_dream_status(&self) -> Result<DreamStatus, AlephError>;

    /// Update the dream daemon status.
    async fn set_dream_status(&self, status: DreamStatus) -> Result<(), AlephError>;

    /// Insert or update a daily insight for the given date.
    async fn upsert_daily_insight(&self, insight: DailyInsight) -> Result<(), AlephError>;

    /// Get the daily insight for a specific date (YYYY-MM-DD format).
    async fn get_daily_insight(&self, date: &str) -> Result<Option<DailyInsight>, AlephError>;
}

// ---------------------------------------------------------------------------
// CompressionStore -- Compression session persistence trait
// ---------------------------------------------------------------------------

/// Abstraction over compression session metadata storage.
///
/// Tracks when compression was last run and stores session records
/// for auditing the memory compression pipeline.
#[async_trait]
pub trait CompressionStore: Send + Sync {
    /// Set the timestamp of the last successful compression run.
    async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AlephError>;

    /// Get the timestamp of the last successful compression run.
    async fn get_last_compression_timestamp(&self) -> Result<Option<i64>, AlephError>;

    /// Record a completed compression session for auditing.
    async fn record_compression_session(
        &self,
        session: &CompressionSession,
    ) -> Result<(), AlephError>;
}

// ---------------------------------------------------------------------------
// MemoryBackend type alias
// ---------------------------------------------------------------------------

use crate::sync_primitives::Arc;

/// Unified memory backend — provides MemoryStore.
///
/// This is the single entry point for all memory storage operations.
/// Wraps `SqliteMemoryBackend` in an `Arc` for shared ownership across
/// the agent loop, thinker, and other subsystems.
pub type MemoryBackend = Arc<sqlite::SqliteMemoryBackend>;
