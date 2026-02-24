//! Scoring stages for the memory retrieval pipeline.
//!
//! Each stage implements the [`ScoringStage`] trait, receiving a list of
//! scored facts and returning a (possibly re-ordered or filtered) list.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::store::types::ScoredFact;

/// A single scoring stage in the pipeline.
///
/// Stages are applied sequentially. Each stage may adjust scores, re-order
/// candidates, or remove candidates that fail a threshold check.
pub trait ScoringStage: Send + Sync {
    /// Human-readable name for logging / debugging.
    fn name(&self) -> &str;

    /// Apply this stage to the candidate list and return the (possibly
    /// modified) result.
    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact>;
}

pub mod cosine_rerank;
pub mod recency_boost;
pub mod importance_weight;
pub mod length_normalization;
pub mod time_decay;
pub mod hard_min_score;
pub mod mmr_diversity;
