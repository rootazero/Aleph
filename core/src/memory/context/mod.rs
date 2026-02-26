/// Context capture data structures for memory anchors
mod compression;
mod enums;
mod fact;
mod paths;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

// Re-export all public items so external code can use `crate::memory::context::*`
pub use compression::{CompressionResult, CompressionSession, FactStats};
pub use enums::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryLayer, MemoryScope, MemoryTier,
    TemporalScope,
};
pub use fact::MemoryFact;
pub use paths::{compute_parent_path, PRESET_PATHS};

// ============================================================================
// ContextAnchor
// ============================================================================

/// Context anchor that identifies when and where an interaction occurred
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAnchor {
    /// Application bundle ID (e.g., "com.apple.Notes")
    pub app_bundle_id: String,
    /// Window title (e.g., "Project Plan.txt")
    pub window_title: String,
    /// Unix timestamp when interaction occurred
    pub timestamp: i64,
    /// Topic ID for associating memories with conversation topics
    /// For multi-turn: specific topic UUID; For single-turn: "single-turn" constant
    pub topic_id: String,
}

/// Default topic ID for single-turn interactions
pub const SINGLE_TURN_TOPIC_ID: &str = "single-turn";

impl ContextAnchor {
    /// Create a new context anchor with current timestamp (for single-turn)
    pub fn now(app_bundle_id: String, window_title: String) -> Self {
        Self::with_topic(
            app_bundle_id,
            window_title,
            SINGLE_TURN_TOPIC_ID.to_string(),
        )
    }

    /// Create context anchor with specific timestamp (for single-turn)
    pub fn with_timestamp(app_bundle_id: String, window_title: String, timestamp: i64) -> Self {
        Self {
            app_bundle_id,
            window_title,
            timestamp,
            topic_id: SINGLE_TURN_TOPIC_ID.to_string(),
        }
    }

    /// Create context anchor with topic ID (for multi-turn conversations)
    pub fn with_topic(app_bundle_id: String, window_title: String, topic_id: String) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            app_bundle_id,
            window_title,
            timestamp,
            topic_id,
        }
    }
}

// ============================================================================
// MemoryEntry
// ============================================================================

/// Memory entry representing a stored interaction
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// Unique identifier (UUID)
    pub id: String,
    /// Context anchor (app + window + time)
    pub context: ContextAnchor,
    /// Original user input
    pub user_input: String,
    /// AI response
    pub ai_output: String,
    /// Vector embedding (384-dim for multilingual-e5-small)
    pub embedding: Option<Vec<f32>>,
    /// Access control scope: "owner", "guest:xxx", "shared"
    pub namespace: String,
    /// Domain isolation workspace ID
    pub workspace: String,
    /// Similarity score (when retrieved from search)
    pub similarity_score: Option<f32>,
}

impl MemoryEntry {
    /// Create new memory entry without embedding
    pub fn new(id: String, context: ContextAnchor, user_input: String, ai_output: String) -> Self {
        Self {
            id,
            context,
            user_input,
            ai_output,
            embedding: None,
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: None,
        }
    }

    /// Create memory entry with embedding
    pub fn with_embedding(
        id: String,
        context: ContextAnchor,
        user_input: String,
        ai_output: String,
        embedding: Vec<f32>,
    ) -> Self {
        Self {
            id,
            context,
            user_input,
            ai_output,
            embedding: Some(embedding),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: None,
        }
    }

    /// Set similarity score (used during retrieval)
    pub fn with_score(mut self, score: f32) -> Self {
        self.similarity_score = Some(score);
        self
    }
}
