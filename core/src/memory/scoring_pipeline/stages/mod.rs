//! Scoring stages for the memory retrieval pipeline.
//!
//! Each stage implements the `ScoringStage` trait and transforms a
//! list of `ScoredFact` candidates in place. Stages are composed
//! into a pipeline by the parent module.

pub mod cosine_rerank;
pub mod hard_min_score;
pub mod importance_weight;
pub mod length_normalization;
pub mod mmr_diversity;
pub mod recency_boost;
pub mod time_decay;

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::store::types::ScoredFact;

/// A single stage in the scoring pipeline.
///
/// Each stage receives the current list of candidates (mutable) and may:
/// - Adjust scores (boost, decay, blend)
/// - Filter out candidates (hard threshold)
/// - Re-order candidates (diversity, MMR)
///
/// After modification, stages **must** leave candidates sorted by
/// descending score (unless the stage is order-preserving, like filtering).
pub trait ScoringStage: Send + Sync {
    /// Human-readable name for logging / debugging.
    fn name(&self) -> &str;

    /// Apply this stage to the candidate list.
    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        ctx: &ScoringContext,
        config: &ScoringPipelineConfig,
    );
}

/// Compute cosine similarity between two vectors.
///
/// Returns 0.0 if either vector is empty or they have different lengths.
/// Does NOT assume the vectors are normalized.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-12 {
        return 0.0;
    }

    (dot / denom) as f32
}

/// Sort candidates by descending score (stable).
pub(crate) fn sort_by_score_desc(candidates: &mut [ScoredFact]) {
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_empty_returns_zero() {
        assert!((cosine_similarity(&[], &[1.0]) - 0.0).abs() < 1e-5);
        assert!((cosine_similarity(&[1.0], &[]) - 0.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_length_mismatch_returns_zero() {
        assert!((cosine_similarity(&[1.0, 2.0], &[1.0]) - 0.0).abs() < 1e-5);
    }
}
