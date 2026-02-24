//! Hard minimum score stage.
//!
//! Filters out candidates whose score falls below a configurable threshold.
//! Order is preserved (no re-sorting needed; just filtering).

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Discards candidates with `score < threshold`.
pub struct HardMinScore;

impl ScoringStage for HardMinScore {
    fn name(&self) -> &str {
        "hard_min_score"
    }

    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        _ctx: &ScoringContext,
        config: &ScoringPipelineConfig,
    ) {
        let threshold = config.hard_min_score;
        candidates.retain(|c| c.score >= threshold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn make_fact(score: f32, content: &str) -> ScoredFact {
        let fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        ScoredFact { fact, score }
    }

    #[test]
    fn filters_below_threshold() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            hard_min_score: 0.5,
            ..Default::default()
        };

        let mut candidates = vec![
            make_fact(0.8, "high"),
            make_fact(0.3, "low"),
            make_fact(0.6, "mid"),
        ];

        HardMinScore.apply(&mut candidates, &ctx, &config);

        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|c| c.score >= 0.5));
    }

    #[test]
    fn keeps_exact_threshold() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            hard_min_score: 0.5,
            ..Default::default()
        };

        let mut candidates = vec![make_fact(0.5, "exact")];
        HardMinScore.apply(&mut candidates, &ctx, &config);

        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn preserves_order() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            hard_min_score: 0.35,
            ..Default::default()
        };

        let mut candidates = vec![
            make_fact(0.9, "first"),
            make_fact(0.7, "second"),
            make_fact(0.1, "filtered"),
            make_fact(0.5, "third"),
        ];

        HardMinScore.apply(&mut candidates, &ctx, &config);

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].fact.content, "first");
        assert_eq!(candidates[1].fact.content, "second");
        assert_eq!(candidates[2].fact.content, "third");
    }

    #[test]
    fn all_below_threshold_returns_empty() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            hard_min_score: 0.9,
            ..Default::default()
        };

        let mut candidates = vec![make_fact(0.1, "a"), make_fact(0.5, "b")];
        HardMinScore.apply(&mut candidates, &ctx, &config);

        assert!(candidates.is_empty());
    }
}
