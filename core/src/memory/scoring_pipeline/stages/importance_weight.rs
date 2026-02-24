//! Importance weight stage.
//!
//! Scales scores by the fact's confidence value, ensuring that even
//! low-confidence facts keep at least 70% of their score.

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::{sort_by_score_desc, ScoringStage};
use crate::memory::store::types::ScoredFact;

/// Multiplicative importance weight: `score *= 0.7 + 0.3 * confidence`.
pub struct ImportanceWeight;

impl ScoringStage for ImportanceWeight {
    fn name(&self) -> &str {
        "importance_weight"
    }

    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        _ctx: &ScoringContext,
        _config: &ScoringPipelineConfig,
    ) {
        for candidate in candidates.iter_mut() {
            let importance = candidate.fact.confidence.clamp(0.0, 1.0);
            let factor = 0.7 + 0.3 * importance;
            candidate.score *= factor;
        }

        sort_by_score_desc(candidates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn make_fact_with_confidence(confidence: f32, score: f32) -> ScoredFact {
        let mut fact = MemoryFact::new("test".to_string(), FactType::Other, vec![]);
        fact.confidence = confidence;
        ScoredFact { fact, score }
    }

    #[test]
    fn full_confidence_preserves_score() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig::default();

        let mut candidates = vec![make_fact_with_confidence(1.0, 0.8)];
        ImportanceWeight.apply(&mut candidates, &ctx, &config);

        // factor = 0.7 + 0.3*1.0 = 1.0
        assert!((candidates[0].score - 0.8).abs() < 1e-5);
    }

    #[test]
    fn zero_confidence_applies_70_percent() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig::default();

        let mut candidates = vec![make_fact_with_confidence(0.0, 1.0)];
        ImportanceWeight.apply(&mut candidates, &ctx, &config);

        // factor = 0.7 + 0.3*0.0 = 0.7
        assert!((candidates[0].score - 0.7).abs() < 1e-5);
    }

    #[test]
    fn half_confidence_applies_85_percent() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig::default();

        let mut candidates = vec![make_fact_with_confidence(0.5, 1.0)];
        ImportanceWeight.apply(&mut candidates, &ctx, &config);

        // factor = 0.7 + 0.3*0.5 = 0.85
        assert!((candidates[0].score - 0.85).abs() < 1e-5);
    }

    #[test]
    fn reorders_by_weighted_score() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig::default();

        let mut candidates = vec![
            make_fact_with_confidence(0.0, 0.9), // 0.9 * 0.7 = 0.63
            make_fact_with_confidence(1.0, 0.7), // 0.7 * 1.0 = 0.70
        ];

        ImportanceWeight.apply(&mut candidates, &ctx, &config);

        assert!((candidates[0].fact.confidence - 1.0).abs() < 1e-5);
    }
}
