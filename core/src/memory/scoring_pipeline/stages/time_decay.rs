//! Time decay stage.
//!
//! Multiplicative decay that reduces scores for old memories,
//! but floors at 50% so ancient memories are never fully discarded
//! by this stage alone.

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::{sort_by_score_desc, ScoringStage};
use crate::memory::store::types::ScoredFact;

/// Multiplicative time decay: `score *= 0.5 + 0.5 * exp(-age_days / half_life)`.
///
/// - Brand new (age=0): factor = 1.0
/// - Very old: factor → 0.5 (floor)
pub struct TimeDecay;

impl ScoringStage for TimeDecay {
    fn name(&self) -> &str {
        "time_decay"
    }

    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        ctx: &ScoringContext,
        config: &ScoringPipelineConfig,
    ) {
        let half_life = config.time_decay_half_life_days;
        if half_life <= 0.0 {
            return;
        }

        for candidate in candidates.iter_mut() {
            let age_secs = (ctx.timestamp - candidate.fact.created_at).max(0) as f64;
            let age_days = age_secs / 86400.0;
            let factor = 0.5 + 0.5 * (-age_days / half_life).exp();
            candidate.score *= factor as f32;
        }

        sort_by_score_desc(candidates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn make_fact_at(created_at: i64, score: f32) -> ScoredFact {
        let mut fact = MemoryFact::new("test".to_string(), FactType::Other, vec![]);
        fact.created_at = created_at;
        ScoredFact { fact, score }
    }

    #[test]
    fn brand_new_fact_no_decay() {
        let now = 1_000_000i64;
        let ctx = ScoringContext::new(None, now);
        let config = ScoringPipelineConfig {
            time_decay_half_life_days: 60.0,
            ..Default::default()
        };

        let mut candidates = vec![make_fact_at(now, 1.0)];
        TimeDecay.apply(&mut candidates, &ctx, &config);

        // factor = 0.5 + 0.5 * exp(0) = 1.0
        assert!((candidates[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn very_old_fact_floors_at_50_percent() {
        let now = 1_000_000i64;
        let created_at = now - 10 * 365 * 86400; // 10 years ago
        let ctx = ScoringContext::new(None, now);
        let config = ScoringPipelineConfig {
            time_decay_half_life_days: 60.0,
            ..Default::default()
        };

        let mut candidates = vec![make_fact_at(created_at, 1.0)];
        TimeDecay.apply(&mut candidates, &ctx, &config);

        // exp(-3650/60) ≈ 0 → factor ≈ 0.5
        assert!((candidates[0].score - 0.5).abs() < 0.01);
    }

    #[test]
    fn at_half_life_decays_to_expected_value() {
        let now = 1_000_000i64;
        let half_life_days = 60.0;
        let created_at = now - (half_life_days as i64) * 86400;
        let ctx = ScoringContext::new(None, now);
        let config = ScoringPipelineConfig {
            time_decay_half_life_days: half_life_days,
            ..Default::default()
        };

        let mut candidates = vec![make_fact_at(created_at, 1.0)];
        TimeDecay.apply(&mut candidates, &ctx, &config);

        // factor = 0.5 + 0.5 * exp(-1) ≈ 0.5 + 0.5*0.3679 ≈ 0.6839
        let expected = 0.5 + 0.5 * (-1.0f64).exp();
        assert!((candidates[0].score - expected as f32).abs() < 1e-4);
    }

    #[test]
    fn newer_fact_scores_higher_after_decay() {
        let now = 1_000_000i64;
        let ctx = ScoringContext::new(None, now);
        let config = ScoringPipelineConfig {
            time_decay_half_life_days: 30.0,
            ..Default::default()
        };

        let mut candidates = vec![
            make_fact_at(now - 180 * 86400, 1.0), // 6 months old
            make_fact_at(now - 1 * 86400, 0.8),   // 1 day old
        ];
        TimeDecay.apply(&mut candidates, &ctx, &config);

        // 1-day old should rank first despite lower base score
        assert!(candidates[0].score > candidates[1].score);
        assert_eq!(candidates[0].fact.created_at, now - 86400);
    }
}
