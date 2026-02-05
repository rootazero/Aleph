use serde::{Deserialize, Serialize};

/// Configuration for transcript indexing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptIndexerConfig {
    /// Maximum tokens per chunk (default: 400)
    pub max_tokens_per_chunk: usize,

    /// Overlap tokens between chunks (default: 80)
    pub overlap_tokens: usize,

    /// Enable chunking for long transcripts (default: true)
    pub enable_chunking: bool,
}

impl Default for TranscriptIndexerConfig {
    fn default() -> Self {
        Self {
            max_tokens_per_chunk: 400,
            overlap_tokens: 80,
            enable_chunking: true,
        }
    }
}
