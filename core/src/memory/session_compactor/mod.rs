//! Session Compactor — intra-session context management.
//!
//! Prevents token overflow in long conversations by summarizing tool call
//! sequences and older turns, while preserving a fresh tail of recent
//! messages verbatim.

use serde::{Deserialize, Serialize};

pub mod context_window;
pub mod fallback;
pub mod summary_engine;
pub mod tool_compactor;

// ---------------------------------------------------------------------------
// SessionCompactorConfig
// ---------------------------------------------------------------------------

/// Configuration for the SessionCompactor subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCompactorConfig {
    /// Enable or disable the compactor entirely.
    pub enabled: bool,
    /// Number of most-recent turns to keep verbatim (never compressed).
    pub fresh_tail_count: usize,
    /// Fraction of the context window at which compaction is triggered (0.0–1.0).
    pub context_threshold: f64,
    /// Target token count per leaf chunk when building the summary tree.
    pub leaf_chunk_tokens: usize,
    /// Minimum number of leaf chunks to merge at depth-1.
    pub d1_min_fanout: usize,
    /// Minimum number of depth-1 nodes to merge at depth-2.
    pub d2_min_fanout: usize,
    /// Maximum recursion depth for hierarchical summarization.
    pub max_summary_depth: u32,
    /// Characters-per-token ratio used for token estimation.
    pub token_estimate_ratio: f64,
    /// Hours to retain session-local facts before expiry.
    pub session_fact_retention_hours: u64,
    /// Minimum confidence required to promote a session-local fact to long-term memory.
    pub promote_confidence_threshold: f32,
}

impl Default for SessionCompactorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fresh_tail_count: 20,
            context_threshold: 0.75,
            leaf_chunk_tokens: 1000,
            d1_min_fanout: 4,
            d2_min_fanout: 3,
            max_summary_depth: 2,
            token_estimate_ratio: 3.5,
            session_fact_retention_hours: 24,
            promote_confidence_threshold: 0.8,
        }
    }
}
