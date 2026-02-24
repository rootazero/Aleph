//! Recency boost stage.
//!
//! Applies an exponential time-based boost to recently accessed or
//! created facts using `config.recency_half_life_days` and
//! `config.recency_weight`.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Boosts scores of recently created/accessed facts.
pub struct RecencyBoostStage;

impl ScoringStage for RecencyBoostStage {
    fn name(&self) -> &str {
        "recency_boost"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        // TODO: implement recency boost
        candidates
    }
}
