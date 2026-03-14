//! Tier promotion logic for memory facts.
//!
//! Evaluates whether a fact should be promoted from a lower tier to a higher one
//! based on configurable criteria (access count, age, strength).

use serde::{Deserialize, Serialize};

use super::context::{MemoryFact, MemoryTier};

/// Rule defining thresholds for a single tier transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionRule {
    /// Minimum number of accesses required.
    pub min_access_count: u32,
    /// Minimum age in days since creation.
    pub min_age_days: f32,
    /// Minimum strength score (0.0–1.0).
    pub min_strength: f32,
}

fn default_short_to_long() -> PromotionRule {
    PromotionRule {
        min_access_count: 3,
        min_age_days: 3.0,
        min_strength: 0.5,
    }
}

fn default_long_to_core() -> PromotionRule {
    PromotionRule {
        min_access_count: 10,
        min_age_days: 30.0,
        min_strength: 0.7,
    }
}

/// Configurable criteria for tier promotion decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionCriteria {
    /// Criteria for ShortTerm → LongTerm promotion.
    #[serde(default = "default_short_to_long")]
    pub short_to_long: PromotionRule,
    /// Criteria for LongTerm → Core promotion.
    #[serde(default = "default_long_to_core")]
    pub long_to_core: PromotionRule,
}

impl Default for PromotionCriteria {
    fn default() -> Self {
        Self {
            short_to_long: default_short_to_long(),
            long_to_core: default_long_to_core(),
        }
    }
}

/// Check whether a fact should be promoted to a higher tier.
///
/// Returns `Some(new_tier)` if the fact meets the promotion criteria,
/// or `None` if no promotion is warranted.
pub fn check_promotion(
    fact: &MemoryFact,
    strength: f32,
    now: i64,
    criteria: &PromotionCriteria,
) -> Option<MemoryTier> {
    let age_days = (now - fact.created_at) as f32 / 86400.0;

    match fact.tier {
        MemoryTier::ShortTerm => {
            let rule = &criteria.short_to_long;
            if fact.access_count >= rule.min_access_count
                && age_days >= rule.min_age_days
                && strength >= rule.min_strength
            {
                Some(MemoryTier::LongTerm)
            } else {
                None
            }
        }
        MemoryTier::LongTerm => {
            let rule = &criteria.long_to_core;
            if fact.access_count >= rule.min_access_count
                && age_days >= rule.min_age_days
                && strength >= rule.min_strength
            {
                Some(MemoryTier::Core)
            } else {
                None
            }
        }
        MemoryTier::Core => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    /// Helper to create a MemoryFact with specific test values.
    fn make_fact(tier: MemoryTier, access_count: u32, created_at: i64) -> MemoryFact {
        MemoryFact::new("test fact".to_string(), FactType::Other, vec![])
            .with_tier(tier)
            .with_access_count(access_count)
            .with_created_at(created_at)
    }

    const DAY: i64 = 86400;

    #[test]
    fn short_term_promotes_when_criteria_met() {
        let now = 100 * DAY;
        let fact = make_fact(MemoryTier::ShortTerm, 5, now - 10 * DAY);
        let criteria = PromotionCriteria::default();
        let result = check_promotion(&fact, 0.6, now, &criteria);
        assert_eq!(result, Some(MemoryTier::LongTerm));
    }

    #[test]
    fn short_term_stays_when_too_few_accesses() {
        let now = 100 * DAY;
        let fact = make_fact(MemoryTier::ShortTerm, 1, now - 10 * DAY);
        let criteria = PromotionCriteria::default();
        let result = check_promotion(&fact, 0.6, now, &criteria);
        assert_eq!(result, None);
    }

    #[test]
    fn short_term_stays_when_too_young() {
        let now = 100 * DAY;
        let fact = make_fact(MemoryTier::ShortTerm, 5, now - 1 * DAY);
        let criteria = PromotionCriteria::default();
        let result = check_promotion(&fact, 0.6, now, &criteria);
        assert_eq!(result, None);
    }

    #[test]
    fn core_never_promotes() {
        let now = 100 * DAY;
        let fact = make_fact(MemoryTier::Core, 100, now - 365 * DAY);
        let criteria = PromotionCriteria::default();
        let result = check_promotion(&fact, 0.9, now, &criteria);
        assert_eq!(result, None);
    }

    #[test]
    fn long_term_promotes_to_core() {
        let now = 100 * DAY;
        let fact = make_fact(MemoryTier::LongTerm, 15, now - 60 * DAY);
        let criteria = PromotionCriteria::default();
        let result = check_promotion(&fact, 0.8, now, &criteria);
        assert_eq!(result, Some(MemoryTier::Core));
    }
}
