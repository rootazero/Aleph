//! Cosine-rerank blending stage.
//!
//! Interpolates between raw vector similarity and reranker scores
//! using `config.rerank_blend`.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Blends vector cosine similarity with reranker scores.
pub struct CosineRerankStage;

impl ScoringStage for CosineRerankStage {
    fn name(&self) -> &str {
        "cosine_rerank"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        // TODO: implement cosine-rerank blending
        candidates
    }
}
