//! Length normalization stage.
//!
//! Penalizes facts whose content length deviates significantly above
//! `config.length_norm_anchor` characters, using a logarithmic
//! attenuation curve. Short facts are NOT penalized.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Normalizes scores by fact text length.
pub struct LengthNormalizationStage;

impl ScoringStage for LengthNormalizationStage {
    fn name(&self) -> &str {
        "length_normalization"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let anchor = ctx.config.length_norm_anchor as f32;

        for c in &mut candidates {
            let ratio = (c.fact.content.len() as f32 / anchor).max(1.0);
            let factor = 1.0 / (1.0 + 0.5 * ratio.log2());
            c.score *= factor;
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
    use crate::memory::scoring_pipeline::context::ScoringContext;

    fn scored_with_content(content: &str, score: f32) -> ScoredFact {
        let fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        ScoredFact { fact, score }
    }

    fn ctx_with_anchor(anchor: usize) -> ScoringContext {
        ScoringContext {
            query: "test".to_string(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig {
                length_norm_anchor: anchor,
                ..Default::default()
            },
        }
    }

    #[test]
    fn at_anchor_no_penalty() {
        let stage = LengthNormalizationStage;
        // Content exactly at anchor length
        let content = "x".repeat(500);
        let candidates = vec![scored_with_content(&content, 1.0)];
        let ctx = ctx_with_anchor(500);
        let result = stage.apply(candidates, &ctx);
        // ratio = 1.0, log2(1.0) = 0, factor = 1.0 / (1 + 0) = 1.0
        assert!((result[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn short_content_no_penalty() {
        let stage = LengthNormalizationStage;
        // Content shorter than anchor
        let content = "short";
        let candidates = vec![scored_with_content(content, 1.0)];
        let ctx = ctx_with_anchor(500);
        let result = stage.apply(candidates, &ctx);
        // ratio clamped to 1.0, so factor = 1.0
        assert!((result[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn long_content_penalized() {
        let stage = LengthNormalizationStage;
        // 2000 chars with 500 anchor => ratio = 4.0
        let content = "x".repeat(2000);
        let candidates = vec![scored_with_content(&content, 1.0)];
        let ctx = ctx_with_anchor(500);
        let result = stage.apply(candidates, &ctx);
        // ratio = 4.0, log2(4.0) = 2.0, factor = 1/(1+1.0) = 0.5
        assert!((result[0].score - 0.5).abs() < 1e-5);
    }

    #[test]
    fn double_anchor_moderate_penalty() {
        let stage = LengthNormalizationStage;
        // 1000 chars with 500 anchor => ratio = 2.0
        let content = "x".repeat(1000);
        let candidates = vec![scored_with_content(&content, 1.0)];
        let ctx = ctx_with_anchor(500);
        let result = stage.apply(candidates, &ctx);
        // ratio = 2.0, log2(2.0) = 1.0, factor = 1/(1+0.5) ≈ 0.6667
        assert!((result[0].score - 2.0 / 3.0).abs() < 1e-4);
    }
}
