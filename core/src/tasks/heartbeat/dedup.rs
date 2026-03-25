//! Deduplication Engine
//!
//! Prevents redundant notifications by comparing new probe outputs
//! against recent history using cosine similarity on embeddings.
//!
//! The `DedupEngine` is a placeholder here; full embedding-backed
//! deduplication will be wired up in Task 8 when the service layer
//! provides a DB connection and embedding provider.

use crate::tasks::heartbeat::config::DedupConfig;

// ── cosine_similarity ─────────────────────────────────────────────────────────

/// Compute cosine similarity between two equal-length float vectors.
///
/// Returns 0.0 if either vector is a zero vector.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

// ── DedupEngine ───────────────────────────────────────────────────────────────

/// Deduplication engine for heartbeat outputs.
///
/// Currently a placeholder — the full implementation (DB conn + embedding
/// provider) will be added in Task 8 when the service layer is wired up.
pub struct DedupEngine {
    #[allow(dead_code)]
    config: DedupConfig,
    // Will be filled in Task 8 with DB conn + embedding provider
}

impl DedupEngine {
    pub fn new(config: DedupConfig) -> Self {
        Self { config }
    }

    /// Check whether the given output is a duplicate of recent outputs.
    ///
    /// Always returns false until the embedding provider is wired in (Task 8).
    pub async fn is_duplicate(&self, _task_id: &str, _output: &str) -> bool {
        false
    }

    /// Record an output for future deduplication comparisons.
    ///
    /// No-op until the embedding provider is wired in (Task 8).
    pub async fn record(&self, _task_id: &str, _output: &str) {
        // TODO: implement with embedding provider
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_zero() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_proportional() {
        // Vectors pointing in the same direction (just scaled differently)
        // should give similarity 1.0
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![2.0, 4.0, 6.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_dedup_engine_placeholder_never_deduplicates() {
        let engine = DedupEngine::new(DedupConfig::default());
        assert!(!engine.is_duplicate("task-1", "some output").await);
        engine.record("task-1", "some output").await;
        // Still no dedup since embedding provider not wired
        assert!(!engine.is_duplicate("task-1", "some output").await);
    }
}
