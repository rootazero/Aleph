//! Time decay stage.
//!
//! Applies exponential decay based on fact age using
//! `config.time_decay_half_life_days`. Older facts receive lower
//! scores unless they have been recently accessed.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Applies exponential time decay to fact scores.
pub struct TimeDecayStage;

impl ScoringStage for TimeDecayStage {
    fn name(&self) -> &str {
        "time_decay"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        // TODO: implement time decay
        candidates
    }
}
