//! Multi-Model Ensemble Engine for Model Router
//!
//! This module provides ensemble execution capabilities, enabling higher reliability
//! and quality through parallel model execution, response aggregation, and consensus
//! detection for critical tasks.
//!
//! # Architecture
//!
//! ```text
//! Request (high complexity or specific intent)
//!   │
//!   ▼
//! ┌─────────────────────────┐
//! │  EnsembleEngine         │ ◀── EnsembleStrategy
//! │  (select models)        │
//! └─────────────────────────┘
//!   │
//!   ▼
//! ┌─────────────────────────┐
//! │  ParallelExecutor       │ ◀── Execute concurrently
//! │  (tokio::join_all)      │
//! └─────────────────────────┘
//!   │
//!   ▼
//! ┌─────────────────────────┐
//! │  ResponseAggregator     │ ◀── Combine results
//! │  (best_of_n/voting/etc) │
//! └─────────────────────────┘
//!   │
//!   ▼
//! EnsembleResult (with confidence + metadata)
//! ```
//!
//! # Module Structure
//!
//! - `types`: Core types (EnsembleMode, QualityMetric, EnsembleConfig, etc.)
//! - `scorers`: Quality scoring implementations
//! - `execution`: ParallelExecutor for concurrent model execution
//! - `aggregation`: ResponseAggregator and EnsembleResult
//! - `engine`: Main EnsembleEngine integration point
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::dispatcher::model_router::ensemble::*;
//!
//! // Configure ensemble for reasoning tasks
//! let config = EnsembleConfig::new(EnsembleMode::BestOfN { n: 2 })
//!     .with_models(vec!["claude-opus", "gpt-4o"])
//!     .with_timeout_ms(30000)
//!     .with_quality_metric(QualityMetric::LengthAndStructure);
//!
//! // Execute ensemble
//! let executor = ParallelExecutor::new(Duration::from_millis(30000));
//! let results = executor.execute_parallel(&models, &request, |m, r| async {
//!     // Call model API
//! }).await;
//! ```

pub mod aggregation;
pub mod engine;
pub mod execution;
pub mod scorers;
pub mod types;

// Re-export all public types for backward compatibility
pub use aggregation::{jaccard_similarity, EnsembleResult, ResponseAggregator};
pub use engine::{
    EnsembleDecision, EnsembleEngine, EnsembleEngineConfig, EnsembleExecutionError,
    EnsembleRequest,
};
pub use execution::ParallelExecutor;
pub use scorers::{
    create_scorer, ConfidenceMarkersScorer, LengthAndStructureScorer, LengthScorer, QualityScorer,
    RelevanceScorer, StructureScorer,
};
pub use types::{
    EnsembleConfig, EnsembleMode, EnsembleStrategy, EnsembleValidationError, ModelExecutionResult,
    QualityMetric, TokenUsage,
};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::TaskIntent;
    use std::time::Duration;

    #[test]
    fn test_ensemble_mode_min_models() {
        assert_eq!(EnsembleMode::Disabled.min_models(), 1);
        assert_eq!(EnsembleMode::BestOfN { n: 3 }.min_models(), 3);
        assert_eq!(EnsembleMode::Voting.min_models(), 3);
        assert_eq!(
            EnsembleMode::Consensus { min_agreement: 0.7 }.min_models(),
            2
        );
        assert_eq!(
            EnsembleMode::Cascade {
                quality_threshold: 0.8
            }
            .min_models(),
            2
        );
    }

    #[test]
    fn test_ensemble_config_validation() {
        // Valid config
        let valid = EnsembleConfig::best_of_n(2).with_models(vec!["model-a", "model-b"]);
        assert!(valid.validate().is_ok());

        // Invalid: not enough models
        let insufficient = EnsembleConfig::best_of_n(3).with_models(vec!["model-a", "model-b"]);
        assert!(matches!(
            insufficient.validate(),
            Err(EnsembleValidationError::InsufficientModels { .. })
        ));

        // Invalid: duplicate models
        let duplicates = EnsembleConfig::best_of_n(2).with_models(vec!["model-a", "model-a"]);
        assert!(matches!(
            duplicates.validate(),
            Err(EnsembleValidationError::DuplicateModels)
        ));

        // Invalid: zero timeout
        let mut zero_timeout = EnsembleConfig::best_of_n(2).with_models(vec!["model-a", "model-b"]);
        zero_timeout.timeout_ms = 0;
        assert!(matches!(
            zero_timeout.validate(),
            Err(EnsembleValidationError::ZeroTimeout)
        ));
    }

    #[test]
    fn test_ensemble_strategy_lookup() {
        let strategy = EnsembleStrategy::new()
            .with_default_mode(EnsembleMode::Disabled)
            .add_intent_strategy(
                TaskIntent::Reasoning,
                EnsembleConfig::best_of_n(2).with_models(vec!["a", "b"]),
            )
            .with_complexity_threshold(0.8)
            .with_high_complexity_config(
                EnsembleConfig::best_of_n(3).with_models(vec!["x", "y", "z"]),
            );

        // Intent-specific lookup
        assert!(strategy.get_config(&TaskIntent::Reasoning, None).is_some());
        assert!(strategy
            .get_config(&TaskIntent::GeneralChat, None)
            .is_none());

        // High complexity overrides intent
        let config = strategy.get_config(&TaskIntent::GeneralChat, Some(0.9));
        assert!(config.is_some());
        assert_eq!(config.unwrap().models.len(), 3);

        // Should use ensemble
        assert!(strategy.should_use_ensemble(&TaskIntent::Reasoning, None));
        assert!(strategy.should_use_ensemble(&TaskIntent::GeneralChat, Some(0.9)));
        assert!(!strategy.should_use_ensemble(&TaskIntent::GeneralChat, Some(0.5)));
    }

    #[test]
    fn test_quality_scorers() {
        let response = r#"
## Summary

Here is the code:

```rust
fn main() {
    println!("Hello");
}
```

Key points:
- Point 1
- Point 2

I'm confident this is correct.
"#;

        // Length scorer
        let length_scorer = LengthScorer;
        let length_score = length_scorer.score(response, "");
        assert!(length_score > 0.0 && length_score <= 1.0);

        // Structure scorer
        let structure_scorer = StructureScorer;
        let structure_score = structure_scorer.score(response, "");
        assert!(structure_score > 0.5); // Has code, lists, headers

        // Combined scorer
        let combined_scorer = LengthAndStructureScorer::default();
        let combined_score = combined_scorer.score(response, "");
        assert!(combined_score > 0.3);

        // Confidence scorer
        let confidence_scorer = ConfidenceMarkersScorer;
        let confidence_score = confidence_scorer.score(response, "");
        assert!(confidence_score > 0.5); // Has "I'm confident"
    }

    #[test]
    fn test_jaccard_similarity() {
        let a = "The quick brown fox jumps over the lazy dog";
        let b = "The quick brown fox jumps over the lazy cat";

        let sim = jaccard_similarity(a, b);
        assert!(sim > 0.7 && sim < 1.0); // High but not perfect

        let c = "Completely different text here";
        let sim2 = jaccard_similarity(a, c);
        assert!(sim2 < 0.3); // Low similarity
    }

    #[test]
    fn test_model_execution_result() {
        let success = ModelExecutionResult::success("model-a", "Hello world", 100)
            .with_tokens(10, 5)
            .with_cost(0.001);

        assert!(success.success);
        assert!(success.has_response());
        assert_eq!(success.tokens.total(), 15);

        let failure = ModelExecutionResult::failure("model-b", "Connection error", 50);
        assert!(!failure.success);
        assert!(!failure.has_response());

        let timeout = ModelExecutionResult::timeout("model-c", 30000);
        assert!(!timeout.success);
        assert_eq!(timeout.error.as_deref(), Some("Timeout"));
    }

    #[test]
    fn test_response_aggregator_best_of_n() {
        let results = vec![
            ModelExecutionResult::success("model-a", "Short", 100),
            ModelExecutionResult::success(
                "model-b",
                "This is a longer response with more content and details",
                150,
            ),
            ModelExecutionResult::failure("model-c", "Error", 50),
        ];

        let aggregator = ResponseAggregator::new(&QualityMetric::Length);
        let result = aggregator.best_of_n(results, "prompt");

        assert!(result.is_success());
        assert_eq!(result.selected_model, "model-b"); // Longer response
        assert_eq!(result.successful_count, 2);
        assert_eq!(result.failed_count, 1);
        assert_eq!(result.aggregation_method, "best_of_n");
    }

    #[test]
    fn test_response_aggregator_consensus() {
        let results = vec![
            ModelExecutionResult::success(
                "model-a",
                "The answer is 42 because of the meaning of life",
                100,
            ),
            ModelExecutionResult::success(
                "model-b",
                "The answer is 42 due to the meaning of everything",
                120,
            ),
            ModelExecutionResult::success(
                "model-c",
                "Something completely different about cats",
                80,
            ),
        ];

        let aggregator = ResponseAggregator::new(&QualityMetric::LengthAndStructure)
            .with_consensus_threshold(0.5);
        let result = aggregator.consensus(results, "What is the answer?");

        assert!(result.is_success());
        assert!(result.consensus_level.is_some());
        // First two are similar, third is different
    }

    #[test]
    fn test_ensemble_result_from_single() {
        let single = ModelExecutionResult::success("model-a", "Response", 100)
            .with_tokens(10, 20)
            .with_cost(0.001)
            .with_quality_score(0.8);

        let result = EnsembleResult::from_single(single);

        assert!(result.is_success());
        assert_eq!(result.selected_model, "model-a");
        assert_eq!(result.confidence, 0.8);
        assert_eq!(result.successful_count, 1);
        assert_eq!(result.failed_count, 0);
    }

    #[tokio::test]
    async fn test_parallel_executor() {
        let executor = ParallelExecutor::new(Duration::from_secs(5)).with_max_concurrency(3);

        let models = vec![
            "model-a".to_string(),
            "model-b".to_string(),
            "model-c".to_string(),
        ];

        let results = executor
            .execute_parallel(&models, |model_id| async move {
                // Simulate model execution
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok((
                    format!("Response from {}", model_id),
                    TokenUsage::new(10, 20),
                    0.001,
                ))
            })
            .await;

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.success));
        assert!(results.iter().all(|r| r.has_response()));
    }

    #[tokio::test]
    async fn test_parallel_executor_timeout() {
        let executor = ParallelExecutor::new(Duration::from_millis(50));

        let models = vec!["slow-model".to_string()];

        let results = executor
            .execute_parallel(&models, |_model_id| async move {
                // Simulate slow model
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(("Response".to_string(), TokenUsage::default(), 0.0))
            })
            .await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert_eq!(results[0].error.as_deref(), Some("Timeout"));
    }

    #[tokio::test]
    async fn test_parallel_executor_partial_failure() {
        let executor = ParallelExecutor::new(Duration::from_secs(5));

        let models = vec!["good-model".to_string(), "bad-model".to_string()];

        let results = executor
            .execute_parallel(&models, |model_id| async move {
                if model_id == "bad-model" {
                    Err("Simulated error".to_string())
                } else {
                    Ok(("Success".to_string(), TokenUsage::default(), 0.0))
                }
            })
            .await;

        assert_eq!(results.len(), 2);

        let good = results.iter().find(|r| r.model_id == "good-model").unwrap();
        assert!(good.success);

        let bad = results.iter().find(|r| r.model_id == "bad-model").unwrap();
        assert!(!bad.success);
        assert!(bad.error.is_some());
    }

    #[test]
    fn test_quality_metric_parsing() {
        assert_eq!(QualityMetric::parse("length"), QualityMetric::Length);
        assert_eq!(QualityMetric::parse("STRUCTURE"), QualityMetric::Structure);
        assert_eq!(
            QualityMetric::parse("length_and_structure"),
            QualityMetric::LengthAndStructure
        );
        assert_eq!(
            QualityMetric::parse("custom_metric"),
            QualityMetric::Custom("custom_metric".to_string())
        );
    }

    #[test]
    fn test_token_usage() {
        let usage = TokenUsage::new(100, 200);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 200);
        assert_eq!(usage.total(), 300);
    }
}
