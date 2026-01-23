//! Outcome tracking for A/B experiments
//!
//! This module provides:
//! - ExperimentOutcome: Single outcome record
//! - MetricStats: Running statistics for a metric
//! - VariantStats: Aggregated statistics per variant
//! - OutcomeTracker: Thread-safe outcome storage

use super::types::{ExperimentId, TrackedMetric, VariantId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use std::time::SystemTime;

// ============================================================================
// Experiment Outcome
// ============================================================================

/// Single outcome record for an experiment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentOutcome {
    /// The experiment this outcome belongs to
    pub experiment_id: ExperimentId,
    /// The variant that was used
    pub variant_id: VariantId,
    /// When the outcome was recorded
    pub timestamp: SystemTime,
    /// Metric values collected
    pub metrics: HashMap<TrackedMetric, f64>,
    /// Request ID for correlation
    pub request_id: String,
    /// Model that was actually used
    pub model_used: String,
}

impl ExperimentOutcome {
    /// Create a new outcome record
    pub fn new(
        experiment_id: impl Into<String>,
        variant_id: impl Into<String>,
        request_id: impl Into<String>,
        model_used: impl Into<String>,
    ) -> Self {
        Self {
            experiment_id: experiment_id.into(),
            variant_id: variant_id.into(),
            timestamp: SystemTime::now(),
            metrics: HashMap::new(),
            request_id: request_id.into(),
            model_used: model_used.into(),
        }
    }

    /// Add a metric value
    pub fn with_metric(mut self, metric: TrackedMetric, value: f64) -> Self {
        self.metrics.insert(metric, value);
        self
    }

    /// Add latency metric
    pub fn with_latency_ms(self, latency: u64) -> Self {
        self.with_metric(TrackedMetric::LatencyMs, latency as f64)
    }

    /// Add cost metric
    pub fn with_cost_usd(self, cost: f64) -> Self {
        self.with_metric(TrackedMetric::CostUsd, cost)
    }

    /// Add success metric
    pub fn with_success(self, success: bool) -> Self {
        self.with_metric(TrackedMetric::SuccessRate, if success { 1.0 } else { 0.0 })
    }

    /// Add token counts
    pub fn with_tokens(self, input: u32, output: u32) -> Self {
        self.with_metric(TrackedMetric::InputTokens, input as f64)
            .with_metric(TrackedMetric::OutputTokens, output as f64)
    }
}

// ============================================================================
// Metric Statistics
// ============================================================================

/// Running statistics for a single metric (online algorithm)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricStats {
    /// Number of observations
    pub count: u64,
    /// Sum of all values
    pub sum: f64,
    /// Sum of squared values (for variance calculation)
    pub sum_sq: f64,
    /// Minimum observed value
    pub min: f64,
    /// Maximum observed value
    pub max: f64,
}

impl MetricStats {
    /// Create new empty stats
    pub fn new() -> Self {
        Self {
            count: 0,
            sum: 0.0,
            sum_sq: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    /// Record a new observation
    pub fn record(&mut self, value: f64) {
        self.count += 1;
        self.sum += value;
        self.sum_sq += value * value;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    /// Calculate the mean
    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }

    /// Calculate the variance (population variance)
    pub fn variance(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            let mean = self.mean();
            (self.sum_sq / self.count as f64) - (mean * mean)
        }
    }

    /// Calculate the standard deviation
    pub fn std_dev(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Calculate the standard error of the mean
    pub fn std_error(&self) -> f64 {
        if self.count <= 1 {
            0.0
        } else {
            self.std_dev() / (self.count as f64).sqrt()
        }
    }
}

// ============================================================================
// Variant Statistics
// ============================================================================

/// Aggregated statistics per variant
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VariantStats {
    /// Total sample count
    pub sample_count: u64,
    /// Per-metric statistics
    pub metrics: HashMap<TrackedMetric, MetricStats>,
}

impl VariantStats {
    /// Create new empty stats
    pub fn new() -> Self {
        Self {
            sample_count: 0,
            metrics: HashMap::new(),
        }
    }

    /// Record an outcome
    pub fn record(&mut self, outcome: &ExperimentOutcome) {
        self.sample_count += 1;
        for (metric, value) in &outcome.metrics {
            self.metrics
                .entry(metric.clone())
                .or_default()
                .record(*value);
        }
    }

    /// Get stats for a specific metric
    pub fn get_metric(&self, metric: &TrackedMetric) -> Option<&MetricStats> {
        self.metrics.get(metric)
    }
}

// ============================================================================
// Outcome Tracker
// ============================================================================

/// Thread-safe outcome tracker
pub struct OutcomeTracker {
    /// Per-experiment, per-variant statistics
    stats: RwLock<HashMap<ExperimentId, HashMap<VariantId, VariantStats>>>,
    /// Raw outcomes for detailed analysis (bounded buffer)
    raw_outcomes: RwLock<VecDeque<ExperimentOutcome>>,
    /// Maximum raw outcomes to retain
    max_raw_outcomes: usize,
}

impl OutcomeTracker {
    /// Create a new outcome tracker
    pub fn new(max_raw_outcomes: usize) -> Self {
        Self {
            stats: RwLock::new(HashMap::new()),
            raw_outcomes: RwLock::new(VecDeque::new()),
            max_raw_outcomes,
        }
    }

    /// Record an outcome
    pub fn record(&self, outcome: ExperimentOutcome) {
        // Update aggregated stats
        {
            let mut stats = self.stats.write().unwrap();
            let experiment_stats = stats.entry(outcome.experiment_id.clone()).or_default();
            let variant_stats = experiment_stats
                .entry(outcome.variant_id.clone())
                .or_default();
            variant_stats.record(&outcome);
        }

        // Add to raw outcomes with eviction
        {
            let mut raw = self.raw_outcomes.write().unwrap();
            raw.push_back(outcome);
            while raw.len() > self.max_raw_outcomes {
                raw.pop_front();
            }
        }
    }

    /// Get statistics for an experiment
    pub fn get_stats(&self, experiment_id: &str) -> Option<HashMap<VariantId, VariantStats>> {
        let stats = self.stats.read().unwrap();
        stats.get(experiment_id).cloned()
    }

    /// Get statistics for a specific variant
    pub fn get_variant_stats(&self, experiment_id: &str, variant_id: &str) -> Option<VariantStats> {
        let stats = self.stats.read().unwrap();
        stats
            .get(experiment_id)
            .and_then(|e| e.get(variant_id))
            .cloned()
    }

    /// Get all statistics
    pub fn get_all_stats(&self) -> HashMap<ExperimentId, HashMap<VariantId, VariantStats>> {
        self.stats.read().unwrap().clone()
    }

    /// Get raw outcomes for an experiment (most recent first)
    pub fn get_raw_outcomes(&self, experiment_id: &str) -> Vec<ExperimentOutcome> {
        let raw = self.raw_outcomes.read().unwrap();
        raw.iter()
            .filter(|o| o.experiment_id == experiment_id)
            .cloned()
            .collect()
    }

    /// Get total outcome count for an experiment
    pub fn get_total_count(&self, experiment_id: &str) -> u64 {
        let stats = self.stats.read().unwrap();
        stats
            .get(experiment_id)
            .map(|e| e.values().map(|v| v.sample_count).sum())
            .unwrap_or(0)
    }

    /// Clear statistics for an experiment
    pub fn clear_experiment(&self, experiment_id: &str) {
        {
            let mut stats = self.stats.write().unwrap();
            stats.remove(experiment_id);
        }
        {
            let mut raw = self.raw_outcomes.write().unwrap();
            raw.retain(|o| o.experiment_id != experiment_id);
        }
    }

    /// Clear all statistics
    pub fn clear_all(&self) {
        {
            let mut stats = self.stats.write().unwrap();
            stats.clear();
        }
        {
            let mut raw = self.raw_outcomes.write().unwrap();
            raw.clear();
        }
    }
}

impl Default for OutcomeTracker {
    fn default() -> Self {
        Self::new(100_000)
    }
}
