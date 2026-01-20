//! Dynamic Scoring for Model Router
//!
//! This module provides scoring algorithms to rank models based on
//! runtime metrics for intelligent routing decisions.

use super::metrics::{ModelMetrics, MultiWindowMetrics};
use super::profiles::{CostTier, LatencyTier, ModelProfile};
use super::TaskIntent;
use serde::{Deserialize, Serialize};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for dynamic scoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringConfig {
    /// Weight for latency score (0.0 - 1.0)
    pub latency_weight: f64,

    /// Weight for cost score (0.0 - 1.0)
    pub cost_weight: f64,

    /// Weight for reliability score (0.0 - 1.0)
    pub reliability_weight: f64,

    /// Weight for quality score (0.0 - 1.0)
    pub quality_weight: f64,

    /// Latency target in ms (below this = full score)
    pub latency_target_ms: f64,

    /// Latency maximum in ms (above this = zero score)
    pub latency_max_ms: f64,

    /// Minimum acceptable success rate
    pub min_success_rate: f64,

    /// Consecutive failures to trigger degradation penalty
    pub degradation_threshold: u32,

    /// Minimum samples for reliable scoring
    pub min_samples: u64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            latency_weight: 0.25,
            cost_weight: 0.25,
            reliability_weight: 0.35,
            quality_weight: 0.15,
            latency_target_ms: 2000.0,
            latency_max_ms: 30000.0,
            min_success_rate: 0.9,
            degradation_threshold: 3,
            min_samples: 10,
        }
    }
}

impl ScoringConfig {
    /// Validate and normalize weights to sum to 1.0
    pub fn normalize(&mut self) {
        let sum = self.latency_weight
            + self.cost_weight
            + self.reliability_weight
            + self.quality_weight;

        if sum > 0.0 {
            self.latency_weight /= sum;
            self.cost_weight /= sum;
            self.reliability_weight /= sum;
            self.quality_weight /= sum;
        } else {
            // Reset to defaults if all zero
            *self = Self::default();
        }
    }

    /// Check if config is valid
    pub fn is_valid(&self) -> bool {
        self.latency_weight >= 0.0
            && self.cost_weight >= 0.0
            && self.reliability_weight >= 0.0
            && self.quality_weight >= 0.0
            && self.latency_target_ms > 0.0
            && self.latency_max_ms > self.latency_target_ms
            && self.min_success_rate >= 0.0
            && self.min_success_rate <= 1.0
    }
}

// =============================================================================
// Score Result
// =============================================================================

/// Detailed scoring result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreResult {
    /// Model ID
    pub model_id: String,

    /// Final weighted score (0.0 - 1.0)
    pub final_score: f64,

    /// Latency component score
    pub latency_score: f64,

    /// Cost component score
    pub cost_score: f64,

    /// Reliability component score
    pub reliability_score: f64,

    /// Quality component score
    pub quality_score: f64,

    /// Penalty factor applied
    pub penalty_factor: f64,

    /// Whether static scoring was used (insufficient data)
    pub is_static: bool,

    /// Reason for score (for debugging)
    pub reason: Option<String>,
}

impl ScoreResult {
    /// Create a new score result
    pub fn new(model_id: impl Into<String>, final_score: f64) -> Self {
        Self {
            model_id: model_id.into(),
            final_score,
            latency_score: 0.0,
            cost_score: 0.0,
            reliability_score: 0.0,
            quality_score: 0.0,
            penalty_factor: 1.0,
            is_static: false,
            reason: None,
        }
    }

    /// Create a static score result
    pub fn static_score(model_id: impl Into<String>, score: f64, reason: &str) -> Self {
        Self {
            model_id: model_id.into(),
            final_score: score,
            latency_score: score,
            cost_score: score,
            reliability_score: score,
            quality_score: score,
            penalty_factor: 1.0,
            is_static: true,
            reason: Some(reason.to_string()),
        }
    }
}

// =============================================================================
// Dynamic Scorer
// =============================================================================

/// Dynamic model scorer based on runtime metrics
pub struct DynamicScorer {
    config: ScoringConfig,
}

impl DynamicScorer {
    /// Create a new scorer with given config
    pub fn new(config: ScoringConfig) -> Self {
        let mut config = config;
        config.normalize();
        Self { config }
    }

    /// Create with default config
    pub fn with_defaults() -> Self {
        Self::new(ScoringConfig::default())
    }

    /// Get current config
    pub fn config(&self) -> &ScoringConfig {
        &self.config
    }

    /// Compute score for a model
    ///
    /// Returns a score between 0.0 and 1.0, where higher is better.
    pub fn score(
        &self,
        profile: &ModelProfile,
        metrics: Option<&MultiWindowMetrics>,
        intent: &TaskIntent,
    ) -> f64 {
        self.score_detailed(profile, metrics, intent).final_score
    }

    /// Compute detailed score for a model
    pub fn score_detailed(
        &self,
        profile: &ModelProfile,
        metrics: Option<&MultiWindowMetrics>,
        intent: &TaskIntent,
    ) -> ScoreResult {
        // Check if we have sufficient data
        let has_data = metrics
            .map(|m| m.medium_term.total_calls >= self.config.min_samples)
            .unwrap_or(false);

        if !has_data {
            return self.score_static(profile, intent);
        }

        let metrics = metrics.unwrap();
        let m = &metrics.medium_term; // Primary window for scoring

        // Compute component scores
        let latency_score = self.compute_latency_score(m);
        let cost_score = self.compute_cost_score(profile, m);
        let reliability_score = self.compute_reliability_score(m);
        let quality_score = self.compute_quality_score(m, intent);

        // Compute penalty factor
        let penalty = self.compute_penalty(m, &metrics.short_term);

        // Weighted sum
        let raw_score = self.config.latency_weight * latency_score
            + self.config.cost_weight * cost_score
            + self.config.reliability_weight * reliability_score
            + self.config.quality_weight * quality_score;

        let final_score = (raw_score * penalty).clamp(0.0, 1.0);

        ScoreResult {
            model_id: profile.id.clone(),
            final_score,
            latency_score,
            cost_score,
            reliability_score,
            quality_score,
            penalty_factor: penalty,
            is_static: false,
            reason: None,
        }
    }

    /// Static scoring for models with insufficient data
    fn score_static(&self, profile: &ModelProfile, intent: &TaskIntent) -> ScoreResult {
        let mut score: f64 = 0.5; // Base score

        // Capability match bonus
        if let Some(required) = intent.required_capability() {
            if profile.has_capability(required) {
                score += 0.2;
            }
        }

        // Cost adjustment
        score += match profile.cost_tier {
            CostTier::Free => 0.15,
            CostTier::Low => 0.1,
            CostTier::Medium => 0.0,
            CostTier::High => -0.1,
        };

        // Latency adjustment
        score += match profile.latency_tier {
            LatencyTier::Fast => 0.1,
            LatencyTier::Medium => 0.0,
            LatencyTier::Slow => -0.1,
        };

        let final_score = score.clamp(0.3, 0.8);

        ScoreResult::static_score(
            &profile.id,
            final_score,
            "Insufficient data, using static scoring",
        )
    }

    /// Compute latency score based on P95 latency
    fn compute_latency_score(&self, metrics: &ModelMetrics) -> f64 {
        let p95 = metrics.latency.p95;

        if p95 <= self.config.latency_target_ms {
            1.0
        } else if p95 >= self.config.latency_max_ms {
            0.0
        } else {
            // Linear interpolation between target and max
            1.0 - (p95 - self.config.latency_target_ms)
                / (self.config.latency_max_ms - self.config.latency_target_ms)
        }
    }

    /// Compute cost score based on actual and expected cost
    fn compute_cost_score(&self, profile: &ModelProfile, metrics: &ModelMetrics) -> f64 {
        // If we have actual cost data, use it
        if let Some(actual_price) = metrics.cost.actual_input_price {
            let expected = profile.cost_tier.cost_multiplier();
            if expected <= 0.0 {
                return 1.0; // Free tier
            }

            let ratio = actual_price / expected;

            // Better than expected = high score
            if ratio <= 1.0 {
                1.0
            } else {
                // Worse than expected = penalize
                (2.0 - ratio).max(0.0)
            }
        } else {
            // Fall back to static cost tier scoring
            match profile.cost_tier {
                CostTier::Free => 1.0,
                CostTier::Low => 0.8,
                CostTier::Medium => 0.5,
                CostTier::High => 0.2,
            }
        }
    }

    /// Compute reliability score based on success rate
    fn compute_reliability_score(&self, metrics: &ModelMetrics) -> f64 {
        let success_rate = metrics.success_rate;

        if success_rate >= 0.99 {
            1.0
        } else if success_rate >= self.config.min_success_rate {
            // Linear from min_success_rate to 0.99
            (success_rate - self.config.min_success_rate) / (0.99 - self.config.min_success_rate)
        } else {
            // Below minimum = zero score
            0.0
        }
    }

    /// Compute quality score based on user feedback
    fn compute_quality_score(&self, metrics: &ModelMetrics, intent: &TaskIntent) -> f64 {
        // Try intent-specific score first
        let intent_key = intent.to_task_type();
        if let Some(intent_metrics) = metrics.intent_performance.get(intent_key) {
            if let Some(score) = intent_metrics.satisfaction_score {
                return score;
            }
        }

        // Fall back to overall satisfaction score
        metrics.satisfaction_score.unwrap_or(0.5)
    }

    /// Compute penalty factor for recent issues
    fn compute_penalty(&self, medium: &ModelMetrics, short: &ModelMetrics) -> f64 {
        let mut penalty = 1.0;

        // Consecutive failures penalty
        if medium.consecutive_failures >= self.config.degradation_threshold {
            penalty *= 0.1; // Heavy penalty
        } else if medium.consecutive_failures > 0 {
            // Gradual penalty
            penalty *= 1.0 - (medium.consecutive_failures as f64 * 0.15);
        }

        // Short-term success rate drop (detect sudden issues)
        if short.total_calls >= 5 {
            let rate_drop = medium.success_rate - short.success_rate;
            if rate_drop > 0.2 {
                // >20% drop in short term
                penalty *= 0.5;
            } else if rate_drop > 0.1 {
                penalty *= 0.75;
            }
        }

        // Rate limit penalty
        if let Some(rate_limit) = &medium.rate_limit {
            if rate_limit.is_limited() {
                penalty *= 0.0; // Completely exclude
            } else if rate_limit.remaining_capacity() < 0.2 {
                penalty *= 0.5;
            }
        }

        penalty.clamp(0.0, 1.0)
    }

    /// Score multiple models and return sorted results
    pub fn score_all(
        &self,
        profiles: &[ModelProfile],
        all_metrics: &std::collections::HashMap<String, MultiWindowMetrics>,
        intent: &TaskIntent,
    ) -> Vec<ScoreResult> {
        let mut results: Vec<_> = profiles
            .iter()
            .map(|p| {
                let metrics = all_metrics.get(&p.id);
                self.score_detailed(p, metrics, intent)
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results
    }
}

impl Default for DynamicScorer {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::Capability;

    fn create_test_profile(id: &str, cost: CostTier, latency: LatencyTier) -> ModelProfile {
        ModelProfile::new(id, "test", id)
            .with_cost_tier(cost)
            .with_latency_tier(latency)
    }

    fn create_test_metrics(model_id: &str, success_rate: f64, latency_p95: f64) -> MultiWindowMetrics {
        let mut metrics = MultiWindowMetrics::new(model_id);

        // Simulate calls to set metrics
        let total_calls = 100;
        let successes = (total_calls as f64 * success_rate) as u64;

        metrics.medium_term.total_calls = total_calls;
        metrics.medium_term.successful_calls = successes;
        metrics.medium_term.success_rate = success_rate;
        metrics.medium_term.latency.p95 = latency_p95;
        metrics.medium_term.latency.count = total_calls;

        metrics
    }

    #[test]
    fn test_scoring_config_default() {
        let config = ScoringConfig::default();
        assert!(config.is_valid());

        let sum = config.latency_weight
            + config.cost_weight
            + config.reliability_weight
            + config.quality_weight;
        assert!((sum - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_scoring_config_normalize() {
        let mut config = ScoringConfig {
            latency_weight: 1.0,
            cost_weight: 1.0,
            reliability_weight: 1.0,
            quality_weight: 1.0,
            ..Default::default()
        };

        config.normalize();

        let sum = config.latency_weight
            + config.cost_weight
            + config.reliability_weight
            + config.quality_weight;
        assert!((sum - 1.0).abs() < 0.01);
        assert!((config.latency_weight - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_static_scoring_capability_match() {
        let scorer = DynamicScorer::with_defaults();

        let mut profile = create_test_profile("model-a", CostTier::Medium, LatencyTier::Medium);
        profile.capabilities = vec![Capability::CodeGeneration];

        let score_match =
            scorer.score(&profile, None, &TaskIntent::CodeGeneration);
        let score_no_match = scorer.score(&profile, None, &TaskIntent::TextAnalysis);

        assert!(score_match > score_no_match);
    }

    #[test]
    fn test_static_scoring_cost_preference() {
        let scorer = DynamicScorer::with_defaults();

        let cheap = create_test_profile("cheap", CostTier::Low, LatencyTier::Medium);
        let expensive = create_test_profile("expensive", CostTier::High, LatencyTier::Medium);

        let cheap_score = scorer.score(&cheap, None, &TaskIntent::GeneralChat);
        let expensive_score = scorer.score(&expensive, None, &TaskIntent::GeneralChat);

        assert!(cheap_score > expensive_score);
    }

    #[test]
    fn test_dynamic_scoring_latency() {
        let scorer = DynamicScorer::with_defaults();

        let profile = create_test_profile("model", CostTier::Medium, LatencyTier::Medium);

        // Fast model (under target)
        let fast_metrics = create_test_metrics("model", 0.99, 1000.0);
        let fast_score = scorer.score(&profile, Some(&fast_metrics), &TaskIntent::GeneralChat);

        // Slow model (over target)
        let slow_metrics = create_test_metrics("model", 0.99, 15000.0);
        let slow_score = scorer.score(&profile, Some(&slow_metrics), &TaskIntent::GeneralChat);

        assert!(fast_score > slow_score);
    }

    #[test]
    fn test_dynamic_scoring_reliability() {
        let scorer = DynamicScorer::with_defaults();

        let profile = create_test_profile("model", CostTier::Medium, LatencyTier::Medium);

        // Reliable model
        let reliable_metrics = create_test_metrics("model", 0.99, 2000.0);
        let reliable_score =
            scorer.score(&profile, Some(&reliable_metrics), &TaskIntent::GeneralChat);

        // Unreliable model
        let unreliable_metrics = create_test_metrics("model", 0.80, 2000.0);
        let unreliable_score =
            scorer.score(&profile, Some(&unreliable_metrics), &TaskIntent::GeneralChat);

        assert!(reliable_score > unreliable_score);
    }

    #[test]
    fn test_penalty_consecutive_failures() {
        let scorer = DynamicScorer::with_defaults();

        let profile = create_test_profile("model", CostTier::Medium, LatencyTier::Medium);

        // No failures
        let mut good_metrics = create_test_metrics("model", 0.99, 2000.0);
        good_metrics.medium_term.consecutive_failures = 0;
        let good_score = scorer.score(&profile, Some(&good_metrics), &TaskIntent::GeneralChat);

        // Many failures
        let mut bad_metrics = create_test_metrics("model", 0.99, 2000.0);
        bad_metrics.medium_term.consecutive_failures = 5;
        let bad_score = scorer.score(&profile, Some(&bad_metrics), &TaskIntent::GeneralChat);

        assert!(good_score > bad_score);
        assert!(bad_score < 0.2); // Heavy penalty
    }

    #[test]
    fn test_score_detailed() {
        let scorer = DynamicScorer::with_defaults();

        let profile = create_test_profile("model", CostTier::Low, LatencyTier::Fast);
        let metrics = create_test_metrics("model", 0.95, 1500.0);

        let result = scorer.score_detailed(&profile, Some(&metrics), &TaskIntent::GeneralChat);

        assert!(!result.is_static);
        assert!(result.final_score > 0.0);
        assert!(result.latency_score > 0.5);
        assert!(result.reliability_score > 0.0);
        assert_eq!(result.model_id, "model");
    }

    #[test]
    fn test_score_all() {
        let scorer = DynamicScorer::with_defaults();

        let profiles = vec![
            create_test_profile("fast", CostTier::High, LatencyTier::Fast),
            create_test_profile("cheap", CostTier::Low, LatencyTier::Slow),
            create_test_profile("balanced", CostTier::Medium, LatencyTier::Medium),
        ];

        let mut all_metrics = std::collections::HashMap::new();
        all_metrics.insert("fast".to_string(), create_test_metrics("fast", 0.99, 500.0));
        all_metrics.insert("cheap".to_string(), create_test_metrics("cheap", 0.95, 5000.0));
        all_metrics.insert(
            "balanced".to_string(),
            create_test_metrics("balanced", 0.97, 2000.0),
        );

        let results = scorer.score_all(&profiles, &all_metrics, &TaskIntent::GeneralChat);

        // Should be sorted by score descending
        assert_eq!(results.len(), 3);
        assert!(results[0].final_score >= results[1].final_score);
        assert!(results[1].final_score >= results[2].final_score);
    }

    #[test]
    fn test_score_insufficient_data() {
        let scorer = DynamicScorer::with_defaults();

        let profile = create_test_profile("model", CostTier::Medium, LatencyTier::Medium);

        // Too few samples
        let mut sparse_metrics = create_test_metrics("model", 0.99, 1000.0);
        sparse_metrics.medium_term.total_calls = 5; // Below min_samples

        let result =
            scorer.score_detailed(&profile, Some(&sparse_metrics), &TaskIntent::GeneralChat);

        assert!(result.is_static);
        assert!(result.reason.is_some());
    }

    #[test]
    fn test_latency_score_boundaries() {
        let scorer = DynamicScorer::with_defaults();
        let profile = create_test_profile("model", CostTier::Medium, LatencyTier::Medium);

        // At target = full score
        let at_target = create_test_metrics("model", 0.99, scorer.config.latency_target_ms);
        let result = scorer.score_detailed(&profile, Some(&at_target), &TaskIntent::GeneralChat);
        assert!((result.latency_score - 1.0).abs() < 0.01);

        // At max = zero score
        let at_max = create_test_metrics("model", 0.99, scorer.config.latency_max_ms);
        let result = scorer.score_detailed(&profile, Some(&at_max), &TaskIntent::GeneralChat);
        assert!(result.latency_score < 0.01);
    }

    #[test]
    fn test_quality_score_with_feedback() {
        let scorer = DynamicScorer::with_defaults();
        let profile = create_test_profile("model", CostTier::Medium, LatencyTier::Medium);

        let mut metrics = create_test_metrics("model", 0.99, 2000.0);
        metrics.medium_term.satisfaction_score = Some(0.9);

        let result = scorer.score_detailed(&profile, Some(&metrics), &TaskIntent::GeneralChat);
        assert!(result.quality_score > 0.5);
    }
}
