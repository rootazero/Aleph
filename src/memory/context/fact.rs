//! `MemoryFact` aggregate root — the core fact entity for the memory system.

use serde::{Deserialize, Serialize};

use crate::domain::Entity;

use super::enums::{
    FactSource, FactSpecificity, MemoryCategory, MemoryLayer, NoteType, TemporalScope,
};
use super::paths::compute_parent_path;

/// Default serde helper for namespace field
pub(crate) fn default_namespace() -> String {
    "owner".to_string()
}

/// Default serde helper for `agent_id` field
pub(crate) fn default_agent_id() -> String {
    "default".to_string()
}

/// A compressed memory fact extracted from conversations by LLM
///
/// Facts are third-person statements about the user, such as:
/// - "The user is learning Rust programming language"
/// - "The user prefers using Vim for coding"
/// - "The user plans to travel to Tokyo next week"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    /// Unique identifier (UUID)
    pub id: String,
    /// Fact content (third-person statement)
    pub content: String,
    /// Type classification
    pub note_type: NoteType,
    /// Vector embedding (dimension varies by provider: 768, 1024, 1536)
    pub embedding: Option<Vec<f32>>,
    /// Source memory IDs for traceability
    pub source_memory_ids: Vec<String>,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
    /// Whether this fact is still valid (soft delete)
    pub is_valid: bool,
    /// Reason for invalidation (if `is_valid` = false)
    pub invalidation_reason: Option<String>,
    /// Timestamp when fact was invalidated due to decay (Unix seconds)
    /// Used for recycle bin retention period
    pub decay_invalidated_at: Option<i64>,
    /// Fact specificity level
    pub specificity: FactSpecificity,
    /// Temporal scope
    pub temporal_scope: TemporalScope,
    /// Access control scope: "owner", "guest:xxx", "shared"
    #[serde(default = "default_namespace")]
    pub namespace: String,
    /// Domain isolation agent ID
    #[serde(default = "default_agent_id")]
    pub agent: String,
    /// Similarity score (when retrieved from search)
    #[serde(skip)]
    pub similarity_score: Option<f32>,
    /// VFS path for hierarchical organization (e.g., "<aleph://user/preferences/coding>")
    pub path: String,
    /// Tiered loading level for retrieval.
    pub layer: MemoryLayer,
    /// Standardized memory category.
    pub category: MemoryCategory,
    /// Fact origin/type
    pub fact_source: FactSource,
    /// Content hash for L1 staleness detection
    pub content_hash: String,
    /// Parent path for ls operations
    pub parent_path: String,
    /// Name of the embedding model that generated this fact's vector
    pub embedding_model: String,
    /// Optional persona identifier when scope == Persona
    #[serde(default)]
    pub persona_id: Option<String>,
    /// Number of times this fact has been accessed / retrieved
    #[serde(default)]
    pub access_count: u32,
    /// Timestamp of last retrieval (Unix seconds)
    #[serde(default)]
    pub last_accessed_at: Option<i64>,
    /// When this fact became true (Unix seconds). None = since creation.
    #[serde(default)]
    pub valid_from: Option<i64>,
    /// When this fact stopped being true (Unix seconds). None = still valid.
    #[serde(default)]
    pub valid_to: Option<i64>,
}

impl Entity for MemoryFact {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl MemoryFact {
    /// Create a new valid memory fact
    #[must_use]
    pub fn new(content: String, note_type: NoteType, source_ids: Vec<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let path = note_type.default_path().to_string();
        let parent_path = compute_parent_path(&path);
        let category = note_type.default_category();

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            note_type,
            embedding: None,
            source_memory_ids: source_ids,
            created_at: now,
            updated_at: now,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            namespace: "owner".to_string(),
            agent: "main".to_string(),
            similarity_score: None,
            path,
            layer: MemoryLayer::L2Detail,
            category,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path,
            embedding_model: String::new(),
            persona_id: None,
            access_count: 0,
            last_accessed_at: None,
            valid_from: None,
            valid_to: None,
        }
    }

    /// Create a new fact with specific ID (for database reconstruction)
    #[must_use]
    pub fn with_id(id: String, content: String, note_type: NoteType) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let path = note_type.default_path().to_string();
        let parent_path = compute_parent_path(&path);
        let category = note_type.default_category();

        Self {
            id,
            content,
            note_type,
            embedding: None,
            source_memory_ids: Vec::new(),
            created_at: now,
            updated_at: now,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            namespace: "owner".to_string(),
            agent: "main".to_string(),
            similarity_score: None,
            path,
            layer: MemoryLayer::L2Detail,
            category,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path,
            embedding_model: String::new(),
            persona_id: None,
            access_count: 0,
            last_accessed_at: None,
            valid_from: None,
            valid_to: None,
        }
    }

    /// Add embedding to the fact
    #[must_use]
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set similarity score (used during retrieval)
    #[must_use]
    pub const fn with_score(mut self, score: f32) -> Self {
        self.similarity_score = Some(score);
        self
    }

    /// Set specificity level
    #[must_use]
    pub const fn with_specificity(mut self, specificity: FactSpecificity) -> Self {
        self.specificity = specificity;
        self
    }

    /// Set temporal scope
    #[must_use]
    pub const fn with_temporal_scope(mut self, scope: TemporalScope) -> Self {
        self.temporal_scope = scope;
        self
    }

    /// Set VFS path
    #[must_use]
    pub fn with_path(mut self, path: String) -> Self {
        self.parent_path = compute_parent_path(&path);
        self.path = path;
        self
    }

    /// Set fact source
    #[must_use]
    pub const fn with_fact_source(mut self, source: FactSource) -> Self {
        self.fact_source = source;
        self
    }

    /// Set memory layer
    #[must_use]
    pub const fn with_layer(mut self, layer: MemoryLayer) -> Self {
        self.layer = layer;
        self
    }

    /// Set memory category
    #[must_use]
    pub const fn with_category(mut self, category: MemoryCategory) -> Self {
        self.category = category;
        self
    }

    /// Set agent ID for domain isolation
    #[must_use]
    pub fn with_agent(mut self, agent: String) -> Self {
        self.agent = agent;
        self
    }

    /// Set persona identifier (implies Persona scope)
    #[must_use]
    pub fn with_persona_id(mut self, persona_id: String) -> Self {
        self.persona_id = Some(persona_id);
        self
    }

    /// Set access count (builder pattern, useful for tests)
    #[must_use]
    pub const fn with_access_count(mut self, count: u32) -> Self {
        self.access_count = count;
        self
    }

    /// Set `created_at` timestamp (builder pattern, useful for tests)
    #[must_use]
    pub const fn with_created_at(mut self, ts: i64) -> Self {
        self.created_at = ts;
        self
    }

    /// Invalidate this fact with a reason
    #[must_use]
    pub fn invalidate(mut self, reason: &str) -> Self {
        self.is_valid = false;
        self.invalidation_reason = Some(reason.to_string());
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        self
    }

    /// Set the timestamp when this fact became true
    #[must_use]
    pub const fn with_valid_from(mut self, ts: i64) -> Self {
        self.valid_from = Some(ts);
        self
    }

    /// Set the timestamp when this fact stopped being true
    #[must_use]
    pub const fn with_valid_to(mut self, ts: i64) -> Self {
        self.valid_to = Some(ts);
        self
    }

    /// Close the validity window at the current time
    #[must_use]
    pub fn close_validity(mut self) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        self.valid_to = Some(now);
        self
    }

    /// Returns true if this fact has no end to its validity window
    #[must_use]
    pub const fn is_currently_valid(&self) -> bool {
        self.valid_to.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::NoteType;

    #[test]
    fn new_fact_has_no_validity_bounds() {
        let fact = MemoryFact::new("test".into(), NoteType::Other, vec![]);
        assert!(fact.valid_from.is_none());
        assert!(fact.valid_to.is_none());
        assert!(fact.is_currently_valid());
    }

    #[test]
    fn close_validity_sets_valid_to() {
        let fact = MemoryFact::new("test".into(), NoteType::Other, vec![]).close_validity();
        assert!(fact.valid_to.is_some());
        assert!(!fact.is_currently_valid());
    }

    #[test]
    fn with_valid_from_sets_timestamp() {
        let fact = MemoryFact::new("test".into(), NoteType::Other, vec![]).with_valid_from(1000);
        assert_eq!(fact.valid_from, Some(1000));
        assert!(fact.is_currently_valid());
    }

    #[test]
    fn with_valid_to_sets_timestamp() {
        let fact = MemoryFact::new("test".into(), NoteType::Other, vec![]).with_valid_to(2000);
        assert_eq!(fact.valid_to, Some(2000));
        assert!(!fact.is_currently_valid());
    }
}
