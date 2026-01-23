//! Statistical analysis for A/B experiments
//!
//! This module provides:
//! - SignificanceResult: Result of statistical test
//! - SignificanceCalculator: Welch's t-test implementation
//! - ExperimentStatus: Current experiment status
//! - MetricSummary: Summary statistics for metrics
//! - VariantSummary: Summary for a variant
//! - ExperimentReport: Complete experiment report

use super::tracking::{MetricStats, VariantStats};
use super::types::{ExperimentConfig, ExperimentId, TrackedMetric, VariantId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

// ============================================================================
// Significance Result
// ============================================================================

/// Result of significance test between two variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignificanceResult {
    /// The metric being compared
    pub metric: TrackedMetric,
    /// Control variant identifier
    pub control_id: VariantId,
    /// Control variant statistics
    pub control_mean: f64,
    pub control_std_dev: f64,
    pub control_n: u64,
    /// Treatment variant identifier
    pub treatment_id: VariantId,
    /// Treatment variant statistics
    pub treatment_mean: f64,
    pub treatment_std_dev: f64,
    pub treatment_n: u64,
    /// T-statistic from Welch's t-test
    pub t_statistic: f64,
    /// Degrees of freedom
    pub degrees_of_freedom: f64,
    /// Two-tailed p-value
    pub p_value: f64,
    /// Whether the result is statistically significant (p < 0.05)
    pub is_significant: bool,
    /// Relative change: (treatment - control) / control
    pub relative_change: f64,
    /// Cohen's d effect size
    pub cohens_d: f64,
}

// ============================================================================
// Significance Calculator
// ============================================================================

/// Calculator for statistical significance
pub struct SignificanceCalculator;

impl SignificanceCalculator {
    /// Perform Welch's t-test comparing two variants for a given metric
    pub fn t_test(
        metric: TrackedMetric,
        control_id: &str,
        control: &MetricStats,
        treatment_id: &str,
        treatment: &MetricStats,
    ) -> Option<SignificanceResult> {
        // Need at least 2 samples in each group
        if control.count < 2 || treatment.count < 2 {
            return None;
        }

        let control_mean = control.mean();
        let treatment_mean = treatment.mean();
        let control_var = control.variance();
        let treatment_var = treatment.variance();
        let control_n = control.count as f64;
        let treatment_n = treatment.count as f64;

        // Welch's t-test
        let se_diff_sq = (control_var / control_n) + (treatment_var / treatment_n);
        if se_diff_sq <= 0.0 {
            return None;
        }
        let se_diff = se_diff_sq.sqrt();

        let t_statistic = (treatment_mean - control_mean) / se_diff;

        // Welch-Satterthwaite degrees of freedom
        let num = se_diff_sq.powi(2);
        let denom = (control_var / control_n).powi(2) / (control_n - 1.0)
            + (treatment_var / treatment_n).powi(2) / (treatment_n - 1.0);
        let df = if denom > 0.0 { num / denom } else { 1.0 };

        // Two-tailed p-value (using simple approximation)
        let p_value = Self::t_distribution_p_value(t_statistic.abs(), df);

        // Relative change
        let relative_change = if control_mean != 0.0 {
            (treatment_mean - control_mean) / control_mean.abs()
        } else {
            0.0
        };

        // Cohen's d effect size
        let pooled_std = ((control_var + treatment_var) / 2.0).sqrt();
        let cohens_d = if pooled_std > 0.0 {
            (treatment_mean - control_mean) / pooled_std
        } else {
            0.0
        };

        Some(SignificanceResult {
            metric,
            control_id: control_id.to_string(),
            control_mean,
            control_std_dev: control.std_dev(),
            control_n: control.count,
            treatment_id: treatment_id.to_string(),
            treatment_mean,
            treatment_std_dev: treatment.std_dev(),
            treatment_n: treatment.count,
            t_statistic,
            degrees_of_freedom: df,
            p_value,
            is_significant: p_value < 0.05,
            relative_change,
            cohens_d,
        })
    }

    /// Approximate p-value from t-distribution using normal approximation
    /// (Good enough for df > 30, which is typical for A/B tests)
    fn t_distribution_p_value(t: f64, df: f64) -> f64 {
        // For large df, t-distribution approaches normal distribution
        // Use a simple approximation based on error function
        // This is adequate for our use case (df typically > 30)
        if df >= 30.0 {
            // Normal approximation
            2.0 * (1.0 - Self::normal_cdf(t))
        } else {
            // Use a more accurate approximation for smaller df
            // Based on the Abramowitz and Stegun approximation
            let x = df / (df + t * t);
            let p = 0.5 * Self::regularized_incomplete_beta(df / 2.0, 0.5, x);
            2.0 * p.min(1.0 - p)
        }
    }

    /// Standard normal CDF approximation
    fn normal_cdf(x: f64) -> f64 {
        0.5 * (1.0 + Self::erf(x / std::f64::consts::SQRT_2))
    }

    /// Error function approximation (Abramowitz and Stegun 7.1.26)
    fn erf(x: f64) -> f64 {
        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x = x.abs();

        let a1 = 0.254829592;
        let a2 = -0.284496736;
        let a3 = 1.421413741;
        let a4 = -1.453152027;
        let a5 = 1.061405429;
        let p = 0.3275911;

        let t = 1.0 / (1.0 + p * x);
        let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x).exp();

        sign * y
    }

    /// Regularized incomplete beta function approximation
    /// (Simplified version for t-distribution)
    fn regularized_incomplete_beta(a: f64, b: f64, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }

        // Use continued fraction expansion for better accuracy
        // This is a simplified version
        let bt = if x == 0.0 || x == 1.0 {
            0.0
        } else {
            (Self::ln_gamma(a + b) - Self::ln_gamma(a) - Self::ln_gamma(b)
                + a * x.ln()
                + b * (1.0 - x).ln())
            .exp()
        };

        if x < (a + 1.0) / (a + b + 2.0) {
            bt * Self::beta_cf(a, b, x) / a
        } else {
            1.0 - bt * Self::beta_cf(b, a, 1.0 - x) / b
        }
    }

    /// Continued fraction for incomplete beta function
    fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
        let max_iter = 100;
        let eps = 3.0e-7;

        let qab = a + b;
        let qap = a + 1.0;
        let qam = a - 1.0;

        let mut c = 1.0;
        let mut d = 1.0 - qab * x / qap;
        if d.abs() < 1e-30 {
            d = 1e-30;
        }
        d = 1.0 / d;
        let mut h = d;

        for m in 1..=max_iter {
            let m = m as f64;
            let m2 = 2.0 * m;

            let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
            d = 1.0 + aa * d;
            if d.abs() < 1e-30 {
                d = 1e-30;
            }
            c = 1.0 + aa / c;
            if c.abs() < 1e-30 {
                c = 1e-30;
            }
            d = 1.0 / d;
            h *= d * c;

            let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
            d = 1.0 + aa * d;
            if d.abs() < 1e-30 {
                d = 1e-30;
            }
            c = 1.0 + aa / c;
            if c.abs() < 1e-30 {
                c = 1e-30;
            }
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;

            if (delta - 1.0).abs() < eps {
                break;
            }
        }

        h
    }

    /// Log gamma function (Stirling approximation)
    fn ln_gamma(x: f64) -> f64 {
        if x <= 0.0 {
            return f64::INFINITY;
        }

        // Stirling's approximation with correction terms
        let x = x - 1.0;
        let tmp = x + 5.5;
        let ser = 1.000000000190015 + 76.18009172947146 / (x + 1.0) - 86.50532032941677 / (x + 2.0)
            + 24.01409824083091 / (x + 3.0)
            - 1.231739572450155 / (x + 4.0)
            + 0.1208650973866179e-2 / (x + 5.0)
            - 0.5395239384953e-5 / (x + 6.0);

        -(tmp - (x + 0.5) * tmp.ln() + ser.ln()) + (2.5066282746310005 * ser / (x + 1.0)).ln()
    }
}

// ============================================================================
// Experiment Status
// ============================================================================

/// Current status of an experiment
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    /// Experiment is actively running
    Running,
    /// Experiment is paused (can be resumed)
    Paused,
    /// Experiment has completed (end time passed)
    Completed,
    /// Insufficient data for analysis
    InsufficientData,
}

impl ExperimentStatus {
    /// Get human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            ExperimentStatus::Running => "Running",
            ExperimentStatus::Paused => "Paused",
            ExperimentStatus::Completed => "Completed",
            ExperimentStatus::InsufficientData => "Insufficient Data",
        }
    }
}

impl std::fmt::Display for ExperimentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ============================================================================
// Summary Types
// ============================================================================

/// Summary of a single metric for a variant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSummary {
    /// Mean value
    pub mean: f64,
    /// Standard deviation
    pub std_dev: f64,
    /// Minimum value
    pub min: f64,
    /// Maximum value
    pub max: f64,
    /// Sample count
    pub count: u64,
}

impl From<&MetricStats> for MetricSummary {
    fn from(stats: &MetricStats) -> Self {
        Self {
            mean: stats.mean(),
            std_dev: stats.std_dev(),
            min: if stats.min.is_infinite() {
                0.0
            } else {
                stats.min
            },
            max: if stats.max.is_infinite() {
                0.0
            } else {
                stats.max
            },
            count: stats.count,
        }
    }
}

/// Summary for a single variant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantSummary {
    /// Variant identifier
    pub variant_id: VariantId,
    /// Variant display name
    pub variant_name: String,
    /// Total sample count
    pub sample_count: u64,
    /// Sample percentage of total
    pub sample_percentage: f64,
    /// Per-metric summaries
    pub metrics: HashMap<TrackedMetric, MetricSummary>,
}

// ============================================================================
// Experiment Report
// ============================================================================

/// Complete experiment report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentReport {
    /// Experiment identifier
    pub experiment_id: ExperimentId,
    /// Experiment display name
    pub experiment_name: String,
    /// Current status
    pub status: ExperimentStatus,
    /// How long the experiment has been running
    pub duration_secs: u64,
    /// Total samples across all variants
    pub total_samples: u64,
    /// Per-variant summaries
    pub variant_summaries: Vec<VariantSummary>,
    /// Significance tests for each tracked metric
    pub significance_tests: Vec<SignificanceResult>,
    /// Automated recommendation (if any)
    pub recommendation: Option<String>,
}

impl ExperimentReport {
    /// Generate a report from experiment config and stats
    pub fn generate(config: &ExperimentConfig, stats: &HashMap<VariantId, VariantStats>) -> Self {
        let total_samples: u64 = stats.values().map(|v| v.sample_count).sum();

        // Determine status
        let status = if !config.enabled {
            ExperimentStatus::Paused
        } else if let Some(end) = config.end_time {
            if SystemTime::now() > end {
                ExperimentStatus::Completed
            } else {
                ExperimentStatus::Running
            }
        } else if total_samples < 60 {
            // At least 30 per variant for 2 variants
            ExperimentStatus::InsufficientData
        } else {
            ExperimentStatus::Running
        };

        // Calculate duration
        let duration_secs = config
            .start_time
            .and_then(|start| SystemTime::now().duration_since(start).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Build variant summaries
        let variant_summaries: Vec<_> = config
            .variants
            .iter()
            .map(|v| {
                let variant_stats = stats.get(&v.id);
                let sample_count = variant_stats.map(|s| s.sample_count).unwrap_or(0);
                let sample_percentage = if total_samples > 0 {
                    (sample_count as f64 / total_samples as f64) * 100.0
                } else {
                    0.0
                };

                let metrics = variant_stats
                    .map(|s| {
                        s.metrics
                            .iter()
                            .map(|(m, ms)| (m.clone(), MetricSummary::from(ms)))
                            .collect()
                    })
                    .unwrap_or_default();

                VariantSummary {
                    variant_id: v.id.clone(),
                    variant_name: v.name.clone(),
                    sample_count,
                    sample_percentage,
                    metrics,
                }
            })
            .collect();

        // Run significance tests for each metric
        let mut significance_tests = Vec::new();
        if config.variants.len() >= 2 {
            let control_id = &config.variants[0].id;
            let control_stats = stats.get(control_id);

            for variant in config.variants.iter().skip(1) {
                let treatment_stats = stats.get(&variant.id);

                if let (Some(control), Some(treatment)) = (control_stats, treatment_stats) {
                    for metric in &config.tracked_metrics {
                        if let (Some(control_metric), Some(treatment_metric)) =
                            (control.get_metric(metric), treatment.get_metric(metric))
                        {
                            if let Some(result) = SignificanceCalculator::t_test(
                                metric.clone(),
                                control_id,
                                control_metric,
                                &variant.id,
                                treatment_metric,
                            ) {
                                significance_tests.push(result);
                            }
                        }
                    }
                }
            }
        }

        // Generate recommendation
        let recommendation = Self::generate_recommendation(&significance_tests);

        ExperimentReport {
            experiment_id: config.id.clone(),
            experiment_name: config.name.clone(),
            status,
            duration_secs,
            total_samples,
            variant_summaries,
            significance_tests,
            recommendation,
        }
    }

    /// Generate automated recommendation based on significance tests
    fn generate_recommendation(tests: &[SignificanceResult]) -> Option<String> {
        if tests.is_empty() {
            return None;
        }

        // Find significant improvements
        let significant_improvements: Vec<_> = tests
            .iter()
            .filter(|t| t.is_significant && t.relative_change < 0.0) // Lower is better for most metrics
            .collect();

        let significant_regressions: Vec<_> = tests
            .iter()
            .filter(|t| t.is_significant && t.relative_change > 0.0)
            .collect();

        if !significant_improvements.is_empty() && significant_regressions.is_empty() {
            let best = significant_improvements
                .iter()
                .max_by(|a, b| a.cohens_d.abs().partial_cmp(&b.cohens_d.abs()).unwrap())
                .unwrap();
            Some(format!(
                "Treatment '{}' shows significant improvement in {} ({:.1}% change, p={:.4}). Consider rolling out.",
                best.treatment_id,
                best.metric,
                best.relative_change * 100.0,
                best.p_value
            ))
        } else if !significant_regressions.is_empty() && significant_improvements.is_empty() {
            let worst = significant_regressions
                .iter()
                .max_by(|a, b| a.cohens_d.abs().partial_cmp(&b.cohens_d.abs()).unwrap())
                .unwrap();
            Some(format!(
                "Treatment '{}' shows significant regression in {} ({:+.1}% change, p={:.4}). Consider keeping control.",
                worst.treatment_id,
                worst.metric,
                worst.relative_change * 100.0,
                worst.p_value
            ))
        } else if !significant_improvements.is_empty() && !significant_regressions.is_empty() {
            Some("Mixed results: some metrics improved while others regressed. Review individual metrics before deciding.".to_string())
        } else {
            Some("No statistically significant differences detected. Consider collecting more data or ending the experiment.".to_string())
        }
    }
}
