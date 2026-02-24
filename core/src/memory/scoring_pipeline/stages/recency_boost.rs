//! Recency boost stage.
//!
//! Applies an additive exponential boost to recently created facts.
//! Brand-new facts get the full weight; old facts get near-zero boost.

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::{sort_by_score_desc, ScoringStage};
use crate::memory::store::types::ScoredFact;

/// Additive recency boost: `score += exp(-age_days / half_life) * weight`.
pub struct RecencyBoost;

impl ScoringStage for RecencyBoost {
    fn name(&self) -> &str {
        "recency_boost"
    }

    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        ctx: &ScoringContext,
        config: &ScoringPipelineConfig,
    ) {
        let weight = config.recency_weight;
        let half_life = config.recency_half_life_days;

        // Skip if disabled
        if weight <= 0.0 || half_life <= 0.0 {
            return;
        }

        for candidate in candidates.iter_mut() {
            let age_secs = (ctx.timestamp - candidate.fact.created_at).max(0) as f64;
            let age_days = age_secs / 86400.0;
            let boost = (-age_days / half_life).exp() * weight;
            candidate.score += boost as f32;
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
    fn brand_new_fact_gets_full_boost() {
        let now = 1_000_000i64;
        let ctx = ScoringContext::new(None, now);
        let config = ScoringPipelineConfig {
            recency_weight: 0.1,
            recency_half_life_days: 14.0,
            ..Default::default()
        };

        let mut candidates = vec![make_fact_at(now, 0.5)];
        RecencyBoost.apply(&mut candidates, &ctx, &config);

        // age=0 → exp(0)=1.0 → boost = 0.1
        assert!((candidates[0].score - 0.6).abs() < 1e-5);
    }

    #[test]
    fn old_fact_gets_negligible_boost() {
        let now = 1_000_000i64;
        let created_at = now - 365 * 86400; // 365 days ago
        let ctx = ScoringContext::new(None, now);
        let config = ScoringPipelineConfig {
            recency_weight: 0.1,
            recency_half_life_days: 14.0,
            ..Default::default()
        };

        let mut candidates = vec![make_fact_at(created_at, 0.5)];
        RecencyBoost.apply(&mut candidates, &ctx, &config);

        // exp(-365/14) ≈ 0 → score stays near 0.5
        assert!((candidates[0].score - 0.5).abs() < 0.001);
    }

    #[test]
    fn skips_when_weight_is_zero() {
        let ctx = ScoringContext::new(None, 1_000_000);
        let config = ScoringPipelineConfig {
            recency_weight: 0.0,
            ..Default::default()
        };

        let mut candidates = vec![make_fact_at(1_000_000, 0.5)];
        RecencyBoost.apply(&mut candidates, &ctx, &config);

        assert!((candidates[0].score - 0.5).abs() < 1e-5);
    }

    #[test]
    fn sorts_recent_fact_higher() {
        let now = 1_000_000i64;
        let ctx = ScoringContext::new(None, now);
        let config = ScoringPipelineConfig {
            recency_weight: 0.2,
            recency_half_life_days: 7.0,
            ..Default::default()
        };

        let mut candidates = vec![
            make_fact_at(now - 30 * 86400, 0.6), // old, same base score
            make_fact_at(now, 0.5),                // new, lower base score
        ];

        RecencyBoost.apply(&mut candidates, &ctx, &config);

        // New fact: 0.5 + 0.2*1.0 = 0.7
        // Old fact: 0.6 + 0.2*exp(-30/7) ≈ 0.6 + tiny
        assert_eq!(candidates[0].fact.created_at, now);
    }
}
