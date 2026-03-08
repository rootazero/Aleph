//! MemoryFact aggregate root — the core fact entity for the memory system.

use serde::{Deserialize, Serialize};

use crate::domain::{AggregateRoot, Entity};

use super::enums::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryLayer, MemoryScope, MemoryTier,
    TemporalScope,
};
use super::paths::compute_parent_path;

/// Default serde helper for namespace field
pub(crate) fn default_namespace() -> String {
    "owner".to_string()
}

/// Default serde helper for workspace_id field
pub(crate) fn default_workspace_id() -> String {
    "default".to_string()
}

/// Default serde helper for strength field
fn default_strength() -> f32 {
    1.0
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
    pub fact_type: FactType,
    /// Vector embedding (dimension varies by provider: 768, 1024, 1536)
    pub embedding: Option<Vec<f32>>,
    /// Source memory IDs for traceability
    pub source_memory_ids: Vec<String>,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
    /// Confidence score (0.0-1.0) from LLM
    pub confidence: f32,
    /// Whether this fact is still valid (soft delete)
    pub is_valid: bool,
    /// Reason for invalidation (if is_valid = false)
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
    /// Domain isolation workspace ID
    #[serde(default = "default_workspace_id")]
    pub workspace: String,
    /// Similarity score (when retrieved from search)
    #[serde(skip)]
    pub similarity_score: Option<f32>,
    /// VFS path for hierarchical organization (e.g., "aleph://user/preferences/coding")
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
    /// Cognitive memory tier (Core / ShortTerm / LongTerm)
    #[serde(default)]
    pub tier: MemoryTier,
    /// Visibility scope (Global / Workspace / Persona)
    #[serde(default)]
    pub scope: MemoryScope,
    /// Optional persona identifier when scope == Persona
    #[serde(default)]
    pub persona_id: Option<String>,
    /// Reinforcement strength (0.0 .. 1.0+), decayed over time
    #[serde(default = "default_strength")]
    pub strength: f32,
    /// Number of times this fact has been accessed / retrieved
    #[serde(default)]
    pub access_count: u32,
    /// Timestamp of last retrieval (Unix seconds)
    #[serde(default)]
    pub last_accessed_at: Option<i64>,
}

impl Entity for MemoryFact {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl AggregateRoot for MemoryFact {}

impl MemoryFact {
    /// Create a new valid memory fact
    pub fn new(content: String, fact_type: FactType, source_ids: Vec<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let path = fact_type.default_path().to_string();
        let parent_path = compute_parent_path(&path);
        let category = fact_type.default_category();

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            fact_type,
            embedding: None,
            source_memory_ids: source_ids,
            created_at: now,
            updated_at: now,
            confidence: 1.0,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            namespace: "owner".to_string(),
            workspace: "main".to_string(),
            similarity_score: None,
            path,
            layer: MemoryLayer::L2Detail,
            category,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path,
            embedding_model: String::new(),
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            persona_id: None,
            strength: 1.0,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    /// Create a new fact with specific ID (for database reconstruction)
    pub fn with_id(id: String, content: String, fact_type: FactType) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let path = fact_type.default_path().to_string();
        let parent_path = compute_parent_path(&path);
        let category = fact_type.default_category();

        Self {
            id,
            content,
            fact_type,
            embedding: None,
            source_memory_ids: Vec::new(),
            created_at: now,
            updated_at: now,
            confidence: 1.0,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            namespace: "owner".to_string(),
            workspace: "main".to_string(),
            similarity_score: None,
            path,
            layer: MemoryLayer::L2Detail,
            category,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path,
            embedding_model: String::new(),
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            persona_id: None,
            strength: 1.0,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    /// Add embedding to the fact
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set confidence score
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Set similarity score (used during retrieval)
    pub fn with_score(mut self, score: f32) -> Self {
        self.similarity_score = Some(score);
        self
    }

    /// Set specificity level
    pub fn with_specificity(mut self, specificity: FactSpecificity) -> Self {
        self.specificity = specificity;
        self
    }

    /// Set temporal scope
    pub fn with_temporal_scope(mut self, scope: TemporalScope) -> Self {
        self.temporal_scope = scope;
        self
    }

    /// Set VFS path
    pub fn with_path(mut self, path: String) -> Self {
        self.parent_path = compute_parent_path(&path);
        self.path = path;
        self
    }

    /// Set fact source
    pub fn with_fact_source(mut self, source: FactSource) -> Self {
        self.fact_source = source;
        self
    }

    /// Set memory layer
    pub fn with_layer(mut self, layer: MemoryLayer) -> Self {
        self.layer = layer;
        self
    }

    /// Set memory category
    pub fn with_category(mut self, category: MemoryCategory) -> Self {
        self.category = category;
        self
    }

    /// Set cognitive memory tier
    pub fn with_tier(mut self, tier: MemoryTier) -> Self {
        self.tier = tier;
        self
    }

    /// Set visibility scope
    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = scope;
        self
    }

    /// Set workspace ID for domain isolation
    pub fn with_workspace(mut self, workspace: String) -> Self {
        self.workspace = workspace;
        self
    }

    /// Set persona identifier (implies Persona scope)
    pub fn with_persona_id(mut self, persona_id: String) -> Self {
        self.persona_id = Some(persona_id);
        self
    }

    /// Invalidate this fact with a reason
    pub fn invalidate(mut self, reason: &str) -> Self {
        self.is_valid = false;
        self.invalidation_reason = Some(reason.to_string());
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        self
    }
}
