//! Scoring context passed through every pipeline stage.
//!
//! Bundles the query text, optional pre-computed embedding, current
//! timestamp, and pipeline configuration so that each stage has
//! everything it needs without extra parameters.

use super::config::ScoringPipelineConfig;

/// Context shared across all scoring stages in a single pipeline run.
pub struct ScoringContext {
    /// The original user query text.
    pub query: String,

    /// Pre-computed embedding for the query (if available).
    /// Stages like MMR diversity use this for cosine-similarity checks.
    pub query_embedding: Option<Vec<f32>>,

    /// Current Unix timestamp in seconds. Used by recency / time-decay stages.
    pub timestamp: i64,

    /// Pipeline configuration knobs.
    pub config: ScoringPipelineConfig,
}
