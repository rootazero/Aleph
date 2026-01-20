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
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::model_router::ab_testing::*;
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

use super::{CostStrategy, PromptFeatures, TaskIntent};
use serde::{Deserialize, Serialize};
use siphasher::sip::SipHasher24;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::RwLock;
use std::time::SystemTime;

// ============================================================================
// Type Aliases
// ============================================================================

/// Unique identifier for an experiment
pub type ExperimentId = String;

/// Unique identifier for a variant within an experiment
pub type VariantId = String;

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
    pub fn from_str(s: &str) -> Self {
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
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            weight: 50,
            model_override: None,
            cost_strategy_override: None,
            parameters: None,
        }
    }

    /// Create a control variant (typically weight 50)
    pub fn control(model: impl Into<String>) -> Self {
        Self {
            id: "control".to_string(),
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
            id: "treatment".to_string(),
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
        let id = id.into();
        Self {
            name: id.clone(),
            id,
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
                    variant_id: variant.id.clone(),
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
            is_control: variant.id == "control",
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

// ============================================================================
// Traffic Split Manager
// ============================================================================

/// Manages traffic splitting for A/B experiments using consistent hashing
pub struct TrafficSplitManager {
    /// Active experiments indexed by ID
    experiments: HashMap<ExperimentId, ExperimentConfig>,
    /// Assignment strategy
    strategy: AssignmentStrategy,
    /// Hash seed for reproducibility
    hash_seed: u64,
}

impl TrafficSplitManager {
    /// Create a new traffic split manager
    pub fn new(experiments: Vec<ExperimentConfig>, strategy: AssignmentStrategy) -> Self {
        let experiments_map = experiments
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();

        Self {
            experiments: experiments_map,
            strategy,
            hash_seed: 0x517cc1b727220a95, // Deterministic seed for consistent hashing
        }
    }

    /// Create with a custom hash seed
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.hash_seed = seed;
        self
    }

    /// Get the assignment strategy
    pub fn strategy(&self) -> AssignmentStrategy {
        self.strategy
    }

    /// Set the assignment strategy
    pub fn set_strategy(&mut self, strategy: AssignmentStrategy) {
        self.strategy = strategy;
    }

    /// Add an experiment
    pub fn add_experiment(&mut self, experiment: ExperimentConfig) {
        self.experiments.insert(experiment.id.clone(), experiment);
    }

    /// Remove an experiment
    pub fn remove_experiment(&mut self, experiment_id: &str) -> Option<ExperimentConfig> {
        self.experiments.remove(experiment_id)
    }

    /// Get an experiment by ID
    pub fn get_experiment(&self, experiment_id: &str) -> Option<&ExperimentConfig> {
        self.experiments.get(experiment_id)
    }

    /// Get all experiments
    pub fn experiments(&self) -> impl Iterator<Item = &ExperimentConfig> {
        self.experiments.values()
    }

    /// Get active experiments (enabled and within time window)
    pub fn active_experiments(&self) -> impl Iterator<Item = &ExperimentConfig> {
        self.experiments.values().filter(|e| e.is_active())
    }

    /// Assign a request to an experiment variant
    ///
    /// Returns `Some(VariantAssignment)` if the request is assigned to an experiment,
    /// or `None` if the request is not in any experiment.
    pub fn assign(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        request_id: &str,
        intent: &TaskIntent,
        features: Option<&PromptFeatures>,
    ) -> Option<VariantAssignment> {
        // Determine the assignment key based on strategy
        let assignment_key = self.get_assignment_key(user_id, session_id, request_id);

        // Try each active experiment
        for experiment in self.active_experiments() {
            // Filter by target intent
            if let Some(ref target_intent) = experiment.target_intent {
                if target_intent != intent {
                    continue;
                }
            }

            // Filter by minimum complexity
            if let Some(min_complexity) = experiment.min_complexity {
                if let Some(features) = features {
                    if features.complexity_score < min_complexity {
                        continue;
                    }
                }
            }

            // Check if this request falls into the experiment's traffic sample
            if self.is_in_traffic_sample(&assignment_key, &experiment.id, experiment.traffic_percentage) {
                // Assign to a variant
                if let Some(variant) = self.select_variant(&assignment_key, experiment) {
                    return Some(VariantAssignment::from_configs(experiment, variant));
                }
            }
        }

        None
    }

    /// Get the assignment key based on strategy
    fn get_assignment_key(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        request_id: &str,
    ) -> String {
        match self.strategy {
            AssignmentStrategy::UserId => {
                user_id
                    .or(session_id)
                    .unwrap_or(request_id)
                    .to_string()
            }
            AssignmentStrategy::SessionId => {
                session_id
                    .or(user_id)
                    .unwrap_or(request_id)
                    .to_string()
            }
            AssignmentStrategy::RequestId => request_id.to_string(),
        }
    }

    /// Check if assignment key falls into experiment's traffic sample
    fn is_in_traffic_sample(&self, key: &str, experiment_id: &str, traffic_percentage: u8) -> bool {
        let hash = self.compute_hash(key, experiment_id);
        let sample = (hash % 100) as u8;
        sample < traffic_percentage
    }

    /// Select a variant based on weighted distribution
    fn select_variant<'a>(
        &self,
        key: &str,
        experiment: &'a ExperimentConfig,
    ) -> Option<&'a VariantConfig> {
        let total_weight = experiment.total_weight();
        if total_weight == 0 {
            return None;
        }

        // Use a different hash for variant selection
        let hash = self.compute_hash(key, &format!("{}-variant", experiment.id));
        let bucket = (hash % total_weight as u64) as u32;

        // Find the variant that contains this bucket
        let mut cumulative = 0u32;
        for variant in &experiment.variants {
            cumulative += variant.weight;
            if bucket < cumulative {
                return Some(variant);
            }
        }

        // Fallback to first variant (shouldn't happen)
        experiment.variants.first()
    }

    /// Compute a deterministic hash for the given key and salt
    fn compute_hash(&self, key: &str, salt: &str) -> u64 {
        let mut hasher = SipHasher24::new_with_keys(self.hash_seed, 0);
        key.hash(&mut hasher);
        salt.hash(&mut hasher);
        hasher.finish()
    }

    /// Enable an experiment
    pub fn enable_experiment(&mut self, experiment_id: &str) -> bool {
        if let Some(experiment) = self.experiments.get_mut(experiment_id) {
            experiment.enabled = true;
            if experiment.start_time.is_none() {
                experiment.start_time = Some(SystemTime::now());
            }
            true
        } else {
            false
        }
    }

    /// Disable an experiment
    pub fn disable_experiment(&mut self, experiment_id: &str) -> bool {
        if let Some(experiment) = self.experiments.get_mut(experiment_id) {
            experiment.enabled = false;
            true
        } else {
            false
        }
    }
}

impl Default for TrafficSplitManager {
    fn default() -> Self {
        Self::new(Vec::new(), AssignmentStrategy::default())
    }
}

// ============================================================================
// Outcome Tracking
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
                .or_insert_with(MetricStats::new)
                .record(*value);
        }
    }

    /// Get stats for a specific metric
    pub fn get_metric(&self, metric: &TrackedMetric) -> Option<&MetricStats> {
        self.metrics.get(metric)
    }
}

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
            let experiment_stats = stats
                .entry(outcome.experiment_id.clone())
                .or_default();
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
    pub fn get_variant_stats(
        &self,
        experiment_id: &str,
        variant_id: &str,
    ) -> Option<VariantStats> {
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

// ============================================================================
// Statistical Analysis
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
        let ser = 1.000000000190015
            + 76.18009172947146 / (x + 1.0)
            - 86.50532032941677 / (x + 2.0)
            + 24.01409824083091 / (x + 3.0)
            - 1.231739572450155 / (x + 4.0)
            + 0.1208650973866179e-2 / (x + 5.0)
            - 0.5395239384953e-5 / (x + 6.0);

        (tmp - (x + 0.5) * tmp.ln() + ser.ln()) * -1.0 + (2.5066282746310005 * ser / (x + 1.0)).ln()
    }
}

// ============================================================================
// Experiment Status and Report
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
            min: if stats.min.is_infinite() { 0.0 } else { stats.min },
            max: if stats.max.is_infinite() { 0.0 } else { stats.max },
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
    pub fn generate(
        config: &ExperimentConfig,
        stats: &HashMap<VariantId, VariantStats>,
    ) -> Self {
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

// ============================================================================
// A/B Testing Engine
// ============================================================================

/// Main A/B testing engine that combines traffic splitting and outcome tracking
pub struct ABTestingEngine {
    /// Traffic split manager for variant assignment
    split_manager: TrafficSplitManager,
    /// Outcome tracker for recording results
    outcome_tracker: OutcomeTracker,
}

impl ABTestingEngine {
    /// Create a new A/B testing engine
    pub fn new(experiments: Vec<ExperimentConfig>) -> Self {
        Self {
            split_manager: TrafficSplitManager::new(experiments, AssignmentStrategy::default()),
            outcome_tracker: OutcomeTracker::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        experiments: Vec<ExperimentConfig>,
        strategy: AssignmentStrategy,
        max_raw_outcomes: usize,
    ) -> Self {
        Self {
            split_manager: TrafficSplitManager::new(experiments, strategy),
            outcome_tracker: OutcomeTracker::new(max_raw_outcomes),
        }
    }

    /// Get the traffic split manager
    pub fn split_manager(&self) -> &TrafficSplitManager {
        &self.split_manager
    }

    /// Get mutable access to the traffic split manager
    pub fn split_manager_mut(&mut self) -> &mut TrafficSplitManager {
        &mut self.split_manager
    }

    /// Get the outcome tracker
    pub fn outcome_tracker(&self) -> &OutcomeTracker {
        &self.outcome_tracker
    }

    /// Assign a request to an experiment variant
    pub fn assign(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        request_id: &str,
        intent: &TaskIntent,
        features: Option<&PromptFeatures>,
    ) -> Option<VariantAssignment> {
        self.split_manager
            .assign(user_id, session_id, request_id, intent, features)
    }

    /// Record an experiment outcome
    pub fn record_outcome(&self, outcome: ExperimentOutcome) {
        self.outcome_tracker.record(outcome);
    }

    /// Get a report for an experiment
    pub fn get_report(&self, experiment_id: &str) -> Option<ExperimentReport> {
        let config = self.split_manager.get_experiment(experiment_id)?;
        let stats = self.outcome_tracker.get_stats(experiment_id)?;
        Some(ExperimentReport::generate(config, &stats))
    }

    /// Get reports for all experiments
    pub fn get_all_reports(&self) -> Vec<ExperimentReport> {
        self.split_manager
            .experiments()
            .filter_map(|config| {
                let stats = self
                    .outcome_tracker
                    .get_stats(&config.id)
                    .unwrap_or_default();
                Some(ExperimentReport::generate(config, &stats))
            })
            .collect()
    }

    /// Add an experiment
    pub fn add_experiment(&mut self, experiment: ExperimentConfig) {
        self.split_manager.add_experiment(experiment);
    }

    /// Remove an experiment
    pub fn remove_experiment(&mut self, experiment_id: &str) -> Option<ExperimentConfig> {
        self.outcome_tracker.clear_experiment(experiment_id);
        self.split_manager.remove_experiment(experiment_id)
    }

    /// Enable an experiment
    pub fn enable_experiment(&mut self, experiment_id: &str) -> bool {
        self.split_manager.enable_experiment(experiment_id)
    }

    /// Disable an experiment
    pub fn disable_experiment(&mut self, experiment_id: &str) -> bool {
        self.split_manager.disable_experiment(experiment_id)
    }

    /// Get active experiment count
    pub fn active_experiment_count(&self) -> usize {
        self.split_manager.active_experiments().count()
    }

    /// Get total experiment count
    pub fn total_experiment_count(&self) -> usize {
        self.split_manager.experiments().count()
    }
}

impl Default for ABTestingEngine {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        let one_variant = ExperimentConfig::new("test")
            .add_variant(VariantConfig::control("model-a"));
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
                .assign(None, None, &format!("req-{}", i), &TaskIntent::GeneralChat, None)
                .is_some()
            {
                in_experiment += 1;
            }
        }

        // Should be approximately 10% (within 2% tolerance = 8-12%)
        let percentage = (in_experiment as f64 / total as f64) * 100.0;
        assert!(
            percentage >= 8.0 && percentage <= 12.0,
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

        let a_pct = (*counts.get("a").unwrap_or(&0) as f64 / total as f64) * 100.0;
        let b_pct = (*counts.get("b").unwrap_or(&0) as f64 / total as f64) * 100.0;

        // Should be approximately 70/30 (within 5% tolerance)
        assert!(
            a_pct >= 65.0 && a_pct <= 75.0,
            "Expected ~70% for A, got {:.1}%",
            a_pct
        );
        assert!(
            b_pct >= 25.0 && b_pct <= 35.0,
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
        let control_stats = stats.get("control").unwrap();

        assert_eq!(control_stats.sample_count, 1);
        assert_eq!(
            control_stats.get_metric(&TrackedMetric::LatencyMs).unwrap().mean(),
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
        for v in [98.0, 99.0, 100.0, 101.0, 102.0, 99.0, 100.0, 101.0, 100.0, 99.0,
                  98.0, 99.0, 100.0, 101.0, 102.0, 99.0, 100.0, 101.0, 100.0, 99.0,
                  98.0, 99.0, 100.0, 101.0, 102.0, 99.0, 100.0, 101.0, 100.0, 99.0] {
            control.record(v);
        }

        // Treatment: mean ~110, similar variance (significant difference)
        for v in [108.0, 109.0, 110.0, 111.0, 112.0, 109.0, 110.0, 111.0, 110.0, 109.0,
                  108.0, 109.0, 110.0, 111.0, 112.0, 109.0, 110.0, 111.0, 110.0, 109.0,
                  108.0, 109.0, 110.0, 111.0, 112.0, 109.0, 110.0, 111.0, 110.0, 109.0] {
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
            if let Some(assignment) = engine.assign(None, None, &request_id, &TaskIntent::GeneralChat, None) {
                let latency = if assignment.variant_id == "control" {
                    100.0 + (i as f64 % 20.0)
                } else {
                    90.0 + (i as f64 % 20.0)
                };

                let outcome = ExperimentOutcome::new(
                    &assignment.experiment_id,
                    &assignment.variant_id,
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

        assert_eq!(report.experiment_id, "test-exp");
        assert_eq!(report.variant_summaries.len(), 2);
        assert!(report.total_samples > 0);
    }

    #[test]
    fn test_tracked_metric_parsing() {
        assert_eq!(TrackedMetric::from_str("latency_ms"), TrackedMetric::LatencyMs);
        assert_eq!(TrackedMetric::from_str("LATENCY"), TrackedMetric::LatencyMs);
        assert_eq!(TrackedMetric::from_str("cost_usd"), TrackedMetric::CostUsd);
        assert_eq!(
            TrackedMetric::from_str("custom_metric"),
            TrackedMetric::Custom("custom_metric".to_string())
        );
    }
}
