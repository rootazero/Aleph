//! Differential testing engine for comparing skill efficiency against baselines.

use serde::{Deserialize, Serialize};

/// Efficiency comparison between a skill and its baseline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EfficiencyDiff {
    pub skill_avg_steps: f32,
    pub baseline_avg_steps: f32,
    pub skill_avg_tokens: f32,
    pub baseline_avg_tokens: f32,
    pub is_more_efficient: bool,
}

impl EfficiencyDiff {
    /// Compute efficiency diff with a tolerance factor.
    ///
    /// A skill is considered "more efficient" if both:
    /// - skill_steps <= baseline_steps * (1.0 + tolerance)
    /// - skill_tokens <= baseline_tokens * (1.0 + tolerance)
    pub fn compute(
        skill_steps: f32,
        skill_tokens: f32,
        baseline_steps: f32,
        baseline_tokens: f32,
        tolerance: f32,
    ) -> Self {
        let step_ok = skill_steps <= baseline_steps * (1.0 + tolerance);
        let token_ok = skill_tokens <= baseline_tokens * (1.0 + tolerance);
        Self {
            skill_avg_steps: skill_steps,
            baseline_avg_steps: baseline_steps,
            skill_avg_tokens: skill_tokens,
            baseline_avg_tokens: baseline_tokens,
            is_more_efficient: step_ok && token_ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_efficient_passes() {
        let diff = EfficiencyDiff::compute(3.0, 500.0, 5.0, 800.0, 0.1);
        assert!(diff.is_more_efficient);
    }

    #[test]
    fn less_efficient_fails() {
        let diff = EfficiencyDiff::compute(8.0, 1200.0, 5.0, 800.0, 0.1);
        assert!(!diff.is_more_efficient);
    }

    #[test]
    fn within_tolerance_passes() {
        // 10% more steps and tokens, but within 0.1 tolerance
        let diff = EfficiencyDiff::compute(5.5, 880.0, 5.0, 800.0, 0.1);
        assert!(diff.is_more_efficient);
    }
}
