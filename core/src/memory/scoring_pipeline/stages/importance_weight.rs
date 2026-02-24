//! Importance weighting stage.
//!
//! Multiplies scores by a factor derived from the fact's confidence
//! field, so higher-confidence facts rank higher.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Weights scores by fact importance / confidence.
pub struct ImportanceWeightStage;

impl ScoringStage for ImportanceWeightStage {
    fn name(&self) -> &str {
        "importance_weight"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        for c in &mut candidates {
            let importance = c.fact.confidence.clamp(0.0, 1.0);
            // Scale factor ranges from 0.7 (confidence=0) to 1.0 (confidence=1)
            c.score *= 0.7 + 0.3 * importance;
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

    fn scored_with_confidence(content: &str, score: f32, confidence: f32) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.confidence = confidence;
        ScoredFact { fact, score }
    }

    fn default_ctx() -> ScoringContext {
        ScoringContext {
            query: "test".to_string(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(),
        }
    }

    #[test]
    fn max_confidence_preserves_score() {
        let stage = ImportanceWeightStage;
        let candidates = vec![scored_with_confidence("high", 1.0, 1.0)];
        let result = stage.apply(candidates, &default_ctx());
        // 1.0 * (0.7 + 0.3 * 1.0) = 1.0
        assert!((result[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn zero_confidence_reduces_to_seventy_percent() {
        let stage = ImportanceWeightStage;
        let candidates = vec![scored_with_confidence("low", 1.0, 0.0)];
        let result = stage.apply(candidates, &default_ctx());
        // 1.0 * (0.7 + 0.3 * 0.0) = 0.7
        assert!((result[0].score - 0.7).abs() < 1e-5);
    }

    #[test]
    fn half_confidence_gives_expected_factor() {
        let stage = ImportanceWeightStage;
        let candidates = vec![scored_with_confidence("mid", 1.0, 0.5)];
        let result = stage.apply(candidates, &default_ctx());
        // 1.0 * (0.7 + 0.3 * 0.5) = 0.85
        assert!((result[0].score - 0.85).abs() < 1e-5);
    }

    #[test]
    fn reorders_by_importance_when_scores_close() {
        let stage = ImportanceWeightStage;
        let candidates = vec![
            scored_with_confidence("low_conf", 0.9, 0.1),
            scored_with_confidence("high_conf", 0.9, 1.0),
        ];
        let result = stage.apply(candidates, &default_ctx());
        assert_eq!(result[0].fact.content, "high_conf");
    }
}
