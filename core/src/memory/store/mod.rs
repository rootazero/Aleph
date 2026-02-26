//! Storage abstraction layer for the memory system.
//!
//! Provides the core storage trait definitions (`MemoryStore`, `GraphStore`,
//! `SessionStore`) and supporting types used by LanceDB-backed (and any future)
//! storage implementations.
//!
//! ## Trait Overview
//!
//! - **`MemoryStore`** -- CRUD, vector/text/hybrid search, VFS path operations
//!   for `MemoryFact` (Layer 2 compressed facts).
//! - **`GraphStore`** -- Knowledge graph node/edge management, entity resolution,
//!   and temporal decay.
//! - **`SessionStore`** -- Raw memory entry (Layer 1) storage and retrieval.

pub mod lance;
pub mod types;

pub use lance::LanceMemoryBackend;

use async_trait::async_trait;

use crate::config::types::memory::GraphDecayPolicy;
use crate::error::AlephError;
use crate::memory::context::{CompressionSession, FactStats, FactType, MemoryEntry, MemoryFact};
use crate::memory::namespace::NamespaceScope;
use crate::memory::audit::AuditEntry;
use crate::memory::dreaming::{DailyInsight, DreamStatus};

use types::{MemoryFilter, ScoredFact, SearchFilter};

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
    /// Total raw memory entries (Layer 1).
    pub total_memories: usize,
    /// Total knowledge-graph nodes.
    pub total_graph_nodes: usize,
    /// Total knowledge-graph edges.
    pub total_graph_edges: usize,
}

/// Result of a graph decay sweep.
#[derive(Debug, Clone, Default)]
pub struct DecayStats {
    /// Nodes whose score was reduced.
    pub nodes_decayed: usize,
    /// Nodes removed because their score fell below the threshold.
    pub nodes_pruned: usize,
    /// Edges whose score was reduced.
    pub edges_decayed: usize,
    /// Edges removed because their score fell below the threshold.
    pub edges_pruned: usize,
}

/// A resolved entity returned by graph entity resolution.
#[derive(Debug, Clone)]
pub struct ResolvedEntity {
    /// Node ID in the graph.
    pub node_id: String,
    /// Canonical name of the entity.
    pub name: String,
    /// Entity kind/type (e.g. "person", "project", "tool").
    pub kind: String,
    /// Alternative names / aliases for this entity.
    pub aliases: Vec<String>,
    /// Context-weighted relevance score.
    pub context_score: f32,
    /// Whether the resolution is ambiguous (multiple candidates).
    pub ambiguous: bool,
}

/// A knowledge-graph node.
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Unique node identifier.
    pub id: String,
    /// Canonical display name.
    pub name: String,
    /// Node kind/type (e.g. "person", "project", "concept").
    pub kind: String,
    /// Alternative names / aliases.
    pub aliases: Vec<String>,
    /// Arbitrary metadata serialized as JSON.
    pub metadata_json: String,
    /// Temporal decay score (starts at 1.0, decreases over time).
    pub decay_score: f32,
    /// Creation timestamp (Unix seconds).
    pub created_at: i64,
    /// Last update timestamp (Unix seconds).
    pub updated_at: i64,
    /// Domain isolation workspace ID.
    pub workspace: String,
}

/// A knowledge-graph edge.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// Unique edge identifier.
    pub id: String,
    /// Source node ID.
    pub from_id: String,
    /// Target node ID.
    pub to_id: String,
    /// Relation label (e.g. "uses", "knows", "works_on").
    pub relation: String,
    /// Edge weight (application-specific).
    pub weight: f32,
    /// Confidence score [0.0, 1.0].
    pub confidence: f32,
    /// Context key for scoping edges to a particular context.
    pub context_key: String,
    /// Temporal decay score (starts at 1.0, decreases over time).
    pub decay_score: f32,
    /// Creation timestamp (Unix seconds).
    pub created_at: i64,
    /// Last update timestamp (Unix seconds).
    pub updated_at: i64,
    /// Timestamp of most recent reference (Unix seconds).
    pub last_seen_at: i64,
    /// Domain isolation workspace ID.
    pub workspace: String,
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
        fact_type: FactType,
        ns: &NamespaceScope,
        workspace: &str,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError>;

    /// Retrieve all facts, optionally including invalidated ones.
    async fn get_all_facts(
        &self,
        include_invalid: bool,
    ) -> Result<Vec<MemoryFact>, AlephError>;

    // -- Mutation helpers ---------------------------------------------------

    /// Soft-delete a fact by marking it invalid with a reason.
    async fn invalidate_fact(&self, id: &str, reason: &str) -> Result<(), AlephError>;

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
}

// ---------------------------------------------------------------------------
// GraphStore -- Knowledge graph storage trait
// ---------------------------------------------------------------------------

/// Abstraction over knowledge-graph storage.
///
/// Provides node/edge CRUD, entity resolution, and temporal decay operations.
#[async_trait]
pub trait GraphStore: Send + Sync {
    /// Insert or update a graph node (upsert by ID).
    async fn upsert_node(&self, node: &GraphNode, workspace: &str) -> Result<(), AlephError>;

    /// Retrieve a node by ID, or `None` if not found.
    async fn get_node(&self, id: &str, workspace: &str) -> Result<Option<GraphNode>, AlephError>;

    /// Insert or update a graph edge (upsert by ID).
    async fn upsert_edge(&self, edge: &GraphEdge, workspace: &str) -> Result<(), AlephError>;

    /// Resolve an entity mention to candidate graph nodes.
    ///
    /// The optional `context_key` narrows results to edges in that context.
    async fn resolve_entity(
        &self,
        query: &str,
        context_key: Option<&str>,
        workspace: &str,
    ) -> Result<Vec<ResolvedEntity>, AlephError>;

    /// Get all edges connected to a node, optionally filtered by context key.
    async fn get_edges_for_node(
        &self,
        node_id: &str,
        context_key: Option<&str>,
        workspace: &str,
    ) -> Result<Vec<GraphEdge>, AlephError>;

    /// Count edges for a node within a specific context.
    async fn count_edges_in_context(
        &self,
        node_id: &str,
        context_key: &str,
        workspace: &str,
    ) -> Result<usize, AlephError>;

    /// Apply temporal decay to all nodes and edges, pruning those below
    /// the minimum score threshold.
    async fn apply_decay(&self, policy: &GraphDecayPolicy, workspace: &str) -> Result<DecayStats, AlephError>;
}

// ---------------------------------------------------------------------------
// SessionStore -- Layer 1 (raw memory) storage trait
// ---------------------------------------------------------------------------

/// Abstraction over raw memory entry storage (Layer 1).
///
/// Provides insert, search, and retrieval for conversation-level memory
/// entries that have not yet been compressed into facts.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Insert a new raw memory entry.
    async fn insert_memory(&self, memory: &MemoryEntry) -> Result<(), AlephError>;

    /// Vector-search over memory entry embeddings.
    async fn search_memories(
        &self,
        embedding: &[f32],
        filter: &MemoryFilter,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError>;

    /// Get memories associated with a specific entity ID.
    async fn get_memories_for_entity(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError>;

    /// Get the most recent memories matching a filter.
    async fn get_recent_memories(
        &self,
        filter: &MemoryFilter,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError>;

    /// Hard-delete a memory entry by ID.
    async fn delete_memory(&self, id: &str) -> Result<(), AlephError>;

    /// Get aggregate statistics across all storage layers.
    async fn get_stats(&self) -> Result<StoreStats, AlephError>;

    /// Get memories created at or after the given timestamp within a namespace.
    ///
    /// Used by the DreamDaemon for daily consolidation.
    async fn get_memories_since(
        &self,
        since_timestamp: i64,
        namespace: &NamespaceScope,
        workspace: &str,
    ) -> Result<Vec<MemoryEntry>, AlephError>;

    /// Delete memory entries older than the given cutoff timestamp.
    ///
    /// Returns the number of deleted entries. Used by cleanup services.
    async fn delete_older_than(
        &self,
        cutoff_timestamp: i64,
    ) -> Result<u64, AlephError>;

    /// Clear memories with optional app/window filters.
    ///
    /// When both filters are `None`, clears all memories.
    /// Returns the number of deleted entries.
    async fn clear_memories(
        &self,
        app_filter: Option<&str>,
        window_filter: Option<&str>,
    ) -> Result<u64, AlephError>;

    /// Get uncompressed memories since a timestamp, up to a limit.
    ///
    /// Used by the compression service to find raw memories that
    /// have not yet been distilled into facts.
    async fn get_uncompressed_memories(
        &self,
        since_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError>;
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
// AuditStore -- Fact audit log persistence trait
// ---------------------------------------------------------------------------

/// Abstraction over fact audit log storage.
///
/// Records all mutations (creation, update, invalidation, deletion) that
/// happen to facts, providing a complete audit trail.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Insert a new audit entry.
    async fn insert_audit_entry(&self, entry: &AuditEntry) -> Result<(), AlephError>;

    /// Get all audit entries for a specific fact.
    async fn get_audit_entries_for_fact(
        &self,
        fact_id: &str,
    ) -> Result<Vec<AuditEntry>, AlephError>;

    /// Get the most recent audit entries across all facts.
    async fn get_recent_audit_entries(
        &self,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, AlephError>;
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
// MemoryEventStore -- Event sourcing persistence trait
// ---------------------------------------------------------------------------

/// Append-only event log for memory domain events.
///
/// This is the **source of truth** for all fact mutations. Events are
/// stored in SQLite and projected to LanceDB for search.
#[async_trait]
pub trait MemoryEventStore: Send + Sync {
    // -- Write ---------------------------------------------------------------

    /// Append a single event. Returns the assigned global ID.
    async fn append_event(
        &self,
        envelope: &crate::memory::events::MemoryEventEnvelope,
    ) -> Result<i64, AlephError>;

    /// Batch-append events (for Pulse flush and migration).
    async fn append_events(
        &self,
        envelopes: &[crate::memory::events::MemoryEventEnvelope],
    ) -> Result<(), AlephError>;

    // -- Read by fact --------------------------------------------------------

    /// Load all events for a fact, ordered by seq.
    async fn get_events_for_fact(
        &self,
        fact_id: &str,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    /// Load events for a fact since a given sequence number.
    async fn get_events_since_seq(
        &self,
        fact_id: &str,
        since_seq: u64,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    // -- Time travel ---------------------------------------------------------

    /// Load all events for a fact up to a given timestamp.
    async fn get_events_until(
        &self,
        fact_id: &str,
        until_timestamp: i64,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    /// Load all events within a time range (across all facts).
    async fn get_events_in_range(
        &self,
        from_timestamp: i64,
        to_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    // -- Statistics ----------------------------------------------------------

    /// Get the latest sequence number for a fact (0 if no events).
    async fn get_latest_seq(&self, fact_id: &str) -> Result<u64, AlephError>;

    /// Count total events, optionally filtered by event type tag.
    async fn count_events(
        &self,
        event_type_filter: Option<&str>,
    ) -> Result<usize, AlephError>;
}

// ---------------------------------------------------------------------------
// MemoryBackend type alias
// ---------------------------------------------------------------------------

use std::sync::Arc;

/// Unified memory backend — provides MemoryStore + GraphStore + SessionStore.
///
/// This is the single entry point for all memory storage operations.
/// Wraps `LanceMemoryBackend` in an `Arc` for shared ownership across
/// the agent loop, thinker, and other subsystems.
pub type MemoryBackend = Arc<lance::LanceMemoryBackend>;
