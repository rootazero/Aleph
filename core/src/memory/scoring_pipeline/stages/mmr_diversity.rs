//! Maximal Marginal Relevance (MMR) diversity stage.
//!
//! De-duplicates semantically similar candidates using
//! `config.mmr_similarity_threshold`. Requires `query_embedding`
//! in the scoring context.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Removes near-duplicate candidates via MMR diversity filtering.
pub struct MmrDiversityStage;

impl ScoringStage for MmrDiversityStage {
    fn name(&self) -> &str {
        "mmr_diversity"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        // TODO: implement MMR diversity filtering
        candidates
    }
}
