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
    ///
    /// Caps the accumulated `content` at 16 MiB to prevent a misbehaving
    /// tool or runaway stream from growing the buffer unboundedly (DoS /
    /// OOM). The cap mirrors the upper bound on a single part's persisted
    /// payload elsewhere in the event store; oversize deltas after the cap
    /// are silently dropped from `content` but still reported via `delta`
    /// for the live UI to render the most recent chunk.
    pub fn append(&mut self, delta: &str) {
        const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;
        if self.content.len() < MAX_CONTENT_BYTES {
            let remaining = MAX_CONTENT_BYTES - self.content.len();
            let chunk = if delta.len() > remaining {
                let mut end = remaining;
                while end > 0 && !delta.is_char_boundary(end) {
                    end -= 1;
                }
                &delta[..end]
            } else {
                delta
            };
            self.content.push_str(chunk);
        }
        self.delta = Some(delta.to_string());
    }

    /// Mark as complete
    pub fn complete(&mut self) {
        self.is_complete = true;
        self.delta = None;
    }
}
