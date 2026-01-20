//! Health Check Data Structures
//!
//! This module defines data structures for tracking AI model health status,
//! implementing circuit breaker pattern, and managing health state transitions.

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};

// ============================================================================
// Health Status
// ============================================================================

/// Health status of an AI model
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[derive(Default)]
pub enum HealthStatus {
    /// Normal service - model is fully available
    Healthy,
    /// Available but impaired (high latency, near rate limit)
    Degraded,
    /// Temporarily unavailable due to failures
    Unhealthy,
    /// Circuit breaker triggered - blocking all requests
    CircuitOpen,
    /// Half-open state - allowing limited test requests
    HalfOpen,
    /// Insufficient data to determine health
    #[default]
    Unknown,
}


impl HealthStatus {
    /// Check if the model can accept requests
    pub fn can_call(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded | Self::Unknown)
    }

    /// Check if the model can accept recovery test requests
    pub fn can_call_for_recovery(&self) -> bool {
        matches!(self, Self::HalfOpen)
    }

    /// Get status priority for sorting (lower is better)
    pub fn priority(&self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Degraded => 1,
            Self::Unknown => 2,
            Self::HalfOpen => 3,
            Self::Unhealthy => 4,
            Self::CircuitOpen => 5,
        }
    }

    /// Get human-readable status text
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "Healthy",
            Self::Degraded => "Degraded",
            Self::Unhealthy => "Unhealthy",
            Self::CircuitOpen => "Circuit Open",
            Self::HalfOpen => "Half Open",
            Self::Unknown => "Unknown",
        }
    }

    /// Get status emoji for display
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Healthy => "✅",
            Self::Degraded => "⚠️",
            Self::Unhealthy => "❌",
            Self::CircuitOpen => "🔴",
            Self::HalfOpen => "🟡",
            Self::Unknown => "❓",
        }
    }
}

// ============================================================================
// Degradation and Unhealthy Reasons
// ============================================================================

/// Reason why a model is in degraded state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DegradationReason {
    /// High latency detected
    HighLatency {
        current_p95_ms: f64,
        threshold_ms: f64,
    },
    /// Partial errors occurring
    PartialErrors { error_rate: f64, threshold: f64 },
    /// Near rate limit
    NearRateLimit { remaining_percent: f64 },
    /// Manually set to degraded
    ManualOverride { reason: String },
}

impl DegradationReason {
    /// Get human-readable description
    pub fn description(&self) -> String {
        match self {
            Self::HighLatency {
                current_p95_ms,
                threshold_ms,
            } => {
                format!(
                    "High latency: p95 {:.0}ms (threshold: {:.0}ms)",
                    current_p95_ms, threshold_ms
                )
            }
            Self::PartialErrors {
                error_rate,
                threshold,
            } => {
                format!(
                    "Partial errors: {:.1}% (threshold: {:.1}%)",
                    error_rate * 100.0,
                    threshold * 100.0
                )
            }
            Self::NearRateLimit { remaining_percent } => {
                format!(
                    "Near rate limit: {:.1}% remaining",
                    remaining_percent * 100.0
                )
            }
            Self::ManualOverride { reason } => {
                format!("Manual override: {}", reason)
            }
        }
    }
}

/// Reason why a model is unhealthy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnhealthyReason {
    /// Too many consecutive failures
    ConsecutiveFailures { count: u32, threshold: u32 },
    /// High error rate
    HighErrorRate { rate: f64, threshold: f64 },
    /// Rate limited by the API
    RateLimited { reset_at: Option<SystemTime> },
    /// API endpoint unreachable
    ApiUnreachable { last_error: String },
    /// Authentication failed
    AuthenticationFailed,
    /// Quota exhausted
    QuotaExhausted,
    /// Manually disabled
    ManuallyDisabled { reason: String },
}

impl UnhealthyReason {
    /// Get human-readable description
    pub fn description(&self) -> String {
        match self {
            Self::ConsecutiveFailures { count, threshold } => {
                format!("Consecutive failures: {} (threshold: {})", count, threshold)
            }
            Self::HighErrorRate { rate, threshold } => {
                format!(
                    "High error rate: {:.1}% (threshold: {:.1}%)",
                    rate * 100.0,
                    threshold * 100.0
                )
            }
            Self::RateLimited { reset_at } => {
                if let Some(reset) = reset_at {
                    if let Ok(duration) = reset.duration_since(SystemTime::now()) {
                        format!("Rate limited, resets in {}s", duration.as_secs())
                    } else {
                        "Rate limited".to_string()
                    }
                } else {
                    "Rate limited".to_string()
                }
            }
            Self::ApiUnreachable { last_error } => {
                format!("API unreachable: {}", last_error)
            }
            Self::AuthenticationFailed => "Authentication failed".to_string(),
            Self::QuotaExhausted => "Quota exhausted".to_string(),
            Self::ManuallyDisabled { reason } => {
                format!("Manually disabled: {}", reason)
            }
        }
    }
}

// ============================================================================
// Circuit Breaker
// ============================================================================

/// State of the circuit breaker
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum CircuitState {
    /// Circuit is closed, normal operation
    #[default]
    Closed,
    /// Circuit is open, blocking requests
    Open,
    /// Circuit is half-open, allowing test requests
    HalfOpen,
}


/// Circuit breaker state for a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerState {
    /// Current circuit state
    pub state: CircuitState,
    /// Number of failures in the current window
    pub failure_count: u32,
    /// When the circuit was opened
    pub opened_at: Option<SystemTime>,
    /// When the next attempt is allowed
    pub next_attempt_at: Option<SystemTime>,
    /// Number of consecutive successes in half-open state
    pub half_open_successes: u32,
    /// Number of times the circuit has opened (for exponential backoff)
    pub open_count: u32,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            opened_at: None,
            next_attempt_at: None,
            half_open_successes: 0,
            open_count: 0,
        }
    }
}

impl CircuitBreakerState {
    /// Create a new circuit breaker in closed state
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the circuit allows requests
    pub fn allows_request(&self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if cooldown has elapsed
                if let Some(next_attempt) = self.next_attempt_at {
                    SystemTime::now() >= next_attempt
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Calculate cooldown duration with exponential backoff
    pub fn calculate_cooldown(&self, base_cooldown_secs: u64) -> Duration {
        // Exponential backoff: base × 2^(min(open_count, 5))
        // Max: base × 32 (e.g., 30s × 32 = 960s = 16 minutes)
        let exponent = self.open_count.min(5);
        let multiplier = 2u64.pow(exponent);
        Duration::from_secs(base_cooldown_secs * multiplier)
    }
}

// ============================================================================
// Rate Limit Info
// ============================================================================

/// Rate limit information from API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitInfo {
    /// Remaining requests in current window
    pub remaining: u32,
    /// Total limit for current window
    pub limit: u32,
    /// When the rate limit resets
    pub reset_at: Option<SystemTime>,
    /// Last updated time
    pub updated_at: SystemTime,
}

impl RateLimitInfo {
    /// Create new rate limit info
    pub fn new(remaining: u32, limit: u32, reset_at: Option<SystemTime>) -> Self {
        Self {
            remaining,
            limit,
            reset_at,
            updated_at: SystemTime::now(),
        }
    }

    /// Get remaining percentage (0.0 to 1.0)
    pub fn remaining_percent(&self) -> f64 {
        if self.limit == 0 {
            1.0
        } else {
            self.remaining as f64 / self.limit as f64
        }
    }

    /// Check if rate limit is exhausted
    pub fn is_exhausted(&self) -> bool {
        self.remaining == 0
    }
}

// ============================================================================
// Health Errors
// ============================================================================

/// Type of error that occurred
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorType {
    /// Network/connection error
    Network,
    /// Request timeout
    Timeout,
    /// Rate limit error (429)
    RateLimit,
    /// Authentication error (401, 403)
    Authentication,
    /// Server error (5xx)
    ServerError,
    /// Bad request (4xx)
    ClientError,
    /// Model overloaded
    Overloaded,
    /// Unknown error
    Unknown,
}

impl ErrorType {
    /// Check if error is transient (likely to recover)
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Network | Self::Timeout | Self::RateLimit | Self::ServerError | Self::Overloaded
        )
    }

    /// Check if error indicates a permanent issue
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Authentication | Self::ClientError)
    }
}

/// A recorded health error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthError {
    /// Type of error
    pub error_type: ErrorType,
    /// Error message
    pub message: String,
    /// HTTP status code if available
    pub status_code: Option<u16>,
    /// When the error occurred
    pub timestamp: SystemTime,
}

impl HealthError {
    /// Create a new health error
    pub fn new(error_type: ErrorType, message: impl Into<String>) -> Self {
        Self {
            error_type,
            message: message.into(),
            status_code: None,
            timestamp: SystemTime::now(),
        }
    }

    /// Create health error with status code
    pub fn with_status(
        error_type: ErrorType,
        message: impl Into<String>,
        status_code: u16,
    ) -> Self {
        Self {
            error_type,
            message: message.into(),
            status_code: Some(status_code),
            timestamp: SystemTime::now(),
        }
    }

    /// Create from HTTP status code
    pub fn from_status_code(status_code: u16, message: impl Into<String>) -> Self {
        let error_type = match status_code {
            401 | 403 => ErrorType::Authentication,
            429 => ErrorType::RateLimit,
            500..=599 => ErrorType::ServerError,
            400..=499 => ErrorType::ClientError,
            _ => ErrorType::Unknown,
        };
        Self::with_status(error_type, message, status_code)
    }
}

// ============================================================================
// Model Health
// ============================================================================

/// Complete health state for a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHealth {
    /// Model identifier
    pub model_id: String,
    /// Current health status
    pub status: HealthStatus,
    /// Why the model is degraded (if applicable)
    pub degradation_reason: Option<DegradationReason>,
    /// Why the model is unhealthy (if applicable)
    pub unhealthy_reason: Option<UnhealthyReason>,
    /// When the current status started
    pub status_since: SystemTime,
    /// Last successful call time
    pub last_success: Option<SystemTime>,
    /// Last failed call time
    pub last_failure: Option<SystemTime>,
    /// Consecutive successful calls
    pub consecutive_successes: u32,
    /// Consecutive failed calls
    pub consecutive_failures: u32,
    /// Rate limit information
    pub rate_limit: Option<RateLimitInfo>,
    /// Circuit breaker state
    pub circuit_breaker: CircuitBreakerState,
    /// Recent errors (capped list)
    pub recent_errors: Vec<HealthError>,
}

impl ModelHealth {
    /// Create a new model health in unknown state
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            status: HealthStatus::Unknown,
            degradation_reason: None,
            unhealthy_reason: None,
            status_since: SystemTime::now(),
            last_success: None,
            last_failure: None,
            consecutive_successes: 0,
            consecutive_failures: 0,
            rate_limit: None,
            circuit_breaker: CircuitBreakerState::new(),
            recent_errors: Vec::new(),
        }
    }

    /// Check if the model can accept requests
    pub fn can_call(&self) -> CallPermission {
        if self.status.can_call() {
            CallPermission::Allowed
        } else if self.status.can_call_for_recovery() || self.circuit_breaker.allows_request() {
            CallPermission::AllowedForRecovery
        } else {
            CallPermission::Blocked {
                reason: self.block_reason(),
            }
        }
    }

    /// Get the reason why calls are blocked
    fn block_reason(&self) -> String {
        match self.status {
            HealthStatus::Unhealthy => {
                if let Some(ref reason) = self.unhealthy_reason {
                    reason.description()
                } else {
                    "Model is unhealthy".to_string()
                }
            }
            HealthStatus::CircuitOpen => {
                if let Some(next) = self.circuit_breaker.next_attempt_at {
                    if let Ok(duration) = next.duration_since(SystemTime::now()) {
                        format!("Circuit open, retry in {}s", duration.as_secs())
                    } else {
                        "Circuit open".to_string()
                    }
                } else {
                    "Circuit open".to_string()
                }
            }
            _ => "Unknown reason".to_string(),
        }
    }

    /// Record a successful call
    pub fn record_success(&mut self) {
        self.last_success = Some(SystemTime::now());
        self.consecutive_successes += 1;
        self.consecutive_failures = 0;
    }

    /// Record a failed call
    pub fn record_failure(&mut self, error: HealthError) {
        self.last_failure = Some(SystemTime::now());
        self.consecutive_failures += 1;
        self.consecutive_successes = 0;

        // Keep only recent errors (max 10)
        self.recent_errors.push(error);
        if self.recent_errors.len() > 10 {
            self.recent_errors.remove(0);
        }
    }

    /// Update rate limit info
    pub fn update_rate_limit(&mut self, info: RateLimitInfo) {
        self.rate_limit = Some(info);
    }

    /// Set status with reason tracking
    pub fn set_status(&mut self, new_status: HealthStatus) {
        if self.status != new_status {
            self.status = new_status;
            self.status_since = SystemTime::now();

            // Clear reasons when transitioning to healthy
            if new_status == HealthStatus::Healthy {
                self.degradation_reason = None;
                self.unhealthy_reason = None;
            }
        }
    }

    /// Get a summary for display
    pub fn summary(&self) -> ModelHealthSummary {
        ModelHealthSummary {
            model_id: self.model_id.clone(),
            status: self.status,
            status_text: self.status.as_str().to_string(),
            status_emoji: self.status.emoji().to_string(),
            reason: self
                .unhealthy_reason
                .as_ref()
                .map(|r| r.description())
                .or_else(|| self.degradation_reason.as_ref().map(|r| r.description())),
            consecutive_successes: self.consecutive_successes,
            consecutive_failures: self.consecutive_failures,
        }
    }
}

/// Permission to call a model
#[derive(Debug, Clone, PartialEq)]
pub enum CallPermission {
    /// Normal call allowed
    Allowed,
    /// Only recovery test call allowed
    AllowedForRecovery,
    /// Calls blocked
    Blocked { reason: String },
}

/// Summary of model health for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHealthSummary {
    /// Model identifier
    pub model_id: String,
    /// Current status
    pub status: HealthStatus,
    /// Status as text
    pub status_text: String,
    /// Status emoji
    pub status_emoji: String,
    /// Reason for current status
    pub reason: Option<String>,
    /// Consecutive successes
    pub consecutive_successes: u32,
    /// Consecutive failures
    pub consecutive_failures: u32,
}

// ============================================================================
// Health Events
// ============================================================================

/// Events emitted by the health system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthEvent {
    /// Health status changed
    StatusChanged {
        model_id: String,
        old_status: HealthStatus,
        new_status: HealthStatus,
        reason: Option<String>,
    },
    /// Circuit breaker opened
    CircuitOpened {
        model_id: String,
        failure_count: u32,
        cooldown_secs: u64,
    },
    /// Circuit breaker closed
    CircuitClosed { model_id: String },
    /// Rate limit warning
    RateLimitWarning {
        model_id: String,
        remaining_percent: f64,
        reset_at: Option<SystemTime>,
    },
}

impl HealthEvent {
    /// Get the model ID for this event
    pub fn model_id(&self) -> &str {
        match self {
            Self::StatusChanged { model_id, .. } => model_id,
            Self::CircuitOpened { model_id, .. } => model_id,
            Self::CircuitClosed { model_id } => model_id,
            Self::RateLimitWarning { model_id, .. } => model_id,
        }
    }

    /// Get a description of the event
    pub fn description(&self) -> String {
        match self {
            Self::StatusChanged {
                model_id,
                old_status,
                new_status,
                reason,
            } => {
                let reason_str = reason
                    .as_ref()
                    .map(|r| format!(": {}", r))
                    .unwrap_or_default();
                format!(
                    "{}: {} → {}{}",
                    model_id,
                    old_status.as_str(),
                    new_status.as_str(),
                    reason_str
                )
            }
            Self::CircuitOpened {
                model_id,
                failure_count,
                cooldown_secs,
            } => {
                format!(
                    "{}: Circuit opened after {} failures, cooldown {}s",
                    model_id, failure_count, cooldown_secs
                )
            }
            Self::CircuitClosed { model_id } => {
                format!("{}: Circuit closed", model_id)
            }
            Self::RateLimitWarning {
                model_id,
                remaining_percent,
                ..
            } => {
                format!(
                    "{}: Rate limit warning, {:.1}% remaining",
                    model_id,
                    remaining_percent * 100.0
                )
            }
        }
    }
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the health system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Enable health tracking
    pub enabled: bool,
    /// Enable active probing of unhealthy models
    pub active_probing: bool,
    /// Number of consecutive failures to mark unhealthy
    pub failure_threshold: u32,
    /// Number of successes to recover from unhealthy
    pub recovery_successes: u32,
    /// Number of successes to recover from degraded
    pub degraded_recovery_successes: u32,
    /// Latency threshold (p95 ms) to mark as degraded
    pub latency_degradation_threshold_ms: u64,
    /// Latency threshold (p95 ms) to recover from degraded
    pub latency_healthy_threshold_ms: u64,
    /// Rate limit remaining percentage to trigger warning
    pub rate_limit_warning_threshold: f64,
    /// Circuit breaker configuration
    pub circuit_breaker: CircuitBreakerConfig,
    /// Probe configuration (if active probing enabled)
    pub probe: ProbeConfig,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            active_probing: false,
            failure_threshold: 3,
            recovery_successes: 2,
            degraded_recovery_successes: 3,
            latency_degradation_threshold_ms: 10000,
            latency_healthy_threshold_ms: 5000,
            rate_limit_warning_threshold: 0.2,
            circuit_breaker: CircuitBreakerConfig::default(),
            probe: ProbeConfig::default(),
        }
    }
}

/// Circuit breaker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Number of failures to open circuit
    pub failure_threshold: u32,
    /// Window in seconds for counting failures
    pub window_secs: u64,
    /// Base cooldown in seconds before half-open
    pub cooldown_secs: u64,
    /// Number of successes in half-open to close circuit
    pub half_open_successes: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            window_secs: 60,
            cooldown_secs: 30,
            half_open_successes: 2,
        }
    }
}

/// Health probe configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeConfig {
    /// Interval between probes in seconds
    pub interval_secs: u64,
    /// Timeout for probe requests
    pub timeout_secs: u64,
    /// Minimal test prompt
    pub test_prompt: String,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            interval_secs: 30,
            timeout_secs: 10,
            test_prompt: "Hi".to_string(),
        }
    }
}

/// Endpoint configuration for probing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct ProbeEndpoint {
    /// Dedicated health check URL (if available)
    pub health_url: Option<String>,
    /// Custom test prompt for this model
    pub test_prompt: Option<String>,
    /// Custom timeout for this model
    pub timeout: Option<Duration>,
}


// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_can_call() {
        assert!(HealthStatus::Healthy.can_call());
        assert!(HealthStatus::Degraded.can_call());
        assert!(HealthStatus::Unknown.can_call());
        assert!(!HealthStatus::Unhealthy.can_call());
        assert!(!HealthStatus::CircuitOpen.can_call());
        assert!(!HealthStatus::HalfOpen.can_call());
    }

    #[test]
    fn test_health_status_priority() {
        assert!(HealthStatus::Healthy.priority() < HealthStatus::Degraded.priority());
        assert!(HealthStatus::Degraded.priority() < HealthStatus::Unknown.priority());
        assert!(HealthStatus::Unknown.priority() < HealthStatus::HalfOpen.priority());
        assert!(HealthStatus::HalfOpen.priority() < HealthStatus::Unhealthy.priority());
        assert!(HealthStatus::Unhealthy.priority() < HealthStatus::CircuitOpen.priority());
    }

    #[test]
    fn test_circuit_breaker_cooldown() {
        let mut cb = CircuitBreakerState::new();

        // First open: 30s
        cb.open_count = 1;
        assert_eq!(cb.calculate_cooldown(30).as_secs(), 60);

        // Second open: 60s
        cb.open_count = 2;
        assert_eq!(cb.calculate_cooldown(30).as_secs(), 120);

        // Max backoff: 30 * 32 = 960s
        cb.open_count = 10;
        assert_eq!(cb.calculate_cooldown(30).as_secs(), 960);
    }

    #[test]
    fn test_model_health_record_success() {
        let mut health = ModelHealth::new("test-model");
        assert_eq!(health.consecutive_successes, 0);
        assert_eq!(health.consecutive_failures, 0);

        health.record_success();
        assert_eq!(health.consecutive_successes, 1);
        assert!(health.last_success.is_some());

        health.record_success();
        assert_eq!(health.consecutive_successes, 2);
    }

    #[test]
    fn test_model_health_record_failure() {
        let mut health = ModelHealth::new("test-model");
        health.record_success();
        health.record_success();
        assert_eq!(health.consecutive_successes, 2);

        let error = HealthError::new(ErrorType::ServerError, "Internal error");
        health.record_failure(error);

        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.consecutive_successes, 0);
        assert_eq!(health.recent_errors.len(), 1);
    }

    #[test]
    fn test_model_health_recent_errors_capped() {
        let mut health = ModelHealth::new("test-model");

        for i in 0..15 {
            let error = HealthError::new(ErrorType::ServerError, format!("Error {}", i));
            health.record_failure(error);
        }

        assert_eq!(health.recent_errors.len(), 10);
        assert!(health.recent_errors[0].message.contains("5")); // Oldest is error 5
    }

    #[test]
    fn test_rate_limit_info() {
        let info = RateLimitInfo::new(50, 100, None);
        assert_eq!(info.remaining_percent(), 0.5);
        assert!(!info.is_exhausted());

        let exhausted = RateLimitInfo::new(0, 100, None);
        assert!(exhausted.is_exhausted());
        assert_eq!(exhausted.remaining_percent(), 0.0);
    }

    #[test]
    fn test_error_type_transient() {
        assert!(ErrorType::Network.is_transient());
        assert!(ErrorType::Timeout.is_transient());
        assert!(ErrorType::RateLimit.is_transient());
        assert!(ErrorType::ServerError.is_transient());
        assert!(!ErrorType::Authentication.is_transient());
        assert!(!ErrorType::ClientError.is_transient());
    }

    #[test]
    fn test_health_error_from_status_code() {
        let e401 = HealthError::from_status_code(401, "Unauthorized");
        assert_eq!(e401.error_type, ErrorType::Authentication);

        let e429 = HealthError::from_status_code(429, "Rate limited");
        assert_eq!(e429.error_type, ErrorType::RateLimit);

        let e500 = HealthError::from_status_code(500, "Server error");
        assert_eq!(e500.error_type, ErrorType::ServerError);
    }

    #[test]
    fn test_degradation_reason_description() {
        let reason = DegradationReason::HighLatency {
            current_p95_ms: 15000.0,
            threshold_ms: 10000.0,
        };
        assert!(reason.description().contains("15000"));

        let reason = DegradationReason::NearRateLimit {
            remaining_percent: 0.15,
        };
        assert!(reason.description().contains("15.0%"));
    }

    #[test]
    fn test_unhealthy_reason_description() {
        let reason = UnhealthyReason::ConsecutiveFailures {
            count: 5,
            threshold: 3,
        };
        let desc = reason.description();
        assert!(desc.contains("5"));
        assert!(desc.contains("3"));

        let reason = UnhealthyReason::AuthenticationFailed;
        assert!(reason.description().contains("Authentication"));
    }

    #[test]
    fn test_health_event_description() {
        let event = HealthEvent::StatusChanged {
            model_id: "gpt-4".to_string(),
            old_status: HealthStatus::Healthy,
            new_status: HealthStatus::Degraded,
            reason: Some("High latency".to_string()),
        };
        let desc = event.description();
        assert!(desc.contains("gpt-4"));
        assert!(desc.contains("Healthy"));
        assert!(desc.contains("Degraded"));

        let event = HealthEvent::CircuitOpened {
            model_id: "gpt-4".to_string(),
            failure_count: 5,
            cooldown_secs: 30,
        };
        assert!(event.description().contains("Circuit opened"));
    }

    #[test]
    fn test_call_permission() {
        let mut health = ModelHealth::new("test");
        health.status = HealthStatus::Healthy;
        assert_eq!(health.can_call(), CallPermission::Allowed);

        // CircuitOpen with future next_attempt_at should be blocked
        health.status = HealthStatus::CircuitOpen;
        health.circuit_breaker.state = CircuitState::Open;
        health.circuit_breaker.next_attempt_at =
            Some(SystemTime::now() + std::time::Duration::from_secs(60));
        match health.can_call() {
            CallPermission::Blocked { .. } => {}
            _ => panic!("Expected Blocked"),
        }

        // CircuitOpen with past next_attempt_at should allow recovery
        health.circuit_breaker.next_attempt_at =
            Some(SystemTime::now() - std::time::Duration::from_secs(1));
        assert_eq!(health.can_call(), CallPermission::AllowedForRecovery);

        health.status = HealthStatus::HalfOpen;
        assert_eq!(health.can_call(), CallPermission::AllowedForRecovery);
    }

    #[test]
    fn test_model_health_summary() {
        let mut health = ModelHealth::new("gpt-4");
        health.status = HealthStatus::Degraded;
        health.degradation_reason = Some(DegradationReason::HighLatency {
            current_p95_ms: 12000.0,
            threshold_ms: 10000.0,
        });
        health.consecutive_successes = 5;

        let summary = health.summary();
        assert_eq!(summary.model_id, "gpt-4");
        assert_eq!(summary.status, HealthStatus::Degraded);
        assert!(summary.reason.is_some());
        assert_eq!(summary.consecutive_successes, 5);
    }

    #[test]
    fn test_default_configs() {
        let config = HealthConfig::default();
        assert!(config.enabled);
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.recovery_successes, 2);

        let cb_config = CircuitBreakerConfig::default();
        assert_eq!(cb_config.failure_threshold, 5);
        assert_eq!(cb_config.cooldown_secs, 30);

        let probe_config = ProbeConfig::default();
        assert_eq!(probe_config.interval_secs, 30);
        assert_eq!(probe_config.test_prompt, "Hi");
    }
}
