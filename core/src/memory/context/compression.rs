//! Compression session records and statistics.

use serde::{Deserialize, Serialize};

/// Record of a compression session for auditing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionSession {
    /// Session ID (UUID)
    pub id: String,
    /// Source memory IDs that were compressed
    pub source_memory_ids: Vec<String>,
    /// Extracted fact IDs
    pub extracted_fact_ids: Vec<String>,
    /// Compression timestamp
    pub compressed_at: i64,
    /// AI provider used for extraction
    pub provider_used: String,
    /// Compression duration in milliseconds
    pub duration_ms: u64,
}

impl CompressionSession {
    /// Create a new compression session record
    pub fn new(
        source_memory_ids: Vec<String>,
        extracted_fact_ids: Vec<String>,
        provider_used: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            source_memory_ids,
            extracted_fact_ids,
            compressed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            provider_used,
            duration_ms,
        }
    }
}

/// Statistics for memory facts
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactStats {
    /// Total number of facts
    pub total_facts: u64,
    /// Number of valid (non-invalidated) facts
    pub valid_facts: u64,
    /// Breakdown by fact type
    pub facts_by_type: std::collections::HashMap<String, u64>,
    /// Oldest fact timestamp
    pub oldest_fact_timestamp: Option<i64>,
    /// Newest fact timestamp
    pub newest_fact_timestamp: Option<i64>,
}

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
    pub fn empty() -> Self {
        Self::default()
    }
}
