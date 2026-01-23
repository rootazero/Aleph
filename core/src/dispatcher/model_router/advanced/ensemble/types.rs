//! Core types for ensemble execution
//!
//! This module contains all type definitions for the ensemble system:
//! - EnsembleMode: Execution strategies (BestOfN, Voting, Consensus, Cascade)
//! - QualityMetric: Response quality scoring methods
//! - EnsembleConfig: Configuration for ensemble execution
//! - EnsembleStrategy: Intent-based strategy mapping
//! - TokenUsage: Token consumption tracking
//! - ModelExecutionResult: Individual model execution results

use crate::dispatcher::model_router::TaskIntent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

// ============================================================================
// Ensemble Mode
// ============================================================================

/// Ensemble execution strategy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "mode")]
#[derive(Default)]
pub enum EnsembleMode {
    /// Disabled - single model routing (no ensemble)
    #[default]
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
    pub fn parse(s: &str) -> Self {
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

fn default_max_cost_multiplier() -> f64 {
    3.0
}

fn default_max_concurrency() -> usize {
    5
}

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
    pub fn get_config(
        &self,
        intent: &TaskIntent,
        complexity: Option<f64>,
    ) -> Option<&EnsembleConfig> {
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
