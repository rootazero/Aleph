//! Vitality Score engine for continuous skill health assessment.
//!
//! Computes a 0.0–1.0 vitality score from success rate, invocation frequency,
//! maintenance cost, and user feedback. Used by the lifecycle manager to drive
//! promotion, observation, demotion, and retirement transitions.

use serde::{Deserialize, Serialize};

/// Components that make up a vitality score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VitalityComponents {
    pub success_rate: f32,
    pub frequency_score: f32,
    pub maintenance_cost_inverse: f32,
    pub user_feedback_multiplier: f32,
}

/// Computed vitality score with its breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VitalityScore {
    pub value: f32,
    pub components: VitalityComponents,
}

/// Configuration for vitality computation.
#[derive(Debug, Clone)]
pub struct VitalityConfig {
    /// Expected invocations per 30-day window (denominator for frequency).
    pub expected_frequency: f32,
    /// Token-cost normalizer (baseline single LLM call cost).
    pub cost_normalizer: f32,
    /// Token-equivalent penalty per retry.
    pub retry_penalty: f32,
    /// Vitality threshold: healthy (no action).
    pub healthy_threshold: f32,
    /// Vitality threshold: enter observation.
    pub warning_threshold: f32,
    /// Vitality threshold: trigger demotion.
    pub demotion_threshold: f32,
    /// Vitality threshold: trigger retirement.
    pub retirement_threshold: f32,
}

impl Default for VitalityConfig {
    fn default() -> Self {
        Self {
            expected_frequency: 10.0,
            cost_normalizer: 2000.0,
            retry_penalty: 500.0,
            healthy_threshold: 0.5,
            warning_threshold: 0.3,
            demotion_threshold: 0.15,
            retirement_threshold: 0.05,
        }
    }
}

/// Input data for vitality computation.
pub struct VitalityInput {
    /// Success rate from SkillMetrics (0.0–1.0).
    pub success_rate: f32,
    /// Invocations in the last 30 days.
    pub invocations_last_30d: u32,
    /// Average tokens consumed per invocation.
    pub avg_tokens: f32,
    /// Average retries per invocation.
    pub avg_retries: f32,
    /// Current user feedback multiplier (1.0 = neutral).
    pub user_feedback_multiplier: f32,
}

impl VitalityScore {
    /// Compute vitality from input metrics and config.
    pub fn compute(input: &VitalityInput, config: &VitalityConfig) -> Self {
        let success_rate = input.success_rate.clamp(0.0, 1.0);

        let frequency_score =
            (input.invocations_last_30d as f32 / config.expected_frequency).min(1.0);

        let raw_cost = input.avg_tokens + (input.avg_retries * config.retry_penalty);
        let maintenance_cost_inverse = 1.0 / (1.0 + raw_cost / config.cost_normalizer);

        let user_mul = input.user_feedback_multiplier.clamp(0.0, 1.0);

        let value =
            (success_rate * frequency_score * maintenance_cost_inverse * user_mul).clamp(0.0, 1.0);

        Self {
            value,
            components: VitalityComponents {
                success_rate,
                frequency_score,
                maintenance_cost_inverse,
                user_feedback_multiplier: user_mul,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 10,
            avg_tokens: 500.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 1.0,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        // 1.0 * 1.0 * (1/(1+500/2000)) * 1.0 = 1.0 * 1.0 * 0.8 * 1.0 = 0.8
        assert!((score.value - 0.8).abs() < 0.01);
    }

    #[test]
    fn zero_invocations_zero_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 0,
            avg_tokens: 500.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 1.0,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        assert!((score.value - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn negative_feedback_reduces_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 10,
            avg_tokens: 500.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 0.7,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        // 1.0 * 1.0 * 0.8 * 0.7 = 0.56
        assert!((score.value - 0.56).abs() < 0.01);
    }

    #[test]
    fn high_cost_reduces_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 10,
            avg_tokens: 10000.0,
            avg_retries: 2.0,
            user_feedback_multiplier: 1.0,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        // cost = 10000 + 2*500 = 11000; inverse = 1/(1+11000/2000) = 1/6.5 ≈ 0.154
        assert!(score.value < 0.2);
    }

    #[test]
    fn values_clamped() {
        let input = VitalityInput {
            success_rate: 1.5,
            invocations_last_30d: 100,
            avg_tokens: 0.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 2.0,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        assert!(score.value <= 1.0);
        assert!(score.components.success_rate <= 1.0);
        assert!(score.components.user_feedback_multiplier <= 1.0);
    }
}
