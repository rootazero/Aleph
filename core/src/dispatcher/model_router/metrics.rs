//! Runtime Metrics for Model Router
//!
//! This module provides data structures for collecting and aggregating
//! runtime metrics from AI model API calls to enable data-driven routing.

use super::TaskIntent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

// =============================================================================
// Call Record - Raw Data
// =============================================================================

/// Single API call record for metrics collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    /// Unique identifier for this call
    pub id: String,

    /// Model profile ID
    pub model_id: String,

    /// Call timestamp
    pub timestamp: SystemTime,

    /// Task intent that triggered this call
    pub intent: TaskIntent,

    /// Input token count
    pub input_tokens: u32,

    /// Output token count
    pub output_tokens: u32,

    /// Total latency (request to complete response)
    pub total_latency: Duration,

    /// Time to first token (for streaming)
    pub ttft: Option<Duration>,

    /// Call outcome
    pub outcome: CallOutcome,

    /// Actual cost in USD (if known)
    pub cost_usd: Option<f64>,

    /// User feedback (if provided)
    pub user_feedback: Option<UserFeedback>,
}

impl CallRecord {
    /// Create a new call record for a successful call
    pub fn success(
        id: impl Into<String>,
        model_id: impl Into<String>,
        intent: TaskIntent,
        input_tokens: u32,
        output_tokens: u32,
        latency: Duration,
    ) -> Self {
        Self {
            id: id.into(),
            model_id: model_id.into(),
            timestamp: SystemTime::now(),
            intent,
            input_tokens,
            output_tokens,
            total_latency: latency,
            ttft: None,
            outcome: CallOutcome::Success,
            cost_usd: None,
            user_feedback: None,
        }
    }

    /// Create a new call record for a failed call
    pub fn failure(
        id: impl Into<String>,
        model_id: impl Into<String>,
        intent: TaskIntent,
        latency: Duration,
        outcome: CallOutcome,
    ) -> Self {
        Self {
            id: id.into(),
            model_id: model_id.into(),
            timestamp: SystemTime::now(),
            intent,
            input_tokens: 0,
            output_tokens: 0,
            total_latency: latency,
            ttft: None,
            outcome,
            cost_usd: None,
            user_feedback: None,
        }
    }

    /// Set TTFT (time to first token)
    pub fn with_ttft(mut self, ttft: Duration) -> Self {
        self.ttft = Some(ttft);
        self
    }

    /// Set cost
    pub fn with_cost(mut self, cost_usd: f64) -> Self {
        self.cost_usd = Some(cost_usd);
        self
    }

    /// Check if call was successful
    pub fn is_success(&self) -> bool {
        self.outcome.is_success()
    }

    /// Get total tokens used
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

/// Call outcome type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CallOutcome {
    /// Successful completion
    Success,
    /// Request timed out
    Timeout,
    /// API error (4xx/5xx)
    ApiError {
        #[serde(default)]
        status_code: u16,
    },
    /// Rate limited (429)
    RateLimited,
    /// Content filtered by safety system
    ContentFiltered,
    /// Context length exceeded
    ContextOverflow,
    /// Network/connection error
    NetworkError,
    /// Unknown error
    #[default]
    Unknown,
}

impl CallOutcome {
    /// Check if outcome is success
    pub fn is_success(&self) -> bool {
        matches!(self, CallOutcome::Success)
    }

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            CallOutcome::Timeout | CallOutcome::RateLimited | CallOutcome::NetworkError
        )
    }

    /// Get error type string
    pub fn error_type(&self) -> &'static str {
        match self {
            CallOutcome::Success => "success",
            CallOutcome::Timeout => "timeout",
            CallOutcome::ApiError { .. } => "api_error",
            CallOutcome::RateLimited => "rate_limited",
            CallOutcome::ContentFiltered => "content_filtered",
            CallOutcome::ContextOverflow => "context_overflow",
            CallOutcome::NetworkError => "network_error",
            CallOutcome::Unknown => "unknown",
        }
    }
}


/// User feedback for quality learning
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserFeedback {
    /// Explicit positive (thumbs up)
    Positive,
    /// Explicit negative (thumbs down)
    Negative,
    /// User regenerated response (implicit negative)
    Regenerated,
    /// User edited and used (partial acceptance)
    EditedAndUsed,
    /// User used as-is (implicit positive)
    UsedAsIs,
}

impl UserFeedback {
    /// Convert to satisfaction score (0.0 - 1.0)
    pub fn to_score(&self) -> f64 {
        match self {
            UserFeedback::Positive => 1.0,
            UserFeedback::UsedAsIs => 0.8,
            UserFeedback::EditedAndUsed => 0.5,
            UserFeedback::Regenerated => 0.2,
            UserFeedback::Negative => 0.0,
        }
    }
}

// =============================================================================
// Aggregated Metrics
// =============================================================================

/// Aggregated metrics for a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetrics {
    /// Model profile ID
    pub model_id: String,

    /// Last update timestamp
    pub last_updated: SystemTime,

    /// Total call count
    pub total_calls: u64,

    /// Successful call count
    pub successful_calls: u64,

    /// Latency statistics
    pub latency: LatencyStats,

    /// TTFT statistics (if streaming)
    pub ttft: Option<LatencyStats>,

    /// Cost statistics
    pub cost: CostStats,

    /// Success rate (sliding window)
    pub success_rate: f64,

    /// Error distribution
    pub error_distribution: ErrorDistribution,

    /// Consecutive failure count
    pub consecutive_failures: u32,

    /// Rate limit state
    pub rate_limit: Option<RateLimitState>,

    /// User satisfaction score (0.0 - 1.0)
    pub satisfaction_score: Option<f64>,

    /// Per-intent performance
    #[serde(default)]
    pub intent_performance: HashMap<String, IntentMetrics>,
}

impl ModelMetrics {
    /// Create new empty metrics for a model
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            last_updated: SystemTime::now(),
            total_calls: 0,
            successful_calls: 0,
            latency: LatencyStats::default(),
            ttft: None,
            cost: CostStats::default(),
            success_rate: 1.0, // Optimistic default
            error_distribution: ErrorDistribution::default(),
            consecutive_failures: 0,
            rate_limit: None,
            satisfaction_score: None,
            intent_performance: HashMap::new(),
        }
    }

    /// Update metrics with a new call record
    pub fn update(&mut self, record: &CallRecord) {
        self.total_calls += 1;
        self.last_updated = SystemTime::now();

        if record.is_success() {
            self.successful_calls += 1;
            self.consecutive_failures = 0;

            // Update latency stats
            self.latency.update(record.total_latency.as_millis() as f64);

            // Update TTFT if present
            if let Some(ttft) = record.ttft {
                let ttft_stats = self.ttft.get_or_insert_with(LatencyStats::default);
                ttft_stats.update(ttft.as_millis() as f64);
            }

            // Update cost stats
            self.cost.total_input_tokens += record.input_tokens as u64;
            self.cost.total_output_tokens += record.output_tokens as u64;
            if let Some(cost) = record.cost_usd {
                self.cost.total_cost += cost;
            }
        } else {
            self.consecutive_failures += 1;
            self.error_distribution.record(&record.outcome);
        }

        // Update success rate
        self.success_rate = self.successful_calls as f64 / self.total_calls as f64;

        // Update cost averages
        if self.total_calls > 0 {
            self.cost.avg_cost_per_call = self.cost.total_cost / self.total_calls as f64;
        }

        // Update intent-specific metrics
        let intent_key = record.intent.to_task_type().to_string();
        let intent_metrics = self
            .intent_performance
            .entry(intent_key)
            .or_default();
        intent_metrics.update(record);

        // Update satisfaction if feedback present
        if let Some(feedback) = record.user_feedback {
            let current = self.satisfaction_score.unwrap_or(0.5);
            let new_score = feedback.to_score();
            // Exponential moving average
            self.satisfaction_score = Some(current * 0.9 + new_score * 0.1);
        }
    }

    /// Check if we have enough data for reliable scoring
    pub fn has_sufficient_data(&self) -> bool {
        self.total_calls >= 10
    }
}

impl Default for ModelMetrics {
    fn default() -> Self {
        Self::new("unknown")
    }
}

/// Latency statistics using Welford's algorithm for streaming computation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyStats {
    /// Sample count
    pub count: u64,
    /// Mean (ms)
    pub mean: f64,
    /// M2 for variance calculation (internal)
    #[serde(default)]
    m2: f64,
    /// Minimum (ms)
    pub min: f64,
    /// Maximum (ms)
    pub max: f64,
    /// P50 estimate (ms)
    pub p50: f64,
    /// P90 estimate (ms)
    pub p90: f64,
    /// P95 estimate (ms)
    pub p95: f64,
    /// P99 estimate (ms)
    pub p99: f64,
}

impl LatencyStats {
    /// Update statistics with a new sample using Welford's algorithm
    pub fn update(&mut self, value: f64) {
        self.count += 1;

        // Welford's online algorithm for mean and variance
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;

        // Update min/max
        if self.count == 1 {
            self.min = value;
            self.max = value;
        } else {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }

        // Estimate percentiles using exponential moving average
        // This is an approximation - for accurate percentiles use a sketch algorithm
        let alpha = 2.0 / (self.count as f64 + 1.0).min(100.0);
        if self.count == 1 {
            self.p50 = value;
            self.p90 = value;
            self.p95 = value;
            self.p99 = value;
        } else {
            // Adjust percentile estimates based on where value falls
            self.p50 = self.estimate_percentile(value, self.p50, 0.50, alpha);
            self.p90 = self.estimate_percentile(value, self.p90, 0.90, alpha);
            self.p95 = self.estimate_percentile(value, self.p95, 0.95, alpha);
            self.p99 = self.estimate_percentile(value, self.p99, 0.99, alpha);
        }
    }

    fn estimate_percentile(&self, value: f64, current: f64, percentile: f64, alpha: f64) -> f64 {
        // P² algorithm approximation
        if value < current {
            current - alpha * (1.0 - percentile)
        } else {
            current + alpha * percentile
        }
    }

    /// Get standard deviation
    pub fn stddev(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            (self.m2 / (self.count - 1) as f64).sqrt()
        }
    }

    /// Merge another LatencyStats into this one
    pub fn merge(&mut self, other: &LatencyStats) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = other.clone();
            return;
        }

        let combined_count = self.count + other.count;
        let delta = other.mean - self.mean;
        let combined_mean = (self.mean * self.count as f64 + other.mean * other.count as f64)
            / combined_count as f64;

        // Parallel algorithm for combining variances
        let combined_m2 = self.m2
            + other.m2
            + delta * delta * (self.count as f64 * other.count as f64) / combined_count as f64;

        self.count = combined_count;
        self.mean = combined_mean;
        self.m2 = combined_m2;
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);

        // Weighted average for percentile estimates
        let w1 = self.count as f64 / combined_count as f64;
        let w2 = other.count as f64 / combined_count as f64;
        self.p50 = self.p50 * w1 + other.p50 * w2;
        self.p90 = self.p90 * w1 + other.p90 * w2;
        self.p95 = self.p95 * w1 + other.p95 * w2;
        self.p99 = self.p99 * w1 + other.p99 * w2;
    }
}

impl Default for LatencyStats {
    fn default() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            min: f64::MAX,
            max: 0.0,
            p50: 0.0,
            p90: 0.0,
            p95: 0.0,
            p99: 0.0,
        }
    }
}

/// Cost statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostStats {
    /// Total cost (USD)
    pub total_cost: f64,
    /// Total input tokens
    pub total_input_tokens: u64,
    /// Total output tokens
    pub total_output_tokens: u64,
    /// Average cost per call
    pub avg_cost_per_call: f64,
    /// Actual input price (per 1M tokens) if measured
    pub actual_input_price: Option<f64>,
    /// Actual output price (per 1M tokens) if measured
    pub actual_output_price: Option<f64>,
}

impl CostStats {
    /// Calculate total tokens
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens
    }

    /// Update actual prices based on recorded data
    pub fn update_prices(&mut self) {
        if self.total_input_tokens > 0 && self.total_cost > 0.0 {
            // Estimate input price (assuming 3:1 output:input cost ratio typical for most models)
            let estimated_input_cost = self.total_cost * 0.25; // 25% to input
            self.actual_input_price =
                Some(estimated_input_cost / (self.total_input_tokens as f64 / 1_000_000.0));
        }
        if self.total_output_tokens > 0 && self.total_cost > 0.0 {
            let estimated_output_cost = self.total_cost * 0.75; // 75% to output
            self.actual_output_price =
                Some(estimated_output_cost / (self.total_output_tokens as f64 / 1_000_000.0));
        }
    }
}

/// Error distribution tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorDistribution {
    pub timeout_count: u32,
    pub rate_limit_count: u32,
    pub api_error_count: u32,
    pub network_error_count: u32,
    pub content_filter_count: u32,
    pub context_overflow_count: u32,
    pub other_count: u32,
}

impl ErrorDistribution {
    /// Record an error
    pub fn record(&mut self, outcome: &CallOutcome) {
        match outcome {
            CallOutcome::Success => {}
            CallOutcome::Timeout => self.timeout_count += 1,
            CallOutcome::RateLimited => self.rate_limit_count += 1,
            CallOutcome::ApiError { .. } => self.api_error_count += 1,
            CallOutcome::NetworkError => self.network_error_count += 1,
            CallOutcome::ContentFiltered => self.content_filter_count += 1,
            CallOutcome::ContextOverflow => self.context_overflow_count += 1,
            CallOutcome::Unknown => self.other_count += 1,
        }
    }

    /// Get total error count
    pub fn total(&self) -> u32 {
        self.timeout_count
            + self.rate_limit_count
            + self.api_error_count
            + self.network_error_count
            + self.content_filter_count
            + self.context_overflow_count
            + self.other_count
    }

    /// Get most common error type
    pub fn most_common(&self) -> Option<&'static str> {
        let counts = [
            (self.timeout_count, "timeout"),
            (self.rate_limit_count, "rate_limited"),
            (self.api_error_count, "api_error"),
            (self.network_error_count, "network_error"),
            (self.content_filter_count, "content_filtered"),
            (self.context_overflow_count, "context_overflow"),
            (self.other_count, "other"),
        ];

        counts
            .iter()
            .filter(|(count, _)| *count > 0)
            .max_by_key(|(count, _)| *count)
            .map(|(_, name)| *name)
    }
}

/// Rate limit state from API headers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitState {
    /// Requests limit
    pub requests_limit: Option<u32>,
    /// Requests remaining
    pub requests_remaining: Option<u32>,
    /// Requests reset time
    pub requests_reset: Option<SystemTime>,
    /// Tokens limit
    pub tokens_limit: Option<u64>,
    /// Tokens remaining
    pub tokens_remaining: Option<u64>,
    /// Tokens reset time
    pub tokens_reset: Option<SystemTime>,
}

impl RateLimitState {
    /// Calculate remaining capacity as percentage (0.0 - 1.0)
    pub fn remaining_capacity(&self) -> f64 {
        match (self.requests_remaining, self.requests_limit) {
            (Some(remaining), Some(limit)) if limit > 0 => remaining as f64 / limit as f64,
            _ => 1.0, // Unknown = assume full capacity
        }
    }

    /// Check if currently rate limited
    pub fn is_limited(&self) -> bool {
        self.requests_remaining == Some(0) || self.tokens_remaining == Some(0)
    }

    /// Check if reset time has passed
    pub fn is_reset(&self) -> bool {
        if let Some(reset) = self.requests_reset {
            SystemTime::now() > reset
        } else {
            false
        }
    }
}

/// Per-intent metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntentMetrics {
    /// Call count for this intent
    pub calls: u64,
    /// Success rate
    pub success_rate: f64,
    /// Average latency (ms)
    pub avg_latency_ms: f64,
    /// Satisfaction score
    pub satisfaction_score: Option<f64>,
}

impl IntentMetrics {
    /// Update with a new call record
    pub fn update(&mut self, record: &CallRecord) {
        let old_count = self.calls;
        self.calls += 1;

        if record.is_success() {
            // Update success rate
            let old_successes = (self.success_rate * old_count as f64).round() as u64;
            self.success_rate = (old_successes + 1) as f64 / self.calls as f64;

            // Update average latency
            self.avg_latency_ms = (self.avg_latency_ms * old_count as f64
                + record.total_latency.as_millis() as f64)
                / self.calls as f64;
        } else {
            let old_successes = (self.success_rate * old_count as f64).round() as u64;
            self.success_rate = old_successes as f64 / self.calls as f64;
        }

        // Update satisfaction
        if let Some(feedback) = record.user_feedback {
            let current = self.satisfaction_score.unwrap_or(0.5);
            let new_score = feedback.to_score();
            self.satisfaction_score = Some(current * 0.9 + new_score * 0.1);
        }
    }
}

// =============================================================================
// Multi-Window Metrics
// =============================================================================

/// Time window configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Short-term window (for spike detection)
    pub short_term: Duration,
    /// Medium-term window (for routing decisions)
    pub medium_term: Duration,
    /// Long-term window (for trend analysis)
    pub long_term: Duration,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            short_term: Duration::from_secs(5 * 60),      // 5 minutes
            medium_term: Duration::from_secs(60 * 60),    // 1 hour
            long_term: Duration::from_secs(24 * 60 * 60), // 24 hours
        }
    }
}

/// Multi-window metrics view
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiWindowMetrics {
    /// Short-term metrics (most sensitive)
    pub short_term: ModelMetrics,
    /// Medium-term metrics (main routing basis)
    pub medium_term: ModelMetrics,
    /// Long-term metrics (trend reference)
    pub long_term: ModelMetrics,
    /// All-time metrics (historical total)
    pub all_time: ModelMetrics,
}

impl MultiWindowMetrics {
    /// Create new multi-window metrics for a model
    pub fn new(model_id: &str) -> Self {
        Self {
            short_term: ModelMetrics::new(model_id),
            medium_term: ModelMetrics::new(model_id),
            long_term: ModelMetrics::new(model_id),
            all_time: ModelMetrics::new(model_id),
        }
    }

    /// Update all windows with a new record
    pub fn update(&mut self, record: &CallRecord) {
        self.short_term.update(record);
        self.medium_term.update(record);
        self.long_term.update(record);
        self.all_time.update(record);
    }

    /// Get the model ID
    pub fn model_id(&self) -> &str {
        &self.all_time.model_id
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_call_record_success() {
        let record = CallRecord::success(
            "call-1",
            "claude-sonnet",
            TaskIntent::CodeGeneration,
            100,
            200,
            Duration::from_millis(1500),
        );

        assert!(record.is_success());
        assert_eq!(record.total_tokens(), 300);
        assert_eq!(record.model_id, "claude-sonnet");
    }

    #[test]
    fn test_call_record_failure() {
        let record = CallRecord::failure(
            "call-2",
            "claude-sonnet",
            TaskIntent::CodeGeneration,
            Duration::from_millis(5000),
            CallOutcome::Timeout,
        );

        assert!(!record.is_success());
        assert_eq!(record.outcome, CallOutcome::Timeout);
    }

    #[test]
    fn test_call_outcome_retryable() {
        assert!(CallOutcome::Timeout.is_retryable());
        assert!(CallOutcome::RateLimited.is_retryable());
        assert!(CallOutcome::NetworkError.is_retryable());
        assert!(!CallOutcome::Success.is_retryable());
        assert!(!CallOutcome::ContentFiltered.is_retryable());
    }

    #[test]
    fn test_user_feedback_score() {
        assert_eq!(UserFeedback::Positive.to_score(), 1.0);
        assert_eq!(UserFeedback::Negative.to_score(), 0.0);
        assert!(UserFeedback::EditedAndUsed.to_score() > UserFeedback::Regenerated.to_score());
    }

    #[test]
    fn test_latency_stats_update() {
        let mut stats = LatencyStats::default();

        stats.update(100.0);
        assert_eq!(stats.count, 1);
        assert_eq!(stats.mean, 100.0);
        assert_eq!(stats.min, 100.0);
        assert_eq!(stats.max, 100.0);

        stats.update(200.0);
        assert_eq!(stats.count, 2);
        assert_eq!(stats.mean, 150.0);
        assert_eq!(stats.min, 100.0);
        assert_eq!(stats.max, 200.0);

        stats.update(150.0);
        assert_eq!(stats.count, 3);
        assert!((stats.mean - 150.0).abs() < 0.01);
    }

    #[test]
    fn test_latency_stats_stddev() {
        let mut stats = LatencyStats::default();

        // Add values with known variance
        for v in [10.0, 20.0, 30.0, 40.0, 50.0] {
            stats.update(v);
        }

        // Mean should be 30
        assert!((stats.mean - 30.0).abs() < 0.01);

        // Stddev should be ~15.81
        let stddev = stats.stddev();
        assert!(stddev > 15.0 && stddev < 16.0);
    }

    #[test]
    fn test_model_metrics_update() {
        let mut metrics = ModelMetrics::new("test-model");

        // Add successful call
        let success = CallRecord::success(
            "1",
            "test-model",
            TaskIntent::CodeGeneration,
            100,
            200,
            Duration::from_millis(1000),
        );
        metrics.update(&success);

        assert_eq!(metrics.total_calls, 1);
        assert_eq!(metrics.successful_calls, 1);
        assert_eq!(metrics.success_rate, 1.0);
        assert_eq!(metrics.consecutive_failures, 0);

        // Add failed call
        let failure = CallRecord::failure(
            "2",
            "test-model",
            TaskIntent::CodeGeneration,
            Duration::from_millis(5000),
            CallOutcome::Timeout,
        );
        metrics.update(&failure);

        assert_eq!(metrics.total_calls, 2);
        assert_eq!(metrics.successful_calls, 1);
        assert_eq!(metrics.success_rate, 0.5);
        assert_eq!(metrics.consecutive_failures, 1);
        assert_eq!(metrics.error_distribution.timeout_count, 1);
    }

    #[test]
    fn test_error_distribution() {
        let mut dist = ErrorDistribution::default();

        dist.record(&CallOutcome::Timeout);
        dist.record(&CallOutcome::Timeout);
        dist.record(&CallOutcome::RateLimited);

        assert_eq!(dist.timeout_count, 2);
        assert_eq!(dist.rate_limit_count, 1);
        assert_eq!(dist.total(), 3);
        assert_eq!(dist.most_common(), Some("timeout"));
    }

    #[test]
    fn test_rate_limit_state() {
        let state = RateLimitState {
            requests_limit: Some(100),
            requests_remaining: Some(20),
            requests_reset: None,
            tokens_limit: None,
            tokens_remaining: None,
            tokens_reset: None,
        };

        assert!((state.remaining_capacity() - 0.2).abs() < 0.01);
        assert!(!state.is_limited());

        let limited = RateLimitState {
            requests_limit: Some(100),
            requests_remaining: Some(0),
            requests_reset: Some(SystemTime::now() + Duration::from_secs(60)),
            tokens_limit: None,
            tokens_remaining: None,
            tokens_reset: None,
        };

        assert!(limited.is_limited());
    }

    #[test]
    fn test_multi_window_metrics() {
        let mut metrics = MultiWindowMetrics::new("test-model");

        let record = CallRecord::success(
            "1",
            "test-model",
            TaskIntent::CodeGeneration,
            100,
            200,
            Duration::from_millis(1000),
        );

        metrics.update(&record);

        assert_eq!(metrics.short_term.total_calls, 1);
        assert_eq!(metrics.medium_term.total_calls, 1);
        assert_eq!(metrics.long_term.total_calls, 1);
        assert_eq!(metrics.all_time.total_calls, 1);
    }

    #[test]
    fn test_intent_metrics() {
        let mut metrics = IntentMetrics::default();

        let success = CallRecord::success(
            "1",
            "test",
            TaskIntent::CodeGeneration,
            100,
            200,
            Duration::from_millis(1000),
        );
        metrics.update(&success);

        assert_eq!(metrics.calls, 1);
        assert_eq!(metrics.success_rate, 1.0);
        assert_eq!(metrics.avg_latency_ms, 1000.0);

        let failure = CallRecord::failure(
            "2",
            "test",
            TaskIntent::CodeGeneration,
            Duration::from_millis(5000),
            CallOutcome::Timeout,
        );
        metrics.update(&failure);

        assert_eq!(metrics.calls, 2);
        assert_eq!(metrics.success_rate, 0.5);
    }

    #[test]
    fn test_latency_stats_merge() {
        let mut stats1 = LatencyStats::default();
        stats1.update(100.0);
        stats1.update(200.0);

        let mut stats2 = LatencyStats::default();
        stats2.update(300.0);
        stats2.update(400.0);

        stats1.merge(&stats2);

        assert_eq!(stats1.count, 4);
        assert!((stats1.mean - 250.0).abs() < 0.01);
        assert_eq!(stats1.min, 100.0);
        assert_eq!(stats1.max, 400.0);
    }
}
