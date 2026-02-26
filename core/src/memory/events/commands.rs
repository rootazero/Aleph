//! Command structs for memory mutations.
//!
//! Each command maps to one or more MemoryEvents.
//! Commands are the input to [`super::handler::MemoryCommandHandler`].

use crate::memory::context::{FactSource, FactType, MemoryScope, MemoryTier};
use crate::memory::events::EventActor;

/// Create a new memory fact.
pub struct CreateFactCommand {
    pub content: String,
    pub fact_type: FactType,
    pub tier: MemoryTier,
    pub scope: MemoryScope,
    pub path: String,
    pub namespace: String,
    pub workspace: String,
    pub confidence: f32,
    pub source: FactSource,
    pub source_memory_ids: Vec<String>,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Update the textual content of an existing fact.
pub struct UpdateContentCommand {
    pub fact_id: String,
    pub new_content: String,
    pub reason: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Soft-delete (invalidate) a fact.
pub struct InvalidateFactCommand {
    pub fact_id: String,
    pub reason: String,
    pub actor: EventActor,
    pub strength_at_invalidation: Option<f32>,
    pub correlation_id: Option<String>,
}

/// Restore a previously invalidated fact.
pub struct RestoreFactCommand {
    pub fact_id: String,
    pub new_strength: f32,
    pub correlation_id: Option<String>,
}

/// Record a fact access/retrieval (Pulse event).
pub struct RecordAccessCommand {
    pub fact_id: String,
    pub query: Option<String>,
    pub relevance_score: Option<f32>,
    pub used_in_response: bool,
    pub correlation_id: Option<String>,
}

/// Apply strength decay to multiple facts (Pulse event, bulk).
pub struct ApplyDecayCommand {
    /// (fact_id, old_strength, new_strength) tuples
    pub fact_ids_with_strength: Vec<(String, f32, f32)>,
    pub decay_factor: f32,
    pub correlation_id: Option<String>,
}

/// Consolidate multiple facts into one.
pub struct ConsolidateCommand {
    pub source_fact_ids: Vec<String>,
    pub consolidated_content: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Permanently delete a fact.
pub struct DeleteFactCommand {
    pub fact_id: String,
    pub reason: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}
