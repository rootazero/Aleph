//! Compaction marker types

use serde::{Deserialize, Serialize};

/// Marker for compaction boundary
///
/// This marker is inserted into the session when compaction occurs,
/// allowing filter_compacted() to find the boundary and discard
/// old context that has been summarized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMarker {
    /// When compaction occurred
    pub timestamp: i64,
    /// Whether this was automatic or user-triggered
    pub auto: bool,
    /// Unique marker identifier (optional for backward compatibility)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker_id: Option<String>,
    /// Number of parts that were compacted (optional for backward compatibility)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts_compacted: Option<usize>,
    /// Number of tokens freed by compaction (optional for backward compatibility)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_freed: Option<u64>,
}

impl CompactionMarker {
    /// Create a new basic compaction marker
    pub fn new(auto: bool) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp(),
            auto,
            marker_id: None,
            parts_compacted: None,
            tokens_freed: None,
        }
    }

    /// Create a basic compaction marker with explicit timestamp
    pub fn with_timestamp(timestamp: i64, auto: bool) -> Self {
        Self {
            timestamp,
            auto,
            marker_id: None,
            parts_compacted: None,
            tokens_freed: None,
        }
    }

    /// Create a detailed compaction marker with full metadata
    pub fn with_details(
        auto: bool,
        marker_id: String,
        parts_compacted: usize,
        tokens_freed: u64,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp(),
            auto,
            marker_id: Some(marker_id),
            parts_compacted: Some(parts_compacted),
            tokens_freed: Some(tokens_freed),
        }
    }
}
