//! Streaming text type for incremental UI updates

use serde::{Deserialize, Serialize};

/// Streaming text part - supports incremental text updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingTextPart {
    /// Unique part identifier
    pub part_id: String,
    /// Current full content
    pub content: String,
    /// Whether streaming has completed
    pub is_complete: bool,
    /// Incremental content delta (for event push)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
}

impl StreamingTextPart {
    /// Create a new streaming text part
    pub fn new(part_id: impl Into<String>) -> Self {
        Self {
            part_id: part_id.into(),
            content: String::new(),
            is_complete: false,
            delta: None,
        }
    }

    /// Create with initial content
    pub fn with_content(part_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            part_id: part_id.into(),
            content: content.into(),
            is_complete: false,
            delta: None,
        }
    }

    /// Append delta to content
    pub fn append(&mut self, delta: &str) {
        self.content.push_str(delta);
        self.delta = Some(delta.to_string());
    }

    /// Mark as complete
    pub fn complete(&mut self) {
        self.is_complete = true;
        self.delta = None;
    }
}
