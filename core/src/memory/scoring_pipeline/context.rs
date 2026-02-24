//! Scoring context passed through the pipeline.
//!
//! Contains the query embedding, current timestamp, and any other
//! information that scoring stages may need.

/// Context available to every scoring stage.
#[derive(Debug, Clone)]
pub struct ScoringContext {
    /// Query embedding for cosine similarity computations.
    /// `None` when the query has no embedding (e.g., keyword-only search).
    pub query_embedding: Option<Vec<f32>>,

    /// Current Unix timestamp in seconds.
    /// Stages use this to compute age-based boosts / decays.
    pub timestamp: i64,
}

impl ScoringContext {
    /// Create a new scoring context.
    pub fn new(query_embedding: Option<Vec<f32>>, timestamp: i64) -> Self {
        Self {
            query_embedding,
            timestamp,
        }
    }

    /// Create a context with the current wall-clock time.
    pub fn now(query_embedding: Option<Vec<f32>>) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self::new(query_embedding, timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_new_stores_values() {
        let emb = vec![1.0, 2.0, 3.0];
        let ctx = ScoringContext::new(Some(emb.clone()), 1000);
        assert_eq!(ctx.query_embedding.unwrap(), emb);
        assert_eq!(ctx.timestamp, 1000);
    }

    #[test]
    fn context_now_has_reasonable_timestamp() {
        let ctx = ScoringContext::now(None);
        // Timestamp should be after 2020-01-01
        assert!(ctx.timestamp > 1_577_836_800);
        assert!(ctx.query_embedding.is_none());
    }
}
