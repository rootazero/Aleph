//! Length normalization stage.
//!
//! Adjusts scores based on fact text length relative to
//! `config.length_norm_anchor` to prevent very short or very long
//! facts from dominating results.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Normalizes scores by fact text length.
pub struct LengthNormalizationStage;

impl ScoringStage for LengthNormalizationStage {
    fn name(&self) -> &str {
        "length_normalization"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        // TODO: implement length normalization
        candidates
    }
}
