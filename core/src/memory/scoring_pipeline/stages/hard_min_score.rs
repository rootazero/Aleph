//! Hard minimum score gate.
//!
//! Drops any candidate whose score falls below
//! `config.hard_min_score`. This is a pure filter — it does not
//! modify scores or re-order surviving candidates.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Drops candidates below the hard minimum score threshold.
pub struct HardMinScoreStage;

impl ScoringStage for HardMinScoreStage {
    fn name(&self) -> &str {
        "hard_min_score"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let threshold = ctx.config.hard_min_score;
        candidates
            .into_iter()
            .filter(|c| c.score >= threshold)
            .collect()
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

    fn scored(content: &str, score: f32) -> ScoredFact {
        ScoredFact {
            fact: MemoryFact::new(content.to_string(), FactType::Other, vec![]),
            score,
        }
    }

    fn ctx_with_threshold(threshold: f32) -> ScoringContext {
        ScoringContext {
            query: "test".to_string(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig {
                hard_min_score: threshold,
                ..Default::default()
            },
        }
    }

    #[test]
    fn filters_below_threshold() {
        let stage = HardMinScoreStage;
        let candidates = vec![
            scored("high", 0.9),
            scored("low", 0.1),
            scored("mid", 0.5),
        ];
        let ctx = ctx_with_threshold(0.35);
        let result = stage.apply(candidates, &ctx);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].fact.content, "high");
        assert_eq!(result[1].fact.content, "mid");
    }

    #[test]
    fn keeps_at_threshold() {
        let stage = HardMinScoreStage;
        let candidates = vec![scored("exact", 0.35)];
        let ctx = ctx_with_threshold(0.35);
        let result = stage.apply(candidates, &ctx);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact.content, "exact");
    }

    #[test]
    fn all_below_threshold_returns_empty() {
        let stage = HardMinScoreStage;
        let candidates = vec![scored("a", 0.1), scored("b", 0.2)];
        let ctx = ctx_with_threshold(0.5);
        let result = stage.apply(candidates, &ctx);
        assert!(result.is_empty());
    }
}
