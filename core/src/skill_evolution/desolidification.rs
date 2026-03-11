//! Auto-de-solidification with triple-tiered triggers.
//!
//! Layer 1: Circuit Breaker — immediate demotion on catastrophic failure.
//! Layer 2: Entropy Canary — vitality penalty on degradation trends.
//! Layer 3: User Feedback — amplifier on human negative signals.

use serde::{Deserialize, Serialize};
use tracing::info;

// ============================================================================
// Circuit Breaker (Layer 1)
// ============================================================================

/// Configuration for the circuit breaker (immediate demotion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    pub consecutive_failure_limit: u32,
    pub success_rate_floor: f32,
    pub window_size: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            consecutive_failure_limit: 3,
            success_rate_floor: 0.5,
            window_size: 10,
        }
    }
}

/// Result of a circuit breaker check.
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitBreakerVerdict {
    /// Skill is healthy, no action.
    Healthy,
    /// Skill has tripped the breaker — demote immediately.
    Tripped { reason: String },
}

/// Checks recent execution window for catastrophic failure patterns.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self { config }
    }

    /// Check a window of recent execution outcomes (true = success, false = failure).
    /// Most recent execution is last in the slice.
    pub fn check(&self, recent_outcomes: &[bool]) -> CircuitBreakerVerdict {
        if recent_outcomes.is_empty() {
            return CircuitBreakerVerdict::Healthy;
        }

        // Check consecutive failures from the tail
        let consecutive_fails = recent_outcomes
            .iter()
            .rev()
            .take_while(|&&success| !success)
            .count() as u32;

        if consecutive_fails >= self.config.consecutive_failure_limit {
            info!(
                target: "aleph::evolution::probe",
                probe = "circuit_breaker_tripped",
                trigger = "consecutive_failures",
                consecutive_fails = consecutive_fails,
                limit = self.config.consecutive_failure_limit,
                "Circuit breaker TRIPPED — consecutive failures"
            );
            return CircuitBreakerVerdict::Tripped {
                reason: format!(
                    "{} consecutive failures (limit: {})",
                    consecutive_fails, self.config.consecutive_failure_limit
                ),
            };
        }

        // Check success rate over window
        let window: Vec<_> = recent_outcomes
            .iter()
            .rev()
            .take(self.config.window_size as usize)
            .collect();
        let success_count = window.iter().filter(|&&s| *s).count() as f32;
        let rate = success_count / window.len() as f32;

        if rate < self.config.success_rate_floor {
            info!(
                target: "aleph::evolution::probe",
                probe = "circuit_breaker_tripped",
                trigger = "low_success_rate",
                success_rate = rate,
                floor = self.config.success_rate_floor,
                "Circuit breaker TRIPPED — low success rate"
            );
            return CircuitBreakerVerdict::Tripped {
                reason: format!(
                    "success rate {:.0}% below floor {:.0}%",
                    rate * 100.0,
                    self.config.success_rate_floor * 100.0
                ),
            };
        }

        CircuitBreakerVerdict::Healthy
    }
}

// ============================================================================
// Entropy Canary (Layer 2)
// ============================================================================

/// Configuration for the entropy canary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropyCanaryConfig {
    /// How much to penalize vitality when entropy is increasing.
    pub entropy_penalty: f32,
    /// Duration degradation threshold (50% = 0.5).
    pub duration_degradation_threshold: f32,
}

impl Default for EntropyCanaryConfig {
    fn default() -> Self {
        Self {
            entropy_penalty: 0.2,
            duration_degradation_threshold: 0.5,
        }
    }
}

/// Computes a vitality penalty based on entropy trend and duration changes.
pub fn compute_entropy_penalty(
    entropy_increasing: bool,
    duration_baseline_ms: f32,
    duration_current_ms: f32,
    config: &EntropyCanaryConfig,
) -> f32 {
    let mut penalty = 0.0;

    if entropy_increasing {
        penalty += config.entropy_penalty;
    }

    if duration_baseline_ms > 0.0 {
        let degradation = (duration_current_ms - duration_baseline_ms) / duration_baseline_ms;
        if degradation > config.duration_degradation_threshold {
            penalty += config.entropy_penalty * 0.5;
        }
    }

    let result = penalty.min(0.5); // cap total penalty

    if result > 0.0 {
        info!(
            target: "aleph::evolution::probe",
            probe = "entropy_canary_penalty",
            penalty = result,
            entropy_increasing = entropy_increasing,
            duration_baseline_ms = duration_baseline_ms,
            duration_current_ms = duration_current_ms,
            "Entropy canary applied vitality penalty"
        );
    }

    result
}

// ============================================================================
// User Feedback (Layer 3)
// ============================================================================

/// Type of user feedback event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FeedbackType {
    Positive,
    Negative,
    ManualEdit,
}

/// A recorded user feedback event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFeedbackEvent {
    pub skill_id: String,
    pub feedback_type: FeedbackType,
    pub timestamp: i64,
}

/// Apply a feedback event to a user_feedback_multiplier.
pub fn apply_feedback(current_multiplier: f32, feedback: &FeedbackType) -> f32 {
    let new_mul = match feedback {
        FeedbackType::Negative | FeedbackType::ManualEdit => (current_multiplier * 0.7).max(0.1),
        FeedbackType::Positive => (current_multiplier * 1.1).min(1.0),
    };

    info!(
        target: "aleph::evolution::probe",
        probe = "user_feedback_applied",
        feedback_type = ?feedback,
        previous_multiplier = current_multiplier,
        new_multiplier = new_mul,
        "User feedback applied to vitality multiplier"
    );

    new_mul
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Circuit Breaker tests --

    #[test]
    fn breaker_healthy_all_success() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let outcomes = vec![true, true, true, true, true];
        assert_eq!(breaker.check(&outcomes), CircuitBreakerVerdict::Healthy);
    }

    #[test]
    fn breaker_trips_consecutive_failures() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let outcomes = vec![true, true, false, false, false];
        match breaker.check(&outcomes) {
            CircuitBreakerVerdict::Tripped { reason } => {
                assert!(reason.contains("3 consecutive failures"));
            }
            _ => panic!("Expected tripped"),
        }
    }

    #[test]
    fn breaker_trips_low_success_rate() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        // 3 success, 7 failures interleaved so consecutive limit isn't hit first
        // Last element is success to avoid consecutive-failure trigger
        let outcomes = vec![false, false, true, false, false, true, false, false, true, false, false, true];
        // window=10 takes last 10: [true, false, false, true, false, false, true, false, false, true]
        // consecutive fails from tail = 0 (last is true)
        // success rate in window of 10 = 4/10 = 40% < 50%
        match breaker.check(&outcomes) {
            CircuitBreakerVerdict::Tripped { reason } => {
                assert!(reason.contains("below floor"));
            }
            _ => panic!("Expected tripped"),
        }
    }

    #[test]
    fn breaker_healthy_on_empty() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert_eq!(breaker.check(&[]), CircuitBreakerVerdict::Healthy);
    }

    // -- Entropy Canary tests --

    #[test]
    fn entropy_penalty_when_increasing() {
        let config = EntropyCanaryConfig::default();
        let penalty = compute_entropy_penalty(true, 1000.0, 1000.0, &config);
        assert!((penalty - 0.2).abs() < 0.01);
    }

    #[test]
    fn entropy_penalty_with_duration_degradation() {
        let config = EntropyCanaryConfig::default();
        // 60% slower + entropy increasing = 0.2 + 0.1 = 0.3
        let penalty = compute_entropy_penalty(true, 1000.0, 1600.0, &config);
        assert!((penalty - 0.3).abs() < 0.01);
    }

    #[test]
    fn entropy_penalty_capped() {
        let config = EntropyCanaryConfig {
            entropy_penalty: 0.4,
            duration_degradation_threshold: 0.1,
        };
        let penalty = compute_entropy_penalty(true, 100.0, 1000.0, &config);
        assert!((penalty - 0.5).abs() < 0.01); // capped at 0.5
    }

    #[test]
    fn no_penalty_when_healthy() {
        let config = EntropyCanaryConfig::default();
        let penalty = compute_entropy_penalty(false, 1000.0, 1000.0, &config);
        assert!((penalty - 0.0).abs() < f32::EPSILON);
    }

    // -- User Feedback tests --

    #[test]
    fn negative_feedback_reduces_multiplier() {
        let mul = apply_feedback(1.0, &FeedbackType::Negative);
        assert!((mul - 0.7).abs() < 0.01);
    }

    #[test]
    fn manual_edit_reduces_multiplier() {
        let mul = apply_feedback(1.0, &FeedbackType::ManualEdit);
        assert!((mul - 0.7).abs() < 0.01);
    }

    #[test]
    fn positive_feedback_recovers_multiplier() {
        let mul = apply_feedback(0.7, &FeedbackType::Positive);
        assert!((mul - 0.77).abs() < 0.01);
    }

    #[test]
    fn positive_feedback_capped_at_one() {
        let mul = apply_feedback(0.95, &FeedbackType::Positive);
        assert!((mul - 1.0).abs() < 0.01);
    }

    #[test]
    fn multiplier_has_floor() {
        let mut mul = 1.0;
        for _ in 0..20 {
            mul = apply_feedback(mul, &FeedbackType::Negative);
        }
        assert!(mul >= 0.1);
    }
}
