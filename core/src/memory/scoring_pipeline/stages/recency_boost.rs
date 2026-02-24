//! Recency boost stage.
//!
//! Applies an exponential time-based boost to recently created facts
//! using `config.recency_half_life_days` and `config.recency_weight`.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Boosts scores of recently created/accessed facts.
pub struct RecencyBoostStage;

impl ScoringStage for RecencyBoostStage {
    fn name(&self) -> &str {
        "recency_boost"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let weight = ctx.config.recency_weight;
        let half_life = ctx.config.recency_half_life_days;

        if weight <= 0.0 || half_life <= 0.0 {
            return candidates;
        }

        for c in &mut candidates {
            let age_secs = (ctx.timestamp - c.fact.created_at).max(0) as f64;
            let age_days = age_secs / 86400.0;
            let boost = (-age_days / half_life as f64).exp() * weight as f64;
            c.score += boost as f32;
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

    fn scored_at(content: &str, score: f32, created_at: i64) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.created_at = created_at;
        ScoredFact { fact, score }
    }

    fn ctx(timestamp: i64, weight: f32, half_life: f32) -> ScoringContext {
        ScoringContext {
            query: "test".to_string(),
            query_embedding: None,
            timestamp,
            config: ScoringPipelineConfig {
                recency_weight: weight,
                recency_half_life_days: half_life,
                ..Default::default()
            },
        }
    }

    #[test]
    fn new_memory_gets_full_boost() {
        let stage = RecencyBoostStage;
        let now = 1700000000_i64;
        let candidates = vec![scored_at("recent", 0.5, now)];
        let ctx = ctx(now, 0.1, 14.0);
        let result = stage.apply(candidates, &ctx);
        // age_days = 0, boost = exp(0) * 0.1 = 0.1
        assert!((result[0].score - 0.6).abs() < 1e-5);
    }

    #[test]
    fn old_memory_gets_small_boost() {
        let stage = RecencyBoostStage;
        let now = 1700000000_i64;
        // 60 days old
        let created = now - 60 * 86400;
        let candidates = vec![scored_at("old", 0.5, created)];
        let ctx = ctx(now, 0.1, 14.0);
        let result = stage.apply(candidates, &ctx);
        // age_days = 60, boost = exp(-60/14) * 0.1 ≈ 0.0014
        let expected_boost = (-60.0_f64 / 14.0).exp() * 0.1;
        assert!((result[0].score as f64 - (0.5 + expected_boost)).abs() < 1e-4);
        assert!(result[0].score < 0.502); // very small boost
    }

    #[test]
    fn zero_weight_disables_boost() {
        let stage = RecencyBoostStage;
        let now = 1700000000_i64;
        let candidates = vec![scored_at("a", 0.5, now)];
        let ctx = ctx(now, 0.0, 14.0);
        let result = stage.apply(candidates, &ctx);
        assert!((result[0].score - 0.5).abs() < 1e-5);
    }

    #[test]
    fn recency_reorders_same_score_candidates() {
        let stage = RecencyBoostStage;
        let now = 1700000000_i64;
        let candidates = vec![
            scored_at("old", 0.8, now - 30 * 86400),
            scored_at("new", 0.8, now),
        ];
        let ctx = ctx(now, 0.1, 14.0);
        let result = stage.apply(candidates, &ctx);
        assert_eq!(result[0].fact.content, "new");
    }
}
