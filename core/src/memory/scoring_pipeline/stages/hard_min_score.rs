//! Hard minimum score gate.
//!
//! Drops any candidate whose score falls below
//! `config.hard_min_score`. This is a filter, not a re-scorer.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Drops candidates below the hard minimum score threshold.
pub struct HardMinScoreStage;

impl ScoringStage for HardMinScoreStage {
    fn name(&self) -> &str {
        "hard_min_score"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        // TODO: implement hard minimum score filtering
        candidates
    }
}
