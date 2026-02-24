//! A/B Testing Framework for Model Router
//!
//! This module provides controlled experimentation capabilities for routing strategies,
//! enabling data-driven optimization of model selection through traffic splitting,
//! outcome tracking, and statistical analysis.
//!
//! # Architecture
//!
//! ```text
//! Request
//!   │
//!   ▼
//! ┌─────────────────────────┐
//! │  TrafficSplitManager    │ ◀── ExperimentConfigs
//! │  (consistent hashing)   │
//! └─────────────────────────┘
//!   │
//!   ▼
//! VariantAssignment (or None)
//!   │
//!   ▼
//! ┌─────────────────────────┐
//! │  OutcomeTracker         │ ◀── Record metrics
//! │  (aggregated stats)     │
//! └─────────────────────────┘
//!   │
//!   ▼
//! ExperimentReport (with significance tests)
//! ```
//!
//! # Module Structure
//!
//! - `types`: Core types (TrackedMetric, VariantConfig, ExperimentConfig, etc.)
//! - `traffic`: TrafficSplitManager for consistent traffic assignment
//! - `tracking`: OutcomeTracker for recording experiment results
//! - `analysis`: SignificanceCalculator and ExperimentReport
//! - `engine`: Main ABTestingEngine integration point
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::dispatcher::model_router::ab_testing::*;
//!
//! // Create experiment configuration
//! let experiment = ExperimentConfig::new("test-gemini-routing")
//!     .with_name("Gemini vs Claude for Reasoning")
//!     .with_traffic_percentage(10)
//!     .add_variant(VariantConfig::control("claude-sonnet"))
//!     .add_variant(VariantConfig::treatment("gemini-pro"));
//!
//! // Create A/B testing engine
//! let engine = ABTestingEngine::new(vec![experiment]);
//!
//! // Assign user to variant
//! if let Some(assignment) = engine.assign("user-123", None, &TaskIntent::Reasoning) {
//!     println!("User assigned to: {}", assignment.variant_name);
//! }
//! ```

pub mod analysis;
pub mod engine;
pub mod tracking;
pub mod traffic;
pub mod types;

// Re-export all public types for backward compatibility
pub use analysis::{
    ExperimentReport, ExperimentStatus, MetricSummary, SignificanceCalculator, SignificanceResult,
    VariantSummary,
};
pub use engine::ABTestingEngine;
pub use tracking::{ExperimentOutcome, MetricStats, OutcomeTracker, VariantStats};
pub use traffic::TrafficSplitManager;
pub use types::{
    AssignmentStrategy, ExperimentConfig, ExperimentId, ExperimentValidationError, TrackedMetric,
    VariantAssignment, VariantConfig, VariantId,
};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::TaskIntent;
    use std::collections::HashMap;

    #[test]
    fn test_experiment_config_validation() {
        // Valid config
        let valid = ExperimentConfig::new("test")
            .add_variant(VariantConfig::control("model-a"))
            .add_variant(VariantConfig::treatment("model-b"));
        assert!(valid.validate().is_ok());

        // Invalid: no variants
        let no_variants = ExperimentConfig::new("test");
        assert!(matches!(
            no_variants.validate(),
            Err(ExperimentValidationError::InsufficientVariants { .. })
        ));

        // Invalid: only one variant
        let one_variant =
            ExperimentConfig::new("test").add_variant(VariantConfig::control("model-a"));
        assert!(matches!(
            one_variant.validate(),
            Err(ExperimentValidationError::InsufficientVariants { .. })
        ));

        // Invalid: duplicate variant IDs
        let duplicate = ExperimentConfig::new("test")
            .add_variant(VariantConfig::new("same"))
            .add_variant(VariantConfig::new("same"));
        assert!(matches!(
            duplicate.validate(),
            Err(ExperimentValidationError::DuplicateVariantId { .. })
        ));
    }

    #[test]
    fn test_traffic_split_consistency() {
        let experiment = ExperimentConfig::new("test")
            .with_traffic_percentage(100) // 100% traffic for testing
            .add_variant(VariantConfig::control("model-a"))
            .add_variant(VariantConfig::treatment("model-b"));

        let manager = TrafficSplitManager::new(vec![experiment], AssignmentStrategy::UserId);

        // Same user_id should always get same variant
        let user_id = "user-123";
        let first_assignment = manager
            .assign(Some(user_id), None, "req-1", &TaskIntent::GeneralChat, None)
            .unwrap();

        for i in 0..100 {
            let assignment = manager
                .assign(
                    Some(user_id),
                    None,
                    &format!("req-{}", i),
                    &TaskIntent::GeneralChat,
                    None,
                )
                .unwrap();
            assert_eq!(first_assignment.variant_id, assignment.variant_id);
        }
    }

    #[test]
    fn test_traffic_percentage() {
        let experiment = ExperimentConfig::new("test")
            .with_traffic_percentage(10)
            .add_variant(VariantConfig::control("model-a"))
            .add_variant(VariantConfig::treatment("model-b"));

        let manager = TrafficSplitManager::new(vec![experiment], AssignmentStrategy::RequestId);

        let mut in_experiment = 0;
        let total = 10000;

        for i in 0..total {
            if manager
                .assign(
                    None,
                    None,
                    &format!("req-{}", i),
                    &TaskIntent::GeneralChat,
                    None,
                )
                .is_some()
            {
                in_experiment += 1;
            }
        }

        // Should be approximately 10% (within 2% tolerance = 8-12%)
        let percentage = (in_experiment as f64 / total as f64) * 100.0;
        assert!(
            (8.0..=12.0).contains(&percentage),
            "Expected ~10%, got {:.1}%",
            percentage
        );
    }

    #[test]
    fn test_weighted_variant_distribution() {
        let experiment = ExperimentConfig::new("test")
            .with_traffic_percentage(100)
            .add_variant(VariantConfig::new("a").with_weight(70))
            .add_variant(VariantConfig::new("b").with_weight(30));

        let manager = TrafficSplitManager::new(vec![experiment], AssignmentStrategy::RequestId);

        let mut counts = HashMap::new();
        let total = 10000;

        for i in 0..total {
            if let Some(assignment) = manager.assign(
                None,
                None,
                &format!("req-{}", i),
                &TaskIntent::GeneralChat,
                None,
            ) {
                *counts.entry(assignment.variant_id).or_insert(0) += 1;
            }
        }

        let a_pct = (*counts.get(&VariantId::new("a")).unwrap_or(&0) as f64 / total as f64) * 100.0;
        let b_pct = (*counts.get(&VariantId::new("b")).unwrap_or(&0) as f64 / total as f64) * 100.0;

        // Should be approximately 70/30 (within 5% tolerance)
        assert!(
            (65.0..=75.0).contains(&a_pct),
            "Expected ~70% for A, got {:.1}%",
            a_pct
        );
        assert!(
            (25.0..=35.0).contains(&b_pct),
            "Expected ~30% for B, got {:.1}%",
            b_pct
        );
    }

    #[test]
    fn test_intent_filtering() {
        let experiment = ExperimentConfig::new("test")
            .with_traffic_percentage(100)
            .with_target_intent(TaskIntent::CodeGeneration)
            .add_variant(VariantConfig::control("model-a"))
            .add_variant(VariantConfig::treatment("model-b"));

        let manager = TrafficSplitManager::new(vec![experiment], AssignmentStrategy::RequestId);

        // Should match CodeGeneration intent
        assert!(manager
            .assign(None, None, "req-1", &TaskIntent::CodeGeneration, None)
            .is_some());

        // Should not match other intents
        assert!(manager
            .assign(None, None, "req-2", &TaskIntent::GeneralChat, None)
            .is_none());
        assert!(manager
            .assign(None, None, "req-3", &TaskIntent::Reasoning, None)
            .is_none());
    }

    #[test]
    fn test_outcome_tracking() {
        let tracker = OutcomeTracker::new(100);

        let outcome = ExperimentOutcome::new("exp-1", "control", "req-1", "model-a")
            .with_latency_ms(150)
            .with_cost_usd(0.001)
            .with_success(true);

        tracker.record(outcome);

        let stats = tracker.get_stats("exp-1").unwrap();
        let control_stats = stats.get(&VariantId::new("control")).unwrap();

        assert_eq!(control_stats.sample_count, 1);
        assert_eq!(
            control_stats
                .get_metric(&TrackedMetric::LatencyMs)
                .unwrap()
                .mean(),
            150.0
        );
    }

    #[test]
    fn test_metric_stats_calculation() {
        let mut stats = MetricStats::new();
        stats.record(10.0);
        stats.record(20.0);
        stats.record(30.0);

        assert_eq!(stats.count, 3);
        assert_eq!(stats.mean(), 20.0);
        assert_eq!(stats.min, 10.0);
        assert_eq!(stats.max, 30.0);

        // Variance of [10, 20, 30] = ((10-20)² + (20-20)² + (30-20)²) / 3 = 200/3 ≈ 66.67
        let variance = stats.variance();
        assert!((variance - 66.666666).abs() < 0.01);
    }

    #[test]
    fn test_significance_calculator() {
        // Create two samples with known means
        let mut control = MetricStats::new();
        let mut treatment = MetricStats::new();

        // Control: mean ~100, low variance
        for v in [
            98.0, 99.0, 100.0, 101.0, 102.0, 99.0, 100.0, 101.0, 100.0, 99.0, 98.0, 99.0, 100.0,
            101.0, 102.0, 99.0, 100.0, 101.0, 100.0, 99.0, 98.0, 99.0, 100.0, 101.0, 102.0, 99.0,
            100.0, 101.0, 100.0, 99.0,
        ] {
            control.record(v);
        }

        // Treatment: mean ~110, similar variance (significant difference)
        for v in [
            108.0, 109.0, 110.0, 111.0, 112.0, 109.0, 110.0, 111.0, 110.0, 109.0, 108.0, 109.0,
            110.0, 111.0, 112.0, 109.0, 110.0, 111.0, 110.0, 109.0, 108.0, 109.0, 110.0, 111.0,
            112.0, 109.0, 110.0, 111.0, 110.0, 109.0,
        ] {
            treatment.record(v);
        }

        let result = SignificanceCalculator::t_test(
            TrackedMetric::LatencyMs,
            "control",
            &control,
            "treatment",
            &treatment,
        )
        .unwrap();

        // Should detect significant difference (10% increase)
        assert!(result.is_significant, "p-value: {}", result.p_value);
        assert!(result.relative_change > 0.05); // At least 5% change
    }

    #[test]
    fn test_ab_testing_engine_e2e() {
        let experiment = ExperimentConfig::new("test-exp")
            .with_name("Test Experiment")
            .with_traffic_percentage(100)
            .add_variant(VariantConfig::control("model-a"))
            .add_variant(VariantConfig::treatment("model-b"));

        let engine = ABTestingEngine::new(vec![experiment]);

        // Simulate some requests
        for i in 0..100 {
            let request_id = format!("req-{}", i);
            if let Some(assignment) =
                engine.assign(None, None, &request_id, &TaskIntent::GeneralChat, None)
            {
                let latency = if assignment.variant_id == VariantId::new("control") {
                    100.0 + (i as f64 % 20.0)
                } else {
                    90.0 + (i as f64 % 20.0)
                };

                let outcome = ExperimentOutcome::new(
                    assignment.experiment_id.as_str(),
                    assignment.variant_id.as_str(),
                    &request_id,
                    assignment.model_override.as_deref().unwrap_or("unknown"),
                )
                .with_latency_ms(latency as u64)
                .with_success(true);

                engine.record_outcome(outcome);
            }
        }

        // Get report
        let report = engine.get_report("test-exp").unwrap();

        assert_eq!(report.experiment_id, ExperimentId::new("test-exp"));
        assert_eq!(report.variant_summaries.len(), 2);
        assert!(report.total_samples > 0);
    }

    #[test]
    fn test_tracked_metric_parsing() {
        assert_eq!(TrackedMetric::parse("latency_ms"), TrackedMetric::LatencyMs);
        assert_eq!(TrackedMetric::parse("LATENCY"), TrackedMetric::LatencyMs);
        assert_eq!(TrackedMetric::parse("cost_usd"), TrackedMetric::CostUsd);
        assert_eq!(
            TrackedMetric::parse("custom_metric"),
            TrackedMetric::Custom("custom_metric".to_string())
        );
    }
}
