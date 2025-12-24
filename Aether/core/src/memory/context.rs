/// Context capture data structures for memory anchors
use serde::{Deserialize, Serialize};

/// Context anchor that identifies when and where an interaction occurred
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAnchor {
    /// Application bundle ID (e.g., "com.apple.Notes")
    pub app_bundle_id: String,
    /// Window title (e.g., "Project Plan.txt")
    pub window_title: String,
    /// Unix timestamp when interaction occurred
    pub timestamp: i64,
}

impl ContextAnchor {
    /// Create a new context anchor with current timestamp
    pub fn now(app_bundle_id: String, window_title: String) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            app_bundle_id,
            window_title,
            timestamp,
        }
    }

    /// Create context anchor with specific timestamp
    pub fn with_timestamp(app_bundle_id: String, window_title: String, timestamp: i64) -> Self {
        Self {
            app_bundle_id,
            window_title,
            timestamp,
        }
    }
}

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
    /// Vector embedding (384-dim for all-MiniLM-L6-v2)
    pub embedding: Option<Vec<f32>>,
    /// Similarity score (when retrieved from search)
    pub similarity_score: Option<f32>,
}

impl MemoryEntry {
    /// Create new memory entry without embedding
    pub fn new(
        id: String,
        context: ContextAnchor,
        user_input: String,
        ai_output: String,
    ) -> Self {
        Self {
            id,
            context,
            user_input,
            ai_output,
            embedding: None,
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
            similarity_score: None,
        }
    }

    /// Set similarity score (used during retrieval)
    pub fn with_score(mut self, score: f32) -> Self {
        self.similarity_score = Some(score);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_anchor_now() {
        let anchor = ContextAnchor::now(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
        );
        assert_eq!(anchor.app_bundle_id, "com.apple.Notes");
        assert_eq!(anchor.window_title, "Test.txt");
        assert!(anchor.timestamp > 0);
    }

    #[test]
    fn test_context_anchor_with_timestamp() {
        let anchor = ContextAnchor::with_timestamp(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
            1234567890,
        );
        assert_eq!(anchor.timestamp, 1234567890);
    }

    #[test]
    fn test_memory_entry_new() {
        let context = ContextAnchor::now("app".to_string(), "window".to_string());
        let entry = MemoryEntry::new(
            "id-123".to_string(),
            context.clone(),
            "user input".to_string(),
            "ai output".to_string(),
        );
        assert_eq!(entry.id, "id-123");
        assert_eq!(entry.context, context);
        assert!(entry.embedding.is_none());
        assert!(entry.similarity_score.is_none());
    }

    #[test]
    fn test_memory_entry_with_embedding() {
        let context = ContextAnchor::now("app".to_string(), "window".to_string());
        let embedding = vec![0.1, 0.2, 0.3];
        let entry = MemoryEntry::with_embedding(
            "id-123".to_string(),
            context,
            "input".to_string(),
            "output".to_string(),
            embedding.clone(),
        );
        assert_eq!(entry.embedding, Some(embedding));
    }

    #[test]
    fn test_memory_entry_with_score() {
        let context = ContextAnchor::now("app".to_string(), "window".to_string());
        let entry = MemoryEntry::new("id".to_string(), context, "in".to_string(), "out".to_string())
            .with_score(0.85);
        assert_eq!(entry.similarity_score, Some(0.85));
    }

    #[test]
    fn test_context_anchor_serialization() {
        let anchor = ContextAnchor::with_timestamp(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
            1234567890,
        );
        let json = serde_json::to_string(&anchor).unwrap();
        let deserialized: ContextAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(anchor, deserialized);
    }
}
