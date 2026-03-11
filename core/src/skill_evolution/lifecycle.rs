//! Skill lifecycle state machine with promotion/demotion logic.

use serde::{Deserialize, Serialize};

/// Lifecycle state of an evolved skill.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SkillLifecycleState {
    Draft,
    Shadow(ShadowState),
    Promoted {
        promoted_at: i64,
        shadow_duration_days: u32,
    },
    Observation {
        entered_at: i64,
        reason: ObservationReason,
        previous_vitality: f32,
    },
    Demoted {
        reason: String,
        demoted_at: i64,
    },
    Retired {
        reason: String,
        retired_at: i64,
    },
}

/// State tracking for a skill in shadow deployment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShadowState {
    pub deployed_at: i64,
    pub invocation_count: u32,
    pub success_count: u32,
}

impl ShadowState {
    /// Compute success rate, returning 0.0 if no invocations.
    pub fn success_rate(&self) -> f32 {
        if self.invocation_count == 0 {
            return 0.0;
        }
        self.success_count as f32 / self.invocation_count as f32
    }
}

/// Thresholds for promoting a skill from shadow to official.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromotionThresholds {
    pub min_invocations: u32,
    pub min_success_rate: f32,
    pub min_shadow_days: u32,
    pub must_beat_baseline: bool,
}

impl Default for PromotionThresholds {
    fn default() -> Self {
        Self {
            min_invocations: 5,
            min_success_rate: 0.85,
            min_shadow_days: 2,
            must_beat_baseline: true,
        }
    }
}

impl PromotionThresholds {
    /// Check if a shadow skill is eligible for promotion.
    pub fn is_eligible(&self, state: &ShadowState, days_in_shadow: u32) -> bool {
        state.invocation_count >= self.min_invocations
            && state.success_rate() >= self.min_success_rate
            && days_in_shadow >= self.min_shadow_days
    }
}

/// Triggers for demoting a skill.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DemotionTriggers {
    pub consecutive_failures: u32,
    pub success_rate_floor: f32,
    pub max_shadow_days: u32,
}

impl Default for DemotionTriggers {
    fn default() -> Self {
        Self {
            consecutive_failures: 3,
            success_rate_floor: 0.5,
            max_shadow_days: 30,
        }
    }
}

impl DemotionTriggers {
    /// Check if a skill should be demoted.
    pub fn should_demote(&self, consecutive_fails: u32, success_rate: f32) -> bool {
        consecutive_fails >= self.consecutive_failures || success_rate < self.success_rate_floor
    }
}

/// Reason a skill entered the observation period.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObservationReason {
    VitalityWarning,
    EntropyIncreasing,
    UserFeedback,
}

/// Origin information for an evolved skill.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkillOrigin {
    pub pattern_id: String,
    pub source_experiences: Vec<String>,
    pub generator_version: String,
    pub created_at: i64,
}

/// Metadata for an evolved skill (distinct from other SkillMetadata types).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvolvedSkillMetadata {
    pub skill_id: String,
    pub lifecycle: SkillLifecycleState,
    pub origin: SkillOrigin,
    pub risk_level: String,
    pub validation_history: Vec<String>,
}

/// Result of a lifecycle transition evaluation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LifecycleTransition {
    Unchanged,
    Promoted,
    Demoted { reason: String },
    Retired { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_eligible() {
        let thresholds = PromotionThresholds::default();
        let state = ShadowState {
            deployed_at: 0,
            invocation_count: 10,
            success_count: 9,
        };
        assert!(thresholds.is_eligible(&state, 3));
    }

    #[test]
    fn not_enough_invocations() {
        let thresholds = PromotionThresholds::default();
        let state = ShadowState {
            deployed_at: 0,
            invocation_count: 3,
            success_count: 3,
        };
        assert!(!thresholds.is_eligible(&state, 3));
    }

    #[test]
    fn too_low_success_rate() {
        let thresholds = PromotionThresholds::default();
        let state = ShadowState {
            deployed_at: 0,
            invocation_count: 10,
            success_count: 7,
        };
        // 0.7 < 0.85
        assert!(!thresholds.is_eligible(&state, 3));
    }

    #[test]
    fn demotion_consecutive_failures() {
        let triggers = DemotionTriggers::default();
        assert!(triggers.should_demote(3, 0.8));
    }

    #[test]
    fn demotion_low_success_rate() {
        let triggers = DemotionTriggers::default();
        assert!(triggers.should_demote(1, 0.4));
    }

    #[test]
    fn observation_state_serialization() {
        let state = SkillLifecycleState::Observation {
            entered_at: 1000,
            reason: ObservationReason::VitalityWarning,
            previous_vitality: 0.28,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SkillLifecycleState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn observation_reason_variants() {
        assert_ne!(
            ObservationReason::VitalityWarning,
            ObservationReason::EntropyIncreasing,
        );
        assert_ne!(
            ObservationReason::EntropyIncreasing,
            ObservationReason::UserFeedback,
        );
    }
}
