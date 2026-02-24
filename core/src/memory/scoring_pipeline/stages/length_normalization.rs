//! Length normalization stage.
//!
//! Penalizes excessively long content. Content at or below the anchor
//! length receives no penalty; longer content gets a logarithmic penalty.

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::{sort_by_score_desc, ScoringStage};
use crate::memory::store::types::ScoredFact;

/// Length normalization: `score *= 1.0 / (1.0 + 0.5 * log2(ratio))`.
///
/// `ratio = max(content.len() / anchor, 1.0)` — short content is not boosted.
pub struct LengthNormalization;

impl ScoringStage for LengthNormalization {
    fn name(&self) -> &str {
        "length_normalization"
    }

    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        _ctx: &ScoringContext,
        config: &ScoringPipelineConfig,
    ) {
        let anchor = config.length_norm_anchor.max(1) as f64;

        for candidate in candidates.iter_mut() {
            let len = candidate.fact.content.len() as f64;
            let ratio = (len / anchor).max(1.0); // clamp >= 1.0
            let factor = 1.0 / (1.0 + 0.5 * ratio.log2());
            candidate.score *= factor as f32;
        }

        sort_by_score_desc(candidates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn make_fact_with_content(content: &str, score: f32) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.confidence = 1.0;
        ScoredFact { fact, score }
    }

    #[test]
    fn content_at_anchor_length_no_penalty() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            length_norm_anchor: 100,
            ..Default::default()
        };

        let content = "a".repeat(100);
        let mut candidates = vec![make_fact_with_content(&content, 0.8)];
        LengthNormalization.apply(&mut candidates, &ctx, &config);

        // ratio = 1.0, log2(1.0) = 0.0, factor = 1.0
        assert!((candidates[0].score - 0.8).abs() < 1e-5);
    }

    #[test]
    fn short_content_no_boost() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            length_norm_anchor: 500,
            ..Default::default()
        };

        let content = "short";
        let mut candidates = vec![make_fact_with_content(content, 0.8)];
        LengthNormalization.apply(&mut candidates, &ctx, &config);

        // ratio = max(5/500, 1.0) = 1.0 → no change
        assert!((candidates[0].score - 0.8).abs() < 1e-5);
    }

    #[test]
    fn double_anchor_gets_penalized() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            length_norm_anchor: 100,
            ..Default::default()
        };

        let content = "a".repeat(200); // 2x anchor
        let mut candidates = vec![make_fact_with_content(&content, 1.0)];
        LengthNormalization.apply(&mut candidates, &ctx, &config);

        // ratio = 2.0, log2(2) = 1.0, factor = 1/(1+0.5) ≈ 0.6667
        assert!((candidates[0].score - (1.0 / 1.5)).abs() < 1e-4);
    }

    #[test]
    fn longer_content_gets_more_penalty() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            length_norm_anchor: 100,
            ..Default::default()
        };

        let short_content = "a".repeat(100);
        let long_content = "a".repeat(800);
        let mut candidates = vec![
            make_fact_with_content(&short_content, 0.8),
            make_fact_with_content(&long_content, 0.8),
        ];
        LengthNormalization.apply(&mut candidates, &ctx, &config);

        // Short (at anchor) should score higher than long
        assert!(candidates[0].score > candidates[1].score);
    }
}
