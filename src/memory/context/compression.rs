//! Compression result records.

use serde::{Deserialize, Serialize};

/// Result of a compression operation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressionResult {
    /// Number of memories processed
    pub memories_processed: u32,
    /// Number of facts extracted
    pub facts_extracted: u32,
    /// Number of old facts invalidated due to conflicts
    pub facts_invalidated: u32,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

impl CompressionResult {
    /// Create an empty result (no work done)
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}
