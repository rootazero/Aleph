//! Cognitive entropy tracker for priority-driven dreaming.
//!
//! Analyzes execution history to detect patterns whose outcomes are
//! diverging (entropy increasing) and therefore most likely to benefit
//! from background crystallization.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Trend direction of a pattern's entropy over recent executions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntropyTrend {
    /// Entropy is rising — pattern is degrading, highest priority.
    Increasing,
    /// Entropy is volatile — insufficient data for a clear trend.
    Volatile,
    /// Entropy has converged — no action needed.
    Stable,
    /// Entropy is falling — pattern is improving, lowest priority.
    Decreasing,
}

impl EntropyTrend {
    /// Numeric priority (lower = more urgent).
    pub fn priority(&self) -> u8 {
        match self {
            EntropyTrend::Increasing => 0,
            EntropyTrend::Volatile => 1,
            EntropyTrend::Stable => 2,
            EntropyTrend::Decreasing => 3,
        }
    }
}

/// Report for a single pattern's cognitive entropy.
#[derive(Debug, Clone)]
pub struct EntropyReport {
    /// The pattern ID this report covers.
    pub pattern_id: String,
    /// Average recent entropy (distance score).
    pub recent_entropy: f32,
    /// Direction the entropy is heading.
    pub entropy_trend: EntropyTrend,
    /// Number of executions analyzed.
    pub execution_count: u32,
}

/// Stateless analyzer for cognitive entropy across patterns.
///
/// All methods are static — no instance state is needed.
pub struct CognitiveEntropyTracker;

impl CognitiveEntropyTracker {
    /// Compute the trend direction from a series of entropy values.
    ///
    /// - Fewer than 3 values → `Volatile`
    /// - Split at midpoint; if second-half avg exceeds first-half avg by >0.1 → `Increasing`
    /// - If second-half avg is lower by >0.1 → `Decreasing`
    /// - Otherwise → `Stable`
    pub fn compute_trend(values: &[f32]) -> EntropyTrend {
        if values.len() < 3 {
            return EntropyTrend::Volatile;
        }

        let mid = values.len() / 2;
        let first_half = &values[..mid];
        let second_half = &values[mid..];

        let first_avg: f32 = first_half.iter().sum::<f32>() / first_half.len() as f32;
        let second_avg: f32 = second_half.iter().sum::<f32>() / second_half.len() as f32;

        let diff = second_avg - first_avg;
        if diff > 0.1 {
            EntropyTrend::Increasing
        } else if diff < -0.1 {
            EntropyTrend::Decreasing
        } else {
            EntropyTrend::Stable
        }
    }

    /// Analyze execution history and produce prioritized entropy reports.
    ///
    /// # Arguments
    /// * `executions` — Map from pattern_id to Vec of `(satisfaction, distance_score)` tuples.
    /// * `entropy_threshold` — Minimum average entropy to include in results.
    ///
    /// # Returns
    /// Reports sorted by trend priority (ascending) then entropy (descending).
    pub fn analyze(
        executions: &HashMap<String, Vec<(f32, f32)>>,
        entropy_threshold: f32,
    ) -> Vec<EntropyReport> {
        let mut reports = Vec::new();

        for (pattern_id, data) in executions {
            if data.len() < 3 {
                continue;
            }

            // Extract distance scores (second element)
            let distances: Vec<f32> = data.iter().map(|(_, d)| *d).collect();
            let avg_entropy: f32 = distances.iter().sum::<f32>() / distances.len() as f32;
            let trend = Self::compute_trend(&distances);

            // Include if above threshold or if trend is increasing
            if avg_entropy >= entropy_threshold || trend == EntropyTrend::Increasing {
                reports.push(EntropyReport {
                    pattern_id: pattern_id.clone(),
                    recent_entropy: avg_entropy,
                    entropy_trend: trend,
                    execution_count: data.len() as u32,
                });
            }
        }

        // Sort by trend priority asc, then entropy desc
        reports.sort_by(|a, b| {
            a.entropy_trend
                .priority()
                .cmp(&b.entropy_trend.priority())
                .then_with(|| {
                    b.recent_entropy
                        .partial_cmp(&a.recent_entropy)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_trend_increasing() {
        let values = vec![0.2, 0.3, 0.25, 0.7, 0.8, 0.75];
        assert_eq!(
            CognitiveEntropyTracker::compute_trend(&values),
            EntropyTrend::Increasing
        );
    }

    #[test]
    fn compute_trend_decreasing() {
        let values = vec![0.8, 0.7, 0.75, 0.2, 0.3, 0.25];
        assert_eq!(
            CognitiveEntropyTracker::compute_trend(&values),
            EntropyTrend::Decreasing
        );
    }

    #[test]
    fn compute_trend_stable() {
        let values = vec![0.5, 0.52, 0.48, 0.51, 0.49, 0.50];
        assert_eq!(
            CognitiveEntropyTracker::compute_trend(&values),
            EntropyTrend::Stable
        );
    }

    #[test]
    fn compute_trend_volatile_too_few() {
        let values = vec![0.5, 0.6];
        assert_eq!(
            CognitiveEntropyTracker::compute_trend(&values),
            EntropyTrend::Volatile
        );
    }

    #[test]
    fn entropy_trend_priority_ordering() {
        assert!(EntropyTrend::Increasing.priority() < EntropyTrend::Volatile.priority());
        assert!(EntropyTrend::Volatile.priority() < EntropyTrend::Stable.priority());
        assert!(EntropyTrend::Stable.priority() < EntropyTrend::Decreasing.priority());
    }
}
