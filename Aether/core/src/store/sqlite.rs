//! SQLite vector store implementation using rig-sqlite

use serde::{Deserialize, Serialize};

/// Memory entry stored in vector database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier
    pub id: String,
    /// User's input
    pub user_input: String,
    /// Assistant's response
    pub assistant_response: String,
    /// Unix timestamp
    pub timestamp: i64,
    /// Source application context
    pub app_context: Option<String>,
}

/// Memory store using rig-sqlite
pub struct MemoryStore {
    // Will be implemented in Phase 2
}

impl MemoryStore {
    /// Create placeholder (will be implemented in Phase 2)
    pub fn placeholder() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_entry_creation() {
        let entry = MemoryEntry {
            id: "test-1".to_string(),
            user_input: "Hello".to_string(),
            assistant_response: "Hi there!".to_string(),
            timestamp: 1234567890,
            app_context: None,
        };
        assert_eq!(entry.id, "test-1");
    }
}
