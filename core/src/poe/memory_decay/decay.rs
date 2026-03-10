//! Decay calculations for POE experience weights.
//!
//! The effective weight formula:
//!   effective_weight = performance_factor * drift_factor * time_factor
//!
//! - Performance: recent reuse success rate (dominant factor)
//! - Drift: how many related files have changed (dominant factor)
//! - Time: exponential half-life decay (weak tiebreaker)

use serde::{Deserialize, Serialize};

/// Configuration for memory decay calculations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    /// Half-life in days for time-based decay (default: 90)
    pub time_half_life_days: u32,
    /// Minimum reuses before performance decay kicks in (default: 3)
    pub min_reuses_for_decay: u32,
    /// Weight threshold below which experience is archived (default: 0.1)
    pub archive_threshold: f32,
    /// Window size for recent reuse performance (default: 5)
    pub performance_window: u32,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            time_half_life_days: 90,
            min_reuses_for_decay: 3,
            archive_threshold: 0.1,
            performance_window: 5,
        }
    }
}

/// Calculator for experience decay weights.
///
/// All methods are stateless and operate on provided inputs,
/// making the calculator easy to test and compose.
pub struct DecayCalculator;

impl DecayCalculator {
    /// Calculate the full effective weight.
    ///
    /// `effective_weight = performance_factor * drift_factor * time_factor`
    ///
    /// All factors should be in [0.0, 1.0]. The result is clamped to [0.0, 1.0].
    pub fn effective_weight(
        performance_factor: f32,
        drift_factor: f32,
        time_factor: f32,
    ) -> f32 {
        (performance_factor * drift_factor * time_factor).clamp(0.0, 1.0)
    }

    /// Performance factor based on recent reuse success/failure.
    ///
    /// Returns 1.0 if fewer than `min_reuses` attempts (benefit of the doubt).
    /// Otherwise returns `success_count / total_count` for the performance window.
    pub fn performance_factor(
        success_count: u32,
        total_count: u32,
        min_reuses: u32,
    ) -> f32 {
        if total_count < min_reuses || total_count == 0 {
            return 1.0;
        }
        success_count as f32 / total_count as f32
    }

    /// Environment drift factor.
    ///
    /// `1.0` = no files changed, `0.0` = all related files changed.
    ///
    /// Formula: `1.0 - (changed_files / total_files).clamp(0.0, 1.0)`
    ///
    /// Returns 1.0 if `total_related_files` is 0 (no files to drift).
    pub fn drift_factor(
        related_files_changed: usize,
        total_related_files: usize,
    ) -> f32 {
        if total_related_files == 0 {
            return 1.0;
        }
        let ratio = related_files_changed as f32 / total_related_files as f32;
        1.0 - ratio.clamp(0.0, 1.0)
    }

    /// Time decay using exponential half-life.
    ///
    /// Formula: `0.5^(age_days / half_life_days)`
    ///
    /// Always returns at least 0.01 (never fully zero).
    pub fn time_factor(age_days: f64, half_life_days: u32) -> f32 {
        if half_life_days == 0 {
            return 0.01;
        }
        let exponent = age_days / half_life_days as f64;
        let factor = 0.5_f64.powf(exponent) as f32;
        factor.max(0.01)
    }

    /// Check if an experience should be archived based on its weight.
    pub fn should_archive(weight: f32, config: &DecayConfig) -> bool {
        weight < config.archive_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_performance_factor_all_success() {
        let factor = DecayCalculator::performance_factor(5, 5, 3);
        assert!((factor - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_performance_factor_all_failure() {
        let factor = DecayCalculator::performance_factor(0, 5, 3);
        assert!(factor.abs() < f32::EPSILON);
    }

    #[test]
    fn test_performance_factor_below_min_reuses() {
        // Benefit of the doubt: too few reuses => 1.0
        let factor = DecayCalculator::performance_factor(0, 2, 3);
        assert!((factor - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_performance_factor_mixed() {
        let factor = DecayCalculator::performance_factor(3, 5, 3);
        assert!((factor - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_drift_factor_no_changes() {
        let factor = DecayCalculator::drift_factor(0, 10);
        assert!((factor - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_drift_factor_all_changed() {
        let factor = DecayCalculator::drift_factor(10, 10);
        assert!(factor.abs() < f32::EPSILON);
    }

    #[test]
    fn test_drift_factor_partial_change() {
        let factor = DecayCalculator::drift_factor(5, 10);
        assert!((factor - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_drift_factor_no_files() {
        // No related files => no drift => 1.0
        let factor = DecayCalculator::drift_factor(0, 0);
        assert!((factor - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_time_factor_at_half_life() {
        let factor = DecayCalculator::time_factor(90.0, 90);
        assert!((factor - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_time_factor_fresh() {
        let factor = DecayCalculator::time_factor(0.0, 90);
        assert!((factor - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_time_factor_very_old() {
        // 10 half-lives => 0.5^10 ~= 0.001, but floor is 0.01
        let factor = DecayCalculator::time_factor(900.0, 90);
        assert!(factor >= 0.01);
        assert!(factor <= 0.02);
    }

    #[test]
    fn test_time_factor_zero_half_life() {
        let factor = DecayCalculator::time_factor(10.0, 0);
        assert!((factor - 0.01).abs() < f32::EPSILON);
    }

    #[test]
    fn test_effective_weight_combined() {
        let weight = DecayCalculator::effective_weight(0.8, 0.5, 0.9);
        let expected = 0.8 * 0.5 * 0.9;
        assert!((weight - expected).abs() < 0.001);
    }

    #[test]
    fn test_effective_weight_clamped() {
        // Should clamp to [0.0, 1.0]
        let weight = DecayCalculator::effective_weight(1.0, 1.0, 1.0);
        assert!(weight <= 1.0);
        let weight = DecayCalculator::effective_weight(0.0, 0.5, 0.5);
        assert!(weight >= 0.0);
    }

    #[test]
    fn test_should_archive_below_threshold() {
        let config = DecayConfig::default();
        assert!(DecayCalculator::should_archive(0.05, &config));
    }

    #[test]
    fn test_should_archive_above_threshold() {
        let config = DecayConfig::default();
        assert!(!DecayCalculator::should_archive(0.5, &config));
    }

    #[test]
    fn test_should_archive_at_threshold() {
        let config = DecayConfig {
            archive_threshold: 0.1,
            ..Default::default()
        };
        // At threshold (not below) => not archived
        assert!(!DecayCalculator::should_archive(0.1, &config));
    }
}
