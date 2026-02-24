//! Time decay stage.
//!
//! Applies exponential decay based on fact age using
//! `config.time_decay_half_life_days`. The decay factor has a floor
//! at 0.5 so old facts are never completely zeroed out.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Applies exponential time decay to fact scores.
pub struct TimeDecayStage;

impl ScoringStage for TimeDecayStage {
    fn name(&self) -> &str {
        "time_decay"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let half_life = ctx.config.time_decay_half_life_days;

        if half_life <= 0.0 {
            return candidates;
        }

        for c in &mut candidates {
            let age_secs = (ctx.timestamp - c.fact.created_at).max(0) as f64;
            let age_days = age_secs / 86400.0;
            // decay ranges from 1.0 (brand new) down to floor of 0.5
            let decay = 0.5 + 0.5 * (-age_days / half_life as f64).exp();
            c.score *= decay as f32;
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

    fn ctx(timestamp: i64, half_life: f32) -> ScoringContext {
        ScoringContext {
            query: "test".to_string(),
            query_embedding: None,
            timestamp,
            config: ScoringPipelineConfig {
                time_decay_half_life_days: half_life,
                ..Default::default()
            },
        }
    }

    #[test]
    fn brand_new_no_decay() {
        let stage = TimeDecayStage;
        let now = 1700000000_i64;
        let candidates = vec![scored_at("new", 1.0, now)];
        let ctx = ctx(now, 60.0);
        let result = stage.apply(candidates, &ctx);
        // decay = 0.5 + 0.5 * exp(0) = 1.0
        assert!((result[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn very_old_floor_at_half() {
        let stage = TimeDecayStage;
        let now = 1700000000_i64;
        // 3650 days old (10 years), with 60 day half-life
        let created = now - 3650 * 86400;
        let candidates = vec![scored_at("ancient", 1.0, created)];
        let ctx = ctx(now, 60.0);
        let result = stage.apply(candidates, &ctx);
        // decay ≈ 0.5 + 0.5 * exp(-3650/60) ≈ 0.5 + ~0 ≈ 0.5
        assert!((result[0].score - 0.5).abs() < 0.01);
    }

    #[test]
    fn at_half_life_expected_decay() {
        let stage = TimeDecayStage;
        let now = 1700000000_i64;
        let half_life_days = 60.0_f32;
        let created = now - (half_life_days as i64) * 86400;
        let candidates = vec![scored_at("mid", 1.0, created)];
        let ctx = ctx(now, half_life_days);
        let result = stage.apply(candidates, &ctx);
        // decay = 0.5 + 0.5 * exp(-1) ≈ 0.5 + 0.5 * 0.3679 ≈ 0.6839
        let expected = 0.5 + 0.5 * (-1.0_f64).exp();
        assert!((result[0].score as f64 - expected).abs() < 1e-4);
        assert!((result[0].score - 0.6839).abs() < 0.01);
    }

    #[test]
    fn zero_half_life_disables() {
        let stage = TimeDecayStage;
        let now = 1700000000_i64;
        let candidates = vec![scored_at("a", 0.8, now - 30 * 86400)];
        let ctx = ctx(now, 0.0);
        let result = stage.apply(candidates, &ctx);
        assert!((result[0].score - 0.8).abs() < 1e-5);
    }
}
