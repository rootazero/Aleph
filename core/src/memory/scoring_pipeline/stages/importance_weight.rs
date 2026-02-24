//! Importance weighting stage.
//!
//! Multiplies scores by the fact's inherent importance / confidence
//! to surface high-value memories.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Weights scores by fact importance / confidence.
pub struct ImportanceWeightStage;

impl ScoringStage for ImportanceWeightStage {
    fn name(&self) -> &str {
        "importance_weight"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        // TODO: implement importance weighting
        candidates
    }
}
