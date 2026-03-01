//! EnsembleEngine - Main integration point for ensemble execution
//!
//! This module provides:
//! - EnsembleEngineConfig: Engine configuration
//! - EnsembleRequest: Request for ensemble execution
//! - EnsembleDecision: Decision on whether to use ensemble
//! - EnsembleEngine: Main engine coordinating execution
//! - EnsembleExecutionError: Error types

use super::aggregation::{EnsembleResult, ResponseAggregator};
use super::execution::ParallelExecutor;
use super::scorers::create_scorer;
use super::types::{
    EnsembleConfig, EnsembleMode, EnsembleStrategy, ModelExecutionResult, QualityMetric,
    TokenUsage,
};
use crate::dispatcher::model_router::TaskIntent;
use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

// ============================================================================
// Engine Configuration
// ============================================================================

/// Configuration for the EnsembleEngine
#[derive(Debug, Clone)]
pub struct EnsembleEngineConfig {
    /// Default ensemble strategy with intent mappings
    pub strategy: EnsembleStrategy,
    /// Default timeout for parallel execution
    pub default_timeout: Duration,
    /// Default quality scorer
    pub default_scorer: QualityMetric,
    /// Maximum budget per ensemble call (optional)
    pub max_budget_usd: Option<f64>,
    /// Maximum number of concurrent model calls
    pub max_concurrency: usize,
}

impl Default for EnsembleEngineConfig {
    fn default() -> Self {
        Self {
            strategy: EnsembleStrategy::new(),
            default_timeout: Duration::from_secs(60),
            default_scorer: QualityMetric::LengthAndStructure,
            max_budget_usd: None,
            max_concurrency: 5,
        }
    }
}

impl EnsembleEngineConfig {
    /// Create a new configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the ensemble strategy
    pub fn with_strategy(mut self, strategy: EnsembleStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the default timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Set the default scorer
    pub fn with_scorer(mut self, scorer: QualityMetric) -> Self {
        self.default_scorer = scorer;
        self
    }

    /// Set budget limit
    pub fn with_budget_limit(mut self, limit_usd: f64) -> Self {
        self.max_budget_usd = Some(limit_usd);
        self
    }

    /// Set max concurrency
    pub fn with_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = max;
        self
    }
}

// ============================================================================
// Ensemble Request
// ============================================================================

/// Request for ensemble execution
#[derive(Debug, Clone)]
pub struct EnsembleRequest {
    /// The prompt to send to models
    pub prompt: String,
    /// Task intent for strategy selection
    pub intent: TaskIntent,
    /// Complexity score (0.0-1.0) for complexity-based triggering
    pub complexity: Option<f64>,
    /// Optional config override
    pub config_override: Option<EnsembleConfig>,
    /// Additional context/metadata
    pub context: HashMap<String, String>,
}

impl EnsembleRequest {
    /// Create a new request
    pub fn new(prompt: impl Into<String>, intent: TaskIntent) -> Self {
        Self {
            prompt: prompt.into(),
            intent,
            complexity: None,
            config_override: None,
            context: HashMap::new(),
        }
    }

    /// Set complexity score
    pub fn with_complexity(mut self, complexity: f64) -> Self {
        self.complexity = Some(complexity.clamp(0.0, 1.0));
        self
    }

    /// Override ensemble configuration
    pub fn with_config(mut self, config: EnsembleConfig) -> Self {
        self.config_override = Some(config);
        self
    }

    /// Add context metadata
    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
}

// ============================================================================
// Ensemble Decision
// ============================================================================

/// Ensemble execution decision
#[derive(Debug, Clone)]
pub struct EnsembleDecision {
    /// Whether to use ensemble execution
    pub should_ensemble: bool,
    /// Selected config (if ensembling)
    pub config: Option<EnsembleConfig>,
    /// Reason for the decision
    pub reason: String,
    /// Filtered models after budget check
    pub available_models: Vec<String>,
}

// ============================================================================
// Ensemble Engine
// ============================================================================

/// EnsembleEngine coordinates ensemble execution
///
/// This is the main integration point that combines:
/// - Strategy selection based on intent
/// - Complexity-based triggering
/// - Budget-aware model selection
/// - Parallel execution and response aggregation
pub struct EnsembleEngine {
    config: EnsembleEngineConfig,
    executor: ParallelExecutor,
    aggregator: ResponseAggregator,
}

impl EnsembleEngine {
    /// Create a new EnsembleEngine
    pub fn new(config: EnsembleEngineConfig) -> Self {
        // Create executor with configured timeout
        let mut executor = ParallelExecutor::new(config.default_timeout);
        executor.max_concurrency = config.max_concurrency;

        let aggregator = ResponseAggregator::new(&config.default_scorer);

        Self {
            config,
            executor,
            aggregator,
        }
    }

    /// Decide whether to use ensemble execution
    pub fn should_ensemble(&self, request: &EnsembleRequest) -> EnsembleDecision {
        // Check for config override
        if let Some(ref override_config) = request.config_override {
            return EnsembleDecision {
                should_ensemble: override_config.mode != EnsembleMode::Disabled,
                config: Some(override_config.clone()),
                reason: "Config override provided".to_string(),
                available_models: override_config.models.clone(),
            };
        }

        // Check intent-specific strategy
        if let Some(config) = self
            .config
            .strategy
            .get_config(&request.intent, request.complexity)
        {
            // Apply budget filter if set
            let available_models = if let Some(max_budget) = self.config.max_budget_usd {
                self.filter_models_by_budget(&config.models, max_budget)
            } else {
                config.models.clone()
            };

            let should_ensemble = config.mode != EnsembleMode::Disabled
                && available_models.len() >= config.mode.min_models();

            let reason = if let Some(complexity) = request.complexity {
                let threshold = self.config.strategy.complexity_threshold.unwrap_or(1.0);
                if complexity >= threshold {
                    format!(
                        "High complexity ({:.2}) triggered ensemble for {:?}",
                        complexity, request.intent
                    )
                } else {
                    format!("Intent {:?} has ensemble strategy", request.intent)
                }
            } else {
                format!("Intent {:?} has ensemble strategy", request.intent)
            };

            return EnsembleDecision {
                should_ensemble,
                config: Some(EnsembleConfig {
                    models: available_models.clone(),
                    ..config.clone()
                }),
                reason,
                available_models,
            };
        }

        // Default: no ensemble
        EnsembleDecision {
            should_ensemble: false,
            config: None,
            reason: format!("No ensemble strategy for {:?}", request.intent),
            available_models: vec![],
        }
    }

    /// Execute ensemble with given executor function
    ///
    /// The executor_fn takes a model ID, returns Result<(response, tokens, cost), error>
    pub async fn execute<F, Fut>(
        &self,
        request: &EnsembleRequest,
        executor_fn: F,
    ) -> Result<EnsembleResult, EnsembleExecutionError>
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(String, TokenUsage, f64), String>> + Send,
    {
        let decision = self.should_ensemble(request);

        if !decision.should_ensemble {
            return Err(EnsembleExecutionError::NotApplicable {
                reason: decision.reason,
            });
        }

        let config = decision.config.unwrap();

        // Validate we have enough models
        if decision.available_models.len() < config.mode.min_models() {
            return Err(EnsembleExecutionError::InsufficientModels {
                required: config.mode.min_models(),
                available: decision.available_models.len(),
            });
        }

        // Execute in parallel using the existing execute_parallel method
        let results = self
            .executor
            .execute_parallel(&decision.available_models, executor_fn)
            .await;

        // Check if all failed
        let successful_count = results.iter().filter(|r| r.response.is_some()).count();
        if successful_count == 0 {
            return Err(EnsembleExecutionError::AllModelsFailed {
                errors: results.iter().filter_map(|r| r.error.clone()).collect(),
            });
        }

        // Aggregate results
        let ensemble_result = self.aggregate(&config, &request.prompt, results);

        Ok(ensemble_result)
    }

    /// Execute cascade mode (run models in order until quality threshold met)
    pub async fn execute_cascade<F, Fut>(
        &self,
        models: &[String],
        prompt: &str,
        quality_threshold: f64,
        executor_fn: F,
    ) -> Result<EnsembleResult, EnsembleExecutionError>
    where
        F: Fn(String) -> Fut + Send + Sync,
        Fut: Future<Output = Result<(String, TokenUsage, f64), String>> + Send,
    {
        let mut all_results = Vec::new();
        let scorer = create_scorer(&self.config.default_scorer);

        for model_id in models {
            let start = Instant::now();
            let result_data = executor_fn(model_id.clone()).await;
            let latency_ms = start.elapsed().as_millis() as u64;

            let result = match result_data {
                Ok((response, tokens, cost)) => {
                    let mut r =
                        ModelExecutionResult::success(model_id.clone(), &response, latency_ms);
                    r.tokens = tokens;
                    r.cost_usd = cost;

                    // Check quality
                    let score = scorer.score(&response, prompt);
                    if score >= quality_threshold {
                        // Met threshold, return early
                        all_results.push(r.clone());
                        // Calculate stats before moving all_results
                        let total_cost: f64 = all_results.iter().map(|r| r.cost_usd).sum();
                        let total_latency: u64 = all_results.iter().map(|r| r.latency_ms).sum();
                        let successful_count =
                            all_results.iter().filter(|r| r.response.is_some()).count();
                        let failed_count =
                            all_results.iter().filter(|r| r.response.is_none()).count();
                        return Ok(EnsembleResult {
                            response,
                            selected_model: model_id.clone(),
                            confidence: score,
                            all_results,
                            aggregation_method: "cascade_early_exit".to_string(),
                            total_cost_usd: total_cost,
                            total_latency_ms: total_latency,
                            consensus_level: None,
                            successful_count,
                            failed_count,
                        });
                    }
                    r
                }
                Err(error) => ModelExecutionResult::failure(model_id.clone(), &error, latency_ms),
            };

            all_results.push(result);
        }

        // No model met threshold, return best response
        let successful: Vec<_> = all_results
            .iter()
            .filter(|r| r.response.is_some())
            .collect();
        if successful.is_empty() {
            return Err(EnsembleExecutionError::AllModelsFailed {
                errors: all_results.iter().filter_map(|r| r.error.clone()).collect(),
            });
        }

        // Find best by quality score
        let best = successful
            .iter()
            .max_by(|a, b| {
                let score_a = scorer.score(a.response.as_ref().unwrap(), prompt);
                let score_b = scorer.score(b.response.as_ref().unwrap(), prompt);
                score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        let best_score = scorer.score(best.response.as_ref().unwrap(), prompt);
        let total_cost: f64 = all_results.iter().map(|r| r.cost_usd).sum();
        let total_latency: u64 = all_results.iter().map(|r| r.latency_ms).sum();
        let successful_count = all_results.iter().filter(|r| r.response.is_some()).count();
        let failed_count = all_results.len() - successful_count;

        Ok(EnsembleResult {
            response: best.response.clone().unwrap(),
            selected_model: best.model_id.clone(),
            confidence: best_score,
            all_results,
            aggregation_method: "cascade_best_fallback".to_string(),
            total_cost_usd: total_cost,
            total_latency_ms: total_latency,
            consensus_level: None,
            successful_count,
            failed_count,
        })
    }

    /// Aggregate results based on ensemble mode
    fn aggregate(
        &self,
        config: &EnsembleConfig,
        prompt: &str,
        results: Vec<ModelExecutionResult>,
    ) -> EnsembleResult {
        match &config.mode {
            EnsembleMode::Disabled => {
                // Should not reach here, but return first successful
                let successful = results.iter().find(|r| r.response.is_some());
                if let Some(result) = successful {
                    EnsembleResult::from_single(result.clone())
                } else {
                    self.aggregator.best_of_n(results, prompt)
                }
            }
            EnsembleMode::BestOfN { n: _ } => self.aggregator.best_of_n(results, prompt),
            EnsembleMode::Voting => self.aggregator.voting(results, prompt),
            EnsembleMode::Consensus { min_agreement: _ } => {
                self.aggregator.consensus(results, prompt)
            }
            EnsembleMode::Cascade {
                quality_threshold: _,
            } => {
                // Cascade should use execute_cascade, but fallback to best_of_n
                self.aggregator.best_of_n(results, prompt)
            }
        }
    }

    /// Filter models by estimated cost against budget
    fn filter_models_by_budget(&self, models: &[String], max_budget: f64) -> Vec<String> {
        // Simple implementation: assume uniform cost and take as many as fit
        // In production, would query model pricing
        let estimated_cost_per_model = max_budget / (models.len() as f64).max(1.0);

        // If budget allows all models, return all
        if estimated_cost_per_model > 0.01 {
            // Assuming $0.01 is minimum viable cost
            models.to_vec()
        } else {
            // Budget too low, take only first model
            models.iter().take(1).cloned().collect()
        }
    }

    /// Get current configuration
    pub fn config(&self) -> &EnsembleEngineConfig {
        &self.config
    }

    /// Update configuration
    pub fn set_config(&mut self, config: EnsembleEngineConfig) {
        self.config = config;
    }

    /// Update strategy only
    pub fn set_strategy(&mut self, strategy: EnsembleStrategy) {
        self.config.strategy = strategy;
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Errors during ensemble execution
#[derive(Debug, Clone, thiserror::Error)]
pub enum EnsembleExecutionError {
    #[error("Ensemble not applicable: {reason}")]
    NotApplicable { reason: String },

    #[error("Insufficient models: need {required}, have {available}")]
    InsufficientModels { required: usize, available: usize },

    #[error("All models failed: {errors:?}")]
    AllModelsFailed { errors: Vec<String> },

    #[error("Budget exceeded: estimated {estimated_usd:.4} > limit {limit_usd:.4}")]
    BudgetExceeded { estimated_usd: f64, limit_usd: f64 },

    #[error("Timeout: ensemble execution exceeded {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
}
