//! Core types for A/B testing framework
//!
//! This module contains type definitions:
//! - TrackedMetric: Metrics that can be tracked
//! - VariantConfig: Configuration for experiment variants
//! - ExperimentConfig: Configuration for experiments
//! - VariantAssignment: Result of variant assignment
//! - AssignmentStrategy: How to assign traffic

use crate::dispatcher::model_router::{CostStrategy, TaskIntent};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

// ============================================================================
// Type Aliases
// ============================================================================

/// Unique identifier for an experiment (newtype for type safety)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExperimentId(String);

impl ExperimentId {
    /// Create a new ExperimentId
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the inner string reference
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert into inner String
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for ExperimentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ExperimentId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl std::fmt::Display for ExperimentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for ExperimentId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Unique identifier for a variant within an experiment (newtype for type safety)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VariantId(String);

impl VariantId {
    /// Create a new VariantId
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the inner string reference
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert into inner String
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for VariantId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for VariantId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl std::fmt::Display for VariantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for VariantId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// ============================================================================
// Tracked Metrics
// ============================================================================

/// Metrics that can be tracked per variant for analysis
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TrackedMetric {
    /// Response latency in milliseconds
    LatencyMs,
    /// Cost in USD
    CostUsd,
    /// Input token count
    InputTokens,
    /// Output token count
    OutputTokens,
    /// Success rate (1.0 for success, 0.0 for failure)
    SuccessRate,
    /// Cache hit rate (1.0 for hit, 0.0 for miss)
    CacheHitRate,
    /// Number of retries needed
    RetryCount,
    /// User rating (requires external feedback, 1-5 scale)
    UserRating,
    /// Custom metric with arbitrary name
    Custom(String),
}

impl TrackedMetric {
    /// Get human-readable display name
    pub fn display_name(&self) -> String {
        match self {
            TrackedMetric::LatencyMs => "Latency (ms)".to_string(),
            TrackedMetric::CostUsd => "Cost (USD)".to_string(),
            TrackedMetric::InputTokens => "Input Tokens".to_string(),
            TrackedMetric::OutputTokens => "Output Tokens".to_string(),
            TrackedMetric::SuccessRate => "Success Rate".to_string(),
            TrackedMetric::CacheHitRate => "Cache Hit Rate".to_string(),
            TrackedMetric::RetryCount => "Retry Count".to_string(),
            TrackedMetric::UserRating => "User Rating".to_string(),
            TrackedMetric::Custom(name) => name.clone(),
        }
    }

    /// Get all built-in metrics
    pub fn all_builtin() -> Vec<TrackedMetric> {
        vec![
            TrackedMetric::LatencyMs,
            TrackedMetric::CostUsd,
            TrackedMetric::InputTokens,
            TrackedMetric::OutputTokens,
            TrackedMetric::SuccessRate,
            TrackedMetric::CacheHitRate,
            TrackedMetric::RetryCount,
            TrackedMetric::UserRating,
        ]
    }

    /// Parse metric from string
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "latency_ms" | "latency" => TrackedMetric::LatencyMs,
            "cost_usd" | "cost" => TrackedMetric::CostUsd,
            "input_tokens" => TrackedMetric::InputTokens,
            "output_tokens" => TrackedMetric::OutputTokens,
            "success_rate" => TrackedMetric::SuccessRate,
            "cache_hit_rate" => TrackedMetric::CacheHitRate,
            "retry_count" => TrackedMetric::RetryCount,
            "user_rating" => TrackedMetric::UserRating,
            other => TrackedMetric::Custom(other.to_string()),
        }
    }
}

impl std::fmt::Display for TrackedMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ============================================================================
// Variant Configuration
// ============================================================================

/// Configuration for a single variant in an experiment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantConfig {
    /// Unique variant identifier within the experiment
    pub id: VariantId,
    /// Human-readable display name
    pub name: String,
    /// Relative weight for traffic allocation (compared to other variants)
    pub weight: u32,
    /// Optional: Override model selection to use this specific model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Optional: Override cost strategy for this variant
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_strategy_override: Option<CostStrategy>,
    /// Custom parameters for advanced use cases (JSON blob)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl VariantConfig {
    /// Create a new variant with the given ID
    pub fn new(id: impl Into<String>) -> Self {
        let id_str = id.into();
        Self {
            name: id_str.clone(),
            id: VariantId::new(id_str),
            weight: 50,
            model_override: None,
            cost_strategy_override: None,
            parameters: None,
        }
    }

    /// Create a control variant (typically weight 50)
    pub fn control(model: impl Into<String>) -> Self {
        Self {
            id: VariantId::new("control"),
            name: "Control".to_string(),
            weight: 50,
            model_override: Some(model.into()),
            cost_strategy_override: None,
            parameters: None,
        }
    }

    /// Create a treatment variant (typically weight 50)
    pub fn treatment(model: impl Into<String>) -> Self {
        Self {
            id: VariantId::new("treatment"),
            name: "Treatment".to_string(),
            weight: 50,
            model_override: Some(model.into()),
            cost_strategy_override: None,
            parameters: None,
        }
    }

    /// Set the display name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the weight
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    /// Set model override
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_override = Some(model.into());
        self
    }

    /// Set cost strategy override
    pub fn with_cost_strategy(mut self, strategy: CostStrategy) -> Self {
        self.cost_strategy_override = Some(strategy);
        self
    }
}

// ============================================================================
// Experiment Configuration
// ============================================================================

/// Configuration for a single A/B experiment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentConfig {
    /// Unique experiment identifier
    pub id: ExperimentId,
    /// Human-readable name
    pub name: String,
    /// Whether the experiment is currently running
    pub enabled: bool,
    /// Percentage of total traffic to include in experiment (0-100)
    pub traffic_percentage: u8,
    /// Variants with their configurations
    pub variants: Vec<VariantConfig>,
    /// Optional: Only include requests with this TaskIntent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_intent: Option<TaskIntent>,
    /// Optional: Minimum complexity score to include (0.0 - 1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_complexity: Option<f64>,
    /// Metrics to track for analysis
    #[serde(default)]
    pub tracked_metrics: Vec<TrackedMetric>,
    /// Optional: Experiment start time (defaults to creation time)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<SystemTime>,
    /// Optional: Experiment end time (runs indefinitely if None)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<SystemTime>,
}

impl ExperimentConfig {
    /// Create a new experiment with the given ID
    pub fn new(id: impl Into<String>) -> Self {
        let id_str = id.into();
        Self {
            name: id_str.clone(),
            id: ExperimentId::new(id_str),
            enabled: true,
            traffic_percentage: 10,
            variants: Vec::new(),
            target_intent: None,
            min_complexity: None,
            tracked_metrics: vec![
                TrackedMetric::LatencyMs,
                TrackedMetric::CostUsd,
                TrackedMetric::SuccessRate,
            ],
            start_time: Some(SystemTime::now()),
            end_time: None,
        }
    }

    /// Set the experiment name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the traffic percentage
    pub fn with_traffic_percentage(mut self, percentage: u8) -> Self {
        self.traffic_percentage = percentage.min(100);
        self
    }

    /// Add a variant
    pub fn add_variant(mut self, variant: VariantConfig) -> Self {
        self.variants.push(variant);
        self
    }

    /// Set target intent filter
    pub fn with_target_intent(mut self, intent: TaskIntent) -> Self {
        self.target_intent = Some(intent);
        self
    }

    /// Set minimum complexity filter
    pub fn with_min_complexity(mut self, complexity: f64) -> Self {
        self.min_complexity = Some(complexity.clamp(0.0, 1.0));
        self
    }

    /// Set tracked metrics
    pub fn with_tracked_metrics(mut self, metrics: Vec<TrackedMetric>) -> Self {
        self.tracked_metrics = metrics;
        self
    }

    /// Set enabled status
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set end time
    pub fn with_end_time(mut self, end_time: SystemTime) -> Self {
        self.end_time = Some(end_time);
        self
    }

    /// Check if experiment is currently active
    pub fn is_active(&self) -> bool {
        if !self.enabled {
            return false;
        }

        let now = SystemTime::now();

        // Check start time
        if let Some(start) = self.start_time {
            if now < start {
                return false;
            }
        }

        // Check end time
        if let Some(end) = self.end_time {
            if now > end {
                return false;
            }
        }

        // Must have at least 2 variants
        if self.variants.len() < 2 {
            return false;
        }

        true
    }

    /// Get total weight across all variants
    pub fn total_weight(&self) -> u32 {
        self.variants.iter().map(|v| v.weight).sum()
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ExperimentValidationError> {
        if self.id.is_empty() {
            return Err(ExperimentValidationError::EmptyId);
        }

        if self.variants.len() < 2 {
            return Err(ExperimentValidationError::InsufficientVariants {
                count: self.variants.len(),
            });
        }

        if self.total_weight() == 0 {
            return Err(ExperimentValidationError::ZeroTotalWeight);
        }

        // Check for duplicate variant IDs
        let mut seen_ids = std::collections::HashSet::new();
        for variant in &self.variants {
            if !seen_ids.insert(&variant.id) {
                return Err(ExperimentValidationError::DuplicateVariantId {
                    variant_id: variant.id.to_string(),
                });
            }
        }

        if self.tracked_metrics.is_empty() {
            return Err(ExperimentValidationError::NoTrackedMetrics);
        }

        Ok(())
    }
}

/// Errors when validating experiment configuration
#[derive(Debug, Clone, thiserror::Error)]
pub enum ExperimentValidationError {
    #[error("Experiment ID cannot be empty")]
    EmptyId,

    #[error("Experiment must have at least 2 variants, got {count}")]
    InsufficientVariants { count: usize },

    #[error("Total variant weight cannot be zero")]
    ZeroTotalWeight,

    #[error("Duplicate variant ID: {variant_id}")]
    DuplicateVariantId { variant_id: String },

    #[error("At least one tracked metric is required")]
    NoTrackedMetrics,
}

// ============================================================================
// Variant Assignment Result
// ============================================================================

/// Result of variant assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantAssignment {
    /// The experiment this assignment belongs to
    pub experiment_id: ExperimentId,
    /// Human-readable experiment name
    pub experiment_name: String,
    /// The assigned variant ID
    pub variant_id: VariantId,
    /// Human-readable variant name
    pub variant_name: String,
    /// Model override from variant config (if any)
    pub model_override: Option<String>,
    /// Cost strategy override from variant config (if any)
    pub cost_strategy_override: Option<CostStrategy>,
    /// Whether this is the control variant
    pub is_control: bool,
}

impl VariantAssignment {
    /// Create from experiment and variant configs
    pub fn from_configs(experiment: &ExperimentConfig, variant: &VariantConfig) -> Self {
        Self {
            experiment_id: experiment.id.clone(),
            experiment_name: experiment.name.clone(),
            variant_id: variant.id.clone(),
            variant_name: variant.name.clone(),
            model_override: variant.model_override.clone(),
            cost_strategy_override: variant.cost_strategy_override,
            is_control: variant.id == VariantId::from("control"),
        }
    }
}

// ============================================================================
// Assignment Strategy
// ============================================================================

/// Strategy for assigning traffic to experiments/variants
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentStrategy {
    /// Hash user_id for consistent assignment across sessions
    #[default]
    UserId,
    /// Hash session_id for consistent assignment within session
    SessionId,
    /// Random per request (no consistency guarantee)
    RequestId,
}

impl AssignmentStrategy {
    /// Get human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            AssignmentStrategy::UserId => "User ID",
            AssignmentStrategy::SessionId => "Session ID",
            AssignmentStrategy::RequestId => "Request ID",
        }
    }
}

impl std::fmt::Display for AssignmentStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}
