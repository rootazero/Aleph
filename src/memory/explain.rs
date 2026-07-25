//! Fact-lifecycle explanation types (the `memory_timeline` read model).
//!
//! Was `memory::audit`. That module also carried an `AuditEntry` / `AuditActor`
//! / `AuditAction` / `AuditDetails` / `ForgettingExplanation` family written to
//! back a `memory_audit_log` table — a table with an `actor` column and **no
//! `INSERT` anywhere in the tree**. Nothing constructed the types either, so
//! they were an audit vocabulary for an audit that never happened, which is
//! strictly worse than no audit: an operator who queries it concludes nothing
//! occurred. Both the table and the types are gone (the same call Aleph made
//! in 2026-07-14 when it deleted the approval-audit store for having only test
//! helpers as writers).
//!
//! What survives is what is actually read: the projection
//! [`crate::memory::events::TimeTraveler::explain_fact`] builds from
//! `memory_events` and `memory_timeline` renders. Agent accountability is a
//! different concern entirely and lives in [`crate::identity`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Explanation of a fact's lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FactExplanation {
    /// The fact ID
    pub fact_id: String,
    /// The fact content (if still available)
    pub content: Option<String>,
    /// Whether the fact is currently valid
    pub is_valid: bool,
    /// Source of creation (e.g., "session", "user")
    pub creation_source: Option<String>,
    /// Number of times accessed
    pub access_count: usize,
    /// Reason for invalidation (if invalidated)
    pub invalidation_reason: Option<String>,
    /// Timeline of events
    pub events: Vec<ExplainedEvent>,
}

/// A single explained event in a fact's lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainedEvent {
    /// Unix timestamp
    pub timestamp: i64,
    /// Action type
    pub action: String,
    /// Human-readable description
    pub description: String,
    /// Who performed the action
    pub actor: String,
}
