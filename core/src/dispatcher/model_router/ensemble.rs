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
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::model_router::ensemble::*;
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

use super::{ModelProfile, TaskIntent};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

// ============================================================================
// Ensemble Mode
// ============================================================================

/// Ensemble execution strategy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum EnsembleMode {
    /// Disabled - single model routing (no ensemble)
    Disabled,
    /// Run N models, return best response by quality score
    BestOfN {
        /// Number of models to run
        n: usize,
    },
    /// Run all models, aggregate by voting (majority wins)
    Voting,
    /// Run all models, require minimum agreement level
    Consensus {
        /// Minimum agreement level required (0.0 - 1.0)
        min_agreement: f64,
    },
    /// Run models in priority order until quality threshold met
    Cascade {
        /// Quality threshold to stop (0.0 - 1.0)
        quality_threshold: f64,
    },
}

impl Default for EnsembleMode {
    fn default() -> Self {
        EnsembleMode::Disabled
    }
}

impl EnsembleMode {
    /// Get human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            EnsembleMode::Disabled => "Disabled",
            EnsembleMode::BestOfN { .. } => "Best of N",
            EnsembleMode::Voting => "Voting",
            EnsembleMode::Consensus { .. } => "Consensus",
            EnsembleMode::Cascade { .. } => "Cascade",
        }
    }

    /// Check if ensemble is enabled
    pub fn is_enabled(&self) -> bool {
        !matches!(self, EnsembleMode::Disabled)
    }

    /// Get recommended minimum models for this mode
    pub fn min_models(&self) -> usize {
        match self {
            EnsembleMode::Disabled => 1,
            EnsembleMode::BestOfN { n } => *n,
            EnsembleMode::Voting => 3, // Need odd number for majority
            EnsembleMode::Consensus { .. } => 2,
            EnsembleMode::Cascade { .. } => 2,
        }
    }
}

impl std::fmt::Display for EnsembleMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ============================================================================
// Quality Metric
// ============================================================================

/// Quality scoring method for response evaluation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum QualityMetric {
    /// Response length (longer often better for explanations)
    Length,
    /// Structured response detection (code blocks, lists, etc.)
    Structure,
    /// Combined length and structure scoring
    #[default]
    LengthAndStructure,
    /// Confidence markers in response ("I'm confident", etc.)
    ConfidenceMarkers,
    /// Semantic similarity to prompt (relevance)
    Relevance,
    /// Custom scoring function name
    Custom(String),
}

impl QualityMetric {
    /// Get human-readable display name
    pub fn display_name(&self) -> String {
        match self {
            QualityMetric::Length => "Length".to_string(),
            QualityMetric::Structure => "Structure".to_string(),
            QualityMetric::LengthAndStructure => "Length & Structure".to_string(),
            QualityMetric::ConfidenceMarkers => "Confidence Markers".to_string(),
            QualityMetric::Relevance => "Relevance".to_string(),
            QualityMetric::Custom(name) => format!("Custom: {}", name),
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "length" => QualityMetric::Length,
            "structure" => QualityMetric::Structure,
            "length_and_structure" | "lengthandstructure" => QualityMetric::LengthAndStructure,
            "confidence_markers" | "confidencemarkers" => QualityMetric::ConfidenceMarkers,
            "relevance" => QualityMetric::Relevance,
            other => QualityMetric::Custom(other.to_string()),
        }
    }
}

impl std::fmt::Display for QualityMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ============================================================================
// Ensemble Configuration
// ============================================================================

/// Configuration for ensemble execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleConfig {
    /// Ensemble mode
    pub mode: EnsembleMode,
    /// Models to include in ensemble (profile IDs)
    pub models: Vec<String>,
    /// Maximum wait time for all models (milliseconds)
    pub timeout_ms: u64,
    /// Quality scoring method
    pub quality_metric: QualityMetric,
    /// Whether to use budget-aware model selection
    #[serde(default)]
    pub budget_aware: bool,
    /// Maximum cost multiplier (e.g., 3.0 = max 3x single model cost)
    #[serde(default = "default_max_cost_multiplier")]
    pub max_cost_multiplier: f64,
    /// Maximum concurrent model executions
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

fn default_max_cost_multiplier() -> f64 {
    3.0
}

fn default_max_concurrency() -> usize {
    5
}

impl Default for EnsembleConfig {
    fn default() -> Self {
        Self {
            mode: EnsembleMode::Disabled,
            models: Vec::new(),
            timeout_ms: 30000,
            quality_metric: QualityMetric::default(),
            budget_aware: false,
            max_cost_multiplier: 3.0,
            max_concurrency: 5,
        }
    }
}

impl EnsembleConfig {
    /// Create a new ensemble configuration
    pub fn new(mode: EnsembleMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    /// Create a disabled (single model) configuration
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Create a best-of-n configuration
    pub fn best_of_n(n: usize) -> Self {
        Self::new(EnsembleMode::BestOfN { n })
    }

    /// Create a voting configuration
    pub fn voting() -> Self {
        Self::new(EnsembleMode::Voting)
    }

    /// Create a consensus configuration
    pub fn consensus(min_agreement: f64) -> Self {
        Self::new(EnsembleMode::Consensus {
            min_agreement: min_agreement.clamp(0.0, 1.0),
        })
    }

    /// Create a cascade configuration
    pub fn cascade(quality_threshold: f64) -> Self {
        Self::new(EnsembleMode::Cascade {
            quality_threshold: quality_threshold.clamp(0.0, 1.0),
        })
    }

    /// Set the models to use
    pub fn with_models(mut self, models: Vec<impl Into<String>>) -> Self {
        self.models = models.into_iter().map(|m| m.into()).collect();
        self
    }

    /// Add a model
    pub fn add_model(mut self, model: impl Into<String>) -> Self {
        self.models.push(model.into());
        self
    }

    /// Set the timeout
    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Set the timeout from Duration
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_ms = timeout.as_millis() as u64;
        self
    }

    /// Set the quality metric
    pub fn with_quality_metric(mut self, metric: QualityMetric) -> Self {
        self.quality_metric = metric;
        self
    }

    /// Enable budget-aware selection
    pub fn with_budget_aware(mut self, enabled: bool) -> Self {
        self.budget_aware = enabled;
        self
    }

    /// Set max cost multiplier
    pub fn with_max_cost_multiplier(mut self, multiplier: f64) -> Self {
        self.max_cost_multiplier = multiplier.max(1.0);
        self
    }

    /// Set max concurrency
    pub fn with_max_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = max.max(1);
        self
    }

    /// Get timeout as Duration
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    /// Check if ensemble is enabled
    pub fn is_enabled(&self) -> bool {
        self.mode.is_enabled() && !self.models.is_empty()
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), EnsembleValidationError> {
        if self.mode.is_enabled() {
            let min_models = self.mode.min_models();
            if self.models.len() < min_models {
                return Err(EnsembleValidationError::InsufficientModels {
                    required: min_models,
                    provided: self.models.len(),
                    mode: self.mode.display_name().to_string(),
                });
            }

            // Check for duplicate models
            let unique: HashSet<_> = self.models.iter().collect();
            if unique.len() != self.models.len() {
                return Err(EnsembleValidationError::DuplicateModels);
            }
        }

        if self.timeout_ms == 0 {
            return Err(EnsembleValidationError::ZeroTimeout);
        }

        if self.max_cost_multiplier < 1.0 {
            return Err(EnsembleValidationError::InvalidCostMultiplier {
                value: self.max_cost_multiplier,
            });
        }

        Ok(())
    }
}

/// Errors when validating ensemble configuration
#[derive(Debug, Clone, thiserror::Error)]
pub enum EnsembleValidationError {
    #[error("Ensemble mode '{mode}' requires at least {required} models, got {provided}")]
    InsufficientModels {
        required: usize,
        provided: usize,
        mode: String,
    },

    #[error("Duplicate models in ensemble configuration")]
    DuplicateModels,

    #[error("Timeout cannot be zero")]
    ZeroTimeout,

    #[error("Cost multiplier must be >= 1.0, got {value}")]
    InvalidCostMultiplier { value: f64 },
}

// ============================================================================
// Ensemble Strategy (Intent Mapping)
// ============================================================================

/// Ensemble strategy configuration with intent mapping
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnsembleStrategy {
    /// Default mode when no specific strategy matches
    #[serde(default)]
    pub default_mode: EnsembleMode,
    /// Per-intent ensemble configurations
    #[serde(default)]
    pub intent_strategies: HashMap<TaskIntent, EnsembleConfig>,
    /// Complexity threshold for high-complexity ensemble
    #[serde(default)]
    pub complexity_threshold: Option<f64>,
    /// Configuration for high-complexity prompts
    #[serde(default)]
    pub high_complexity_config: Option<EnsembleConfig>,
}

impl EnsembleStrategy {
    /// Create a new ensemble strategy
    pub fn new() -> Self {
        Self::default()
    }

    /// Set default mode
    pub fn with_default_mode(mut self, mode: EnsembleMode) -> Self {
        self.default_mode = mode;
        self
    }

    /// Add intent-specific strategy
    pub fn add_intent_strategy(mut self, intent: TaskIntent, config: EnsembleConfig) -> Self {
        self.intent_strategies.insert(intent, config);
        self
    }

    /// Set complexity threshold for ensemble triggering
    pub fn with_complexity_threshold(mut self, threshold: f64) -> Self {
        self.complexity_threshold = Some(threshold.clamp(0.0, 1.0));
        self
    }

    /// Set high-complexity configuration
    pub fn with_high_complexity_config(mut self, config: EnsembleConfig) -> Self {
        self.high_complexity_config = Some(config);
        self
    }

    /// Get configuration for a given intent and complexity
    pub fn get_config(&self, intent: &TaskIntent, complexity: Option<f64>) -> Option<&EnsembleConfig> {
        // First check complexity threshold
        if let (Some(threshold), Some(comp), Some(config)) = (
            self.complexity_threshold,
            complexity,
            self.high_complexity_config.as_ref(),
        ) {
            if comp >= threshold {
                return Some(config);
            }
        }

        // Then check intent-specific
        if let Some(config) = self.intent_strategies.get(intent) {
            return Some(config);
        }

        None
    }

    /// Check if ensemble should be used
    pub fn should_use_ensemble(&self, intent: &TaskIntent, complexity: Option<f64>) -> bool {
        // Check complexity threshold
        if let (Some(threshold), Some(comp)) = (self.complexity_threshold, complexity) {
            if comp >= threshold && self.high_complexity_config.is_some() {
                return true;
            }
        }

        // Check intent-specific
        if self.intent_strategies.contains_key(intent) {
            return true;
        }

        // Check default mode
        self.default_mode.is_enabled()
    }
}

// ============================================================================
// Token Usage
// ============================================================================

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input/prompt tokens
    pub input_tokens: u32,
    /// Output/completion tokens
    pub output_tokens: u32,
}

impl TokenUsage {
    /// Create new token usage
    pub fn new(input: u32, output: u32) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
        }
    }

    /// Total tokens
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

// ============================================================================
// Model Execution Result
// ============================================================================

/// Result from a single model execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelExecutionResult {
    /// Model identifier
    pub model_id: String,
    /// Response content (if successful)
    pub response: Option<String>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Execution latency in milliseconds
    pub latency_ms: u64,
    /// Token usage
    pub tokens: TokenUsage,
    /// Estimated cost in USD
    pub cost_usd: f64,
    /// Whether this execution was successful
    pub success: bool,
    /// Quality score (0.0 - 1.0, computed after execution)
    #[serde(default)]
    pub quality_score: Option<f64>,
}

impl ModelExecutionResult {
    /// Create a successful result
    pub fn success(
        model_id: impl Into<String>,
        response: impl Into<String>,
        latency_ms: u64,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            response: Some(response.into()),
            error: None,
            latency_ms,
            tokens: TokenUsage::default(),
            cost_usd: 0.0,
            success: true,
            quality_score: None,
        }
    }

    /// Create a failed result
    pub fn failure(model_id: impl Into<String>, error: impl Into<String>, latency_ms: u64) -> Self {
        Self {
            model_id: model_id.into(),
            response: None,
            error: Some(error.into()),
            latency_ms,
            tokens: TokenUsage::default(),
            cost_usd: 0.0,
            success: false,
            quality_score: None,
        }
    }

    /// Create a timeout result
    pub fn timeout(model_id: impl Into<String>, timeout_ms: u64) -> Self {
        Self {
            model_id: model_id.into(),
            response: None,
            error: Some("Timeout".to_string()),
            latency_ms: timeout_ms,
            tokens: TokenUsage::default(),
            cost_usd: 0.0,
            success: false,
            quality_score: None,
        }
    }

    /// Set token usage
    pub fn with_tokens(mut self, input: u32, output: u32) -> Self {
        self.tokens = TokenUsage::new(input, output);
        self
    }

    /// Set cost
    pub fn with_cost(mut self, cost_usd: f64) -> Self {
        self.cost_usd = cost_usd;
        self
    }

    /// Set quality score
    pub fn with_quality_score(mut self, score: f64) -> Self {
        self.quality_score = Some(score.clamp(0.0, 1.0));
        self
    }

    /// Check if this result has a response
    pub fn has_response(&self) -> bool {
        self.success && self.response.is_some()
    }

    /// Get response reference
    pub fn response_ref(&self) -> Option<&str> {
        self.response.as_deref()
    }
}

// ============================================================================
// Parallel Executor
// ============================================================================

/// Manages parallel execution of multiple models
pub struct ParallelExecutor {
    /// Timeout for entire ensemble
    timeout: Duration,
    /// Maximum concurrent requests
    max_concurrency: usize,
}

impl ParallelExecutor {
    /// Create a new parallel executor
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            max_concurrency: 5,
        }
    }

    /// Create with custom concurrency limit
    pub fn with_max_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = max.max(1);
        self
    }

    /// Create from ensemble config
    pub fn from_config(config: &EnsembleConfig) -> Self {
        Self {
            timeout: config.timeout(),
            max_concurrency: config.max_concurrency,
        }
    }

    /// Get the timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Get max concurrency
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Execute request across multiple models concurrently
    ///
    /// # Arguments
    /// * `models` - Models to execute (profile IDs)
    /// * `executor_fn` - Async function that executes a single model
    ///
    /// # Returns
    /// Vector of execution results (one per model, in same order)
    pub async fn execute_parallel<F, Fut>(
        &self,
        models: &[String],
        executor_fn: F,
    ) -> Vec<ModelExecutionResult>
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(String, TokenUsage, f64), String>> + Send,
    {
        use futures::future::join_all;
        use tokio::time::timeout;

        // Limit concurrency
        let models_to_run: Vec<_> = models.iter().take(self.max_concurrency).cloned().collect();
        let semaphore = Arc::new(Semaphore::new(self.max_concurrency));
        let executor_fn = Arc::new(executor_fn);

        let futures: Vec<_> = models_to_run
            .into_iter()
            .map(|model_id| {
                let sem = semaphore.clone();
                let timeout_duration = self.timeout;
                let model_id_clone = model_id.clone();
                let executor = executor_fn.clone();

                async move {
                    let _permit = sem.acquire().await.unwrap();
                    let start = Instant::now();

                    match timeout(timeout_duration, executor(model_id.clone())).await {
                        Ok(Ok((response, tokens, cost))) => {
                            ModelExecutionResult::success(&model_id, response, start.elapsed().as_millis() as u64)
                                .with_tokens(tokens.input_tokens, tokens.output_tokens)
                                .with_cost(cost)
                        }
                        Ok(Err(error)) => {
                            ModelExecutionResult::failure(&model_id, error, start.elapsed().as_millis() as u64)
                        }
                        Err(_) => {
                            ModelExecutionResult::timeout(&model_id_clone, timeout_duration.as_millis() as u64)
                        }
                    }
                }
            })
            .collect();

        join_all(futures).await
    }

    /// Execute with ModelProfile references (convenience method)
    pub async fn execute_parallel_profiles<F, Fut>(
        &self,
        profiles: &[&ModelProfile],
        executor_fn: F,
    ) -> Vec<ModelExecutionResult>
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(String, TokenUsage, f64), String>> + Send,
    {
        let model_ids: Vec<String> = profiles.iter().map(|p| p.id.clone()).collect();
        self.execute_parallel(&model_ids, executor_fn).await
    }
}

impl Default for ParallelExecutor {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

// ============================================================================
// Quality Scorer Trait and Implementations
// ============================================================================

/// Trait for scoring response quality
pub trait QualityScorer: Send + Sync {
    /// Score a response (0.0 - 1.0, higher is better)
    fn score(&self, response: &str, prompt: &str) -> f64;

    /// Get the metric type this scorer implements
    fn metric(&self) -> QualityMetric;
}

/// Length-based quality scorer
pub struct LengthScorer;

impl QualityScorer for LengthScorer {
    fn score(&self, response: &str, _prompt: &str) -> f64 {
        // Normalize length: 1000 chars = 1.0, capped at 1.0
        (response.len() as f64 / 1000.0).min(1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::Length
    }
}

/// Structure-based quality scorer
pub struct StructureScorer;

impl QualityScorer for StructureScorer {
    fn score(&self, response: &str, _prompt: &str) -> f64 {
        let mut score: f64 = 0.0;

        // Check for code blocks
        if response.contains("```") {
            score += 0.3;
        }

        // Check for bullet lists
        if response.contains("\n- ") || response.contains("\n* ") || response.contains("\n• ") {
            score += 0.25;
        }

        // Check for numbered lists
        if response.contains("\n1.") || response.contains("\n1)") {
            score += 0.2;
        }

        // Check for headers
        if response.contains("\n## ") || response.contains("\n### ") || response.contains("\n# ") {
            score += 0.25;
        }

        // Check for paragraphs (multiple double newlines)
        if response.matches("\n\n").count() >= 2 {
            score += 0.15;
        }

        // Check for emphasis (bold/italic)
        if response.contains("**") || response.contains("__") || response.contains("*") {
            score += 0.1;
        }

        score.min(1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::Structure
    }
}

/// Combined length and structure scorer
pub struct LengthAndStructureScorer {
    length_weight: f64,
    structure_weight: f64,
}

impl Default for LengthAndStructureScorer {
    fn default() -> Self {
        Self {
            length_weight: 0.4,
            structure_weight: 0.6,
        }
    }
}

impl LengthAndStructureScorer {
    /// Create with custom weights
    pub fn new(length_weight: f64, structure_weight: f64) -> Self {
        let total = length_weight + structure_weight;
        Self {
            length_weight: length_weight / total,
            structure_weight: structure_weight / total,
        }
    }
}

impl QualityScorer for LengthAndStructureScorer {
    fn score(&self, response: &str, prompt: &str) -> f64 {
        let length_scorer = LengthScorer;
        let structure_scorer = StructureScorer;

        let length_score = length_scorer.score(response, prompt);
        let structure_score = structure_scorer.score(response, prompt);

        self.length_weight * length_score + self.structure_weight * structure_score
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::LengthAndStructure
    }
}

/// Confidence markers scorer
pub struct ConfidenceMarkersScorer;

impl QualityScorer for ConfidenceMarkersScorer {
    fn score(&self, response: &str, _prompt: &str) -> f64 {
        let response_lower = response.to_lowercase();
        let mut score: f64 = 0.5; // Start neutral

        // Positive confidence markers
        let positive_markers = [
            "i'm confident",
            "i am confident",
            "certainly",
            "definitely",
            "clearly",
            "without a doubt",
            "absolutely",
            "the answer is",
            "this is correct",
        ];

        for marker in positive_markers {
            if response_lower.contains(marker) {
                score += 0.1;
            }
        }

        // Negative/hedging markers
        let negative_markers = [
            "i think",
            "i believe",
            "might be",
            "could be",
            "possibly",
            "perhaps",
            "i'm not sure",
            "i am not sure",
            "uncertain",
            "it depends",
        ];

        for marker in negative_markers {
            if response_lower.contains(marker) {
                score -= 0.05;
            }
        }

        score.clamp(0.0, 1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::ConfidenceMarkers
    }
}

/// Relevance scorer (based on word overlap with prompt)
pub struct RelevanceScorer;

impl QualityScorer for RelevanceScorer {
    fn score(&self, response: &str, prompt: &str) -> f64 {
        let prompt_words: HashSet<_> = prompt
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 3) // Skip short words
            .collect();

        if prompt_words.is_empty() {
            return 0.5;
        }

        let response_words: HashSet<_> = response
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 3)
            .collect();

        let overlap = prompt_words.intersection(&response_words).count();
        let coverage = overlap as f64 / prompt_words.len() as f64;

        // Scale: 30% coverage = 0.5, 60% coverage = 1.0
        ((coverage - 0.3) / 0.3).clamp(0.0, 1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::Relevance
    }
}

/// Create a scorer from a QualityMetric
pub fn create_scorer(metric: &QualityMetric) -> Box<dyn QualityScorer> {
    match metric {
        QualityMetric::Length => Box::new(LengthScorer),
        QualityMetric::Structure => Box::new(StructureScorer),
        QualityMetric::LengthAndStructure => Box::new(LengthAndStructureScorer::default()),
        QualityMetric::ConfidenceMarkers => Box::new(ConfidenceMarkersScorer),
        QualityMetric::Relevance => Box::new(RelevanceScorer),
        QualityMetric::Custom(_) => Box::new(LengthAndStructureScorer::default()), // Fallback
    }
}

// ============================================================================
// Ensemble Result
// ============================================================================

/// Result from ensemble execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleResult {
    /// Final aggregated response
    pub response: String,
    /// Model that produced the selected response
    pub selected_model: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// All model results
    pub all_results: Vec<ModelExecutionResult>,
    /// Aggregation method used
    pub aggregation_method: String,
    /// Total cost of ensemble (sum of all models)
    pub total_cost_usd: f64,
    /// Total latency (wall clock time, max of individual latencies)
    pub total_latency_ms: u64,
    /// Consensus level (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consensus_level: Option<f64>,
    /// Number of successful model calls
    pub successful_count: usize,
    /// Number of failed model calls
    pub failed_count: usize,
}

impl EnsembleResult {
    /// Create a result from a single successful response (fallback)
    pub fn from_single(result: ModelExecutionResult) -> Self {
        let response = result.response.clone().unwrap_or_default();
        let model_id = result.model_id.clone();
        let latency = result.latency_ms;
        let cost = result.cost_usd;
        let confidence = result.quality_score.unwrap_or(0.5);
        let success = result.success;

        Self {
            response,
            selected_model: model_id,
            confidence,
            all_results: vec![result],
            aggregation_method: "single".to_string(),
            total_cost_usd: cost,
            total_latency_ms: latency,
            consensus_level: None,
            successful_count: if success { 1 } else { 0 },
            failed_count: if success { 0 } else { 1 },
        }
    }

    /// Create an error result
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            response: String::new(),
            selected_model: String::new(),
            confidence: 0.0,
            all_results: Vec::new(),
            aggregation_method: "error".to_string(),
            total_cost_usd: 0.0,
            total_latency_ms: 0,
            consensus_level: None,
            successful_count: 0,
            failed_count: 0,
        }
    }

    /// Check if this is a successful result
    pub fn is_success(&self) -> bool {
        !self.response.is_empty() && self.successful_count > 0
    }

    /// Get total tokens used
    pub fn total_tokens(&self) -> TokenUsage {
        let input: u32 = self.all_results.iter().map(|r| r.tokens.input_tokens).sum();
        let output: u32 = self.all_results.iter().map(|r| r.tokens.output_tokens).sum();
        TokenUsage::new(input, output)
    }
}

// ============================================================================
// Response Aggregator
// ============================================================================

/// Aggregates multiple model responses into a single result
pub struct ResponseAggregator {
    scorer: Box<dyn QualityScorer>,
    consensus_threshold: f64,
}

impl ResponseAggregator {
    /// Create a new response aggregator
    pub fn new(metric: &QualityMetric) -> Self {
        Self {
            scorer: create_scorer(metric),
            consensus_threshold: 0.7,
        }
    }

    /// Set consensus threshold
    pub fn with_consensus_threshold(mut self, threshold: f64) -> Self {
        self.consensus_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Score all results
    pub fn score_results(&self, results: &mut [ModelExecutionResult], prompt: &str) {
        for result in results.iter_mut() {
            if let Some(response) = &result.response {
                result.quality_score = Some(self.scorer.score(response, prompt));
            }
        }
    }

    /// Aggregate using best-of-n strategy
    pub fn best_of_n(&self, mut results: Vec<ModelExecutionResult>, prompt: &str) -> EnsembleResult {
        // Score all successful results
        self.score_results(&mut results, prompt);

        let successful: Vec<_> = results.iter().filter(|r| r.has_response()).collect();

        if successful.is_empty() {
            return self.fallback_error(&results);
        }

        // Find best by quality score
        let best = successful
            .iter()
            .max_by(|a, b| {
                a.quality_score
                    .unwrap_or(0.0)
                    .partial_cmp(&b.quality_score.unwrap_or(0.0))
                    .unwrap()
            })
            .unwrap();

        let total_cost: f64 = results.iter().map(|r| r.cost_usd).sum();
        let max_latency = results.iter().map(|r| r.latency_ms).max().unwrap_or(0);
        let successful_count = successful.len();
        let failed_count = results.len() - successful_count;

        EnsembleResult {
            response: best.response.clone().unwrap(),
            selected_model: best.model_id.clone(),
            confidence: best.quality_score.unwrap_or(0.5),
            all_results: results,
            aggregation_method: "best_of_n".to_string(),
            total_cost_usd: total_cost,
            total_latency_ms: max_latency,
            consensus_level: None,
            successful_count,
            failed_count,
        }
    }

    /// Aggregate using consensus detection
    pub fn consensus(&self, mut results: Vec<ModelExecutionResult>, prompt: &str) -> EnsembleResult {
        self.score_results(&mut results, prompt);

        let successful: Vec<_> = results.iter().filter(|r| r.has_response()).collect();

        if successful.len() < 2 {
            return self.best_of_n(results, prompt);
        }

        // Calculate pairwise similarity
        let similarities = self.calculate_similarities(&successful);
        let consensus_level = if similarities.is_empty() {
            0.0
        } else {
            similarities.iter().sum::<f64>() / similarities.len() as f64
        };

        // Get best result first
        let mut result = self.best_of_n(results, prompt);
        result.consensus_level = Some(consensus_level);
        result.aggregation_method = "consensus".to_string();

        // Adjust confidence based on consensus
        if consensus_level < self.consensus_threshold {
            result.confidence *= 0.7; // Reduce confidence for low consensus
        }

        result
    }

    /// Aggregate using voting
    pub fn voting(&self, mut results: Vec<ModelExecutionResult>, prompt: &str) -> EnsembleResult {
        self.score_results(&mut results, prompt);

        let successful: Vec<_> = results.iter().filter(|r| r.has_response()).collect();

        if successful.len() < 3 {
            return self.best_of_n(results, prompt);
        }

        // Group responses by similarity
        let groups = self.group_by_similarity(&successful);

        // Find largest group
        let largest_group = groups
            .iter()
            .max_by_key(|g| g.len())
            .unwrap();

        // Select best from largest group
        let best = largest_group
            .iter()
            .max_by(|a, b| {
                a.quality_score
                    .unwrap_or(0.0)
                    .partial_cmp(&b.quality_score.unwrap_or(0.0))
                    .unwrap()
            })
            .unwrap();

        let total_cost: f64 = results.iter().map(|r| r.cost_usd).sum();
        let max_latency = results.iter().map(|r| r.latency_ms).max().unwrap_or(0);
        let vote_confidence = largest_group.len() as f64 / successful.len() as f64;
        let successful_count = successful.len();
        let total_count = results.len();

        EnsembleResult {
            response: best.response.clone().unwrap(),
            selected_model: best.model_id.clone(),
            confidence: vote_confidence * best.quality_score.unwrap_or(0.5),
            all_results: results,
            aggregation_method: "voting".to_string(),
            total_cost_usd: total_cost,
            total_latency_ms: max_latency,
            consensus_level: Some(vote_confidence),
            successful_count,
            failed_count: total_count - successful_count,
        }
    }

    /// Calculate pairwise Jaccard similarity between responses
    fn calculate_similarities(&self, results: &[&ModelExecutionResult]) -> Vec<f64> {
        let mut similarities = Vec::new();

        for i in 0..results.len() {
            for j in (i + 1)..results.len() {
                if let (Some(r1), Some(r2)) = (&results[i].response, &results[j].response) {
                    similarities.push(jaccard_similarity(r1, r2));
                }
            }
        }

        similarities
    }

    /// Group responses by similarity
    fn group_by_similarity<'a>(
        &self,
        results: &[&'a ModelExecutionResult],
    ) -> Vec<Vec<&'a ModelExecutionResult>> {
        if results.is_empty() {
            return Vec::new();
        }

        let mut groups: Vec<Vec<&ModelExecutionResult>> = Vec::new();

        for result in results {
            let mut found_group = false;

            for group in &mut groups {
                // Check similarity with first member of group
                if let (Some(r1), Some(r2)) = (&group[0].response, &result.response) {
                    if jaccard_similarity(r1, r2) >= self.consensus_threshold {
                        group.push(result);
                        found_group = true;
                        break;
                    }
                }
            }

            if !found_group {
                groups.push(vec![result]);
            }
        }

        groups
    }

    /// Create error result when all models failed
    fn fallback_error(&self, results: &[ModelExecutionResult]) -> EnsembleResult {
        let total_cost: f64 = results.iter().map(|r| r.cost_usd).sum();
        let max_latency = results.iter().map(|r| r.latency_ms).max().unwrap_or(0);

        let error_msg = results
            .iter()
            .filter_map(|r| r.error.as_ref())
            .next()
            .cloned()
            .unwrap_or_else(|| "All models failed".to_string());

        EnsembleResult {
            response: format!("Error: {}", error_msg),
            selected_model: String::new(),
            confidence: 0.0,
            all_results: results.to_vec(),
            aggregation_method: "fallback_error".to_string(),
            total_cost_usd: total_cost,
            total_latency_ms: max_latency,
            consensus_level: None,
            successful_count: 0,
            failed_count: results.len(),
        }
    }
}

/// Calculate Jaccard similarity between two strings (word-based)
pub fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let words_a: HashSet<_> = a
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 2)
        .collect();

    let words_b: HashSet<_> = b
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 2)
        .collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

// ============================================================================
// EnsembleEngine - Main Integration Point
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
        if let Some(config) = self.config.strategy.get_config(&request.intent, request.complexity) {
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
                errors: results
                    .iter()
                    .filter_map(|r| r.error.clone())
                    .collect(),
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
                    let mut r = ModelExecutionResult::success(model_id.clone(), &response, latency_ms);
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
                        let successful_count = all_results.iter().filter(|r| r.response.is_some()).count();
                        let failed_count = all_results.iter().filter(|r| r.response.is_none()).count();
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
        let successful: Vec<_> = all_results.iter().filter(|r| r.response.is_some()).collect();
        if successful.is_empty() {
            return Err(EnsembleExecutionError::AllModelsFailed {
                errors: all_results
                    .iter()
                    .filter_map(|r| r.error.clone())
                    .collect(),
            });
        }

        // Find best by quality score
        let best = successful
            .iter()
            .max_by(|a, b| {
                let score_a = scorer.score(a.response.as_ref().unwrap(), prompt);
                let score_b = scorer.score(b.response.as_ref().unwrap(), prompt);
                score_a.partial_cmp(&score_b).unwrap()
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
    fn aggregate(&self, config: &EnsembleConfig, prompt: &str, results: Vec<ModelExecutionResult>) -> EnsembleResult {
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
            EnsembleMode::Cascade { quality_threshold: _ } => {
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensemble_mode_min_models() {
        assert_eq!(EnsembleMode::Disabled.min_models(), 1);
        assert_eq!(EnsembleMode::BestOfN { n: 3 }.min_models(), 3);
        assert_eq!(EnsembleMode::Voting.min_models(), 3);
        assert_eq!(EnsembleMode::Consensus { min_agreement: 0.7 }.min_models(), 2);
        assert_eq!(EnsembleMode::Cascade { quality_threshold: 0.8 }.min_models(), 2);
    }

    #[test]
    fn test_ensemble_config_validation() {
        // Valid config
        let valid = EnsembleConfig::best_of_n(2)
            .with_models(vec!["model-a", "model-b"]);
        assert!(valid.validate().is_ok());

        // Invalid: not enough models
        let insufficient = EnsembleConfig::best_of_n(3)
            .with_models(vec!["model-a", "model-b"]);
        assert!(matches!(
            insufficient.validate(),
            Err(EnsembleValidationError::InsufficientModels { .. })
        ));

        // Invalid: duplicate models
        let duplicates = EnsembleConfig::best_of_n(2)
            .with_models(vec!["model-a", "model-a"]);
        assert!(matches!(
            duplicates.validate(),
            Err(EnsembleValidationError::DuplicateModels)
        ));

        // Invalid: zero timeout
        let mut zero_timeout = EnsembleConfig::best_of_n(2)
            .with_models(vec!["model-a", "model-b"]);
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
        assert!(strategy.get_config(&TaskIntent::GeneralChat, None).is_none());

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
            ModelExecutionResult::success("model-b", "This is a longer response with more content and details", 150),
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
            ModelExecutionResult::success("model-a", "The answer is 42 because of the meaning of life", 100),
            ModelExecutionResult::success("model-b", "The answer is 42 due to the meaning of everything", 120),
            ModelExecutionResult::success("model-c", "Something completely different about cats", 80),
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
        let executor = ParallelExecutor::new(Duration::from_secs(5))
            .with_max_concurrency(3);

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

        let models = vec![
            "good-model".to_string(),
            "bad-model".to_string(),
        ];

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
        assert_eq!(QualityMetric::from_str("length"), QualityMetric::Length);
        assert_eq!(QualityMetric::from_str("STRUCTURE"), QualityMetric::Structure);
        assert_eq!(
            QualityMetric::from_str("length_and_structure"),
            QualityMetric::LengthAndStructure
        );
        assert_eq!(
            QualityMetric::from_str("custom_metric"),
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
