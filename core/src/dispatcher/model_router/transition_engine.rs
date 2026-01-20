//! Health Transition Engine
//!
//! This module implements the state machine for health status transitions,
//! including circuit breaker logic and transition rule evaluation.

use super::health::{
    CircuitBreakerConfig, CircuitState, DegradationReason, HealthConfig, HealthError, HealthEvent,
    HealthStatus, ModelHealth, UnhealthyReason,
};
use std::time::{Duration, SystemTime};

// ============================================================================
// Transition Results
// ============================================================================

/// Result of a transition evaluation
#[derive(Debug, Clone)]
pub struct TransitionResult {
    /// New status after transition (None if no change)
    pub new_status: Option<HealthStatus>,
    /// Event to emit (if status changed)
    pub event: Option<HealthEvent>,
    /// Whether circuit breaker state changed
    pub circuit_changed: bool,
}

impl TransitionResult {
    /// No transition occurred
    pub fn no_change() -> Self {
        Self {
            new_status: None,
            event: None,
            circuit_changed: false,
        }
    }

    /// Status changed
    pub fn changed(new_status: HealthStatus, event: HealthEvent) -> Self {
        Self {
            new_status: Some(new_status),
            event: Some(event),
            circuit_changed: false,
        }
    }

    /// Circuit breaker changed
    pub fn circuit_change(new_status: HealthStatus, event: HealthEvent) -> Self {
        Self {
            new_status: Some(new_status),
            event: Some(event),
            circuit_changed: true,
        }
    }
}

/// Call result for transition evaluation
#[derive(Debug, Clone)]
pub struct CallResult {
    /// Whether the call succeeded
    pub success: bool,
    /// Latency of the call (if available)
    pub latency: Option<Duration>,
    /// Error details (if failed)
    pub error: Option<HealthError>,
    /// P95 latency from recent calls (for degradation check)
    pub recent_p95_latency: Option<Duration>,
}

impl CallResult {
    /// Create a successful call result
    pub fn success(latency: Duration) -> Self {
        Self {
            success: true,
            latency: Some(latency),
            error: None,
            recent_p95_latency: None,
        }
    }

    /// Create a successful call result with p95 context
    pub fn success_with_context(latency: Duration, recent_p95: Duration) -> Self {
        Self {
            success: true,
            latency: Some(latency),
            error: None,
            recent_p95_latency: Some(recent_p95),
        }
    }

    /// Create a failed call result
    pub fn failure(error: HealthError) -> Self {
        Self {
            success: false,
            latency: None,
            error: Some(error),
            recent_p95_latency: None,
        }
    }
}

// ============================================================================
// Health Transition Engine
// ============================================================================

/// Engine for evaluating health status transitions
pub struct HealthTransitionEngine {
    config: HealthConfig,
}

impl HealthTransitionEngine {
    /// Create a new transition engine with config
    pub fn new(config: HealthConfig) -> Self {
        Self { config }
    }

    /// Create with default config
    pub fn with_defaults() -> Self {
        Self::new(HealthConfig::default())
    }

    /// Update configuration
    pub fn set_config(&mut self, config: HealthConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &HealthConfig {
        &self.config
    }

    /// Evaluate transition based on call result
    ///
    /// This is the main entry point for processing call results.
    /// It updates the model health state and returns any transitions.
    pub fn evaluate(&self, health: &mut ModelHealth, result: &CallResult) -> TransitionResult {
        if result.success {
            self.on_success(health, result)
        } else {
            self.on_failure(health, result)
        }
    }

    /// Check if circuit breaker cooldown has elapsed and transition to HalfOpen
    pub fn check_cooldown(&self, health: &mut ModelHealth) -> TransitionResult {
        if health.status != HealthStatus::CircuitOpen {
            return TransitionResult::no_change();
        }

        if health.circuit_breaker.allows_request() {
            // Transition to HalfOpen
            let old_status = health.status;
            health.status = HealthStatus::HalfOpen;
            health.status_since = SystemTime::now();
            health.circuit_breaker.state = CircuitState::HalfOpen;
            health.circuit_breaker.half_open_successes = 0;

            TransitionResult::changed(
                HealthStatus::HalfOpen,
                HealthEvent::StatusChanged {
                    model_id: health.model_id.clone(),
                    old_status,
                    new_status: HealthStatus::HalfOpen,
                    reason: Some("Cooldown elapsed, testing recovery".to_string()),
                },
            )
        } else {
            TransitionResult::no_change()
        }
    }

    /// Handle successful call
    fn on_success(&self, health: &mut ModelHealth, result: &CallResult) -> TransitionResult {
        health.record_success();

        match health.status {
            HealthStatus::Unknown => self.unknown_to_healthy(health),
            HealthStatus::Healthy => self.check_degradation(health, result),
            HealthStatus::Degraded => self.check_degraded_recovery(health, result),
            HealthStatus::Unhealthy => self.check_unhealthy_recovery(health),
            HealthStatus::HalfOpen => self.check_half_open_recovery(health),
            HealthStatus::CircuitOpen => {
                // Success during CircuitOpen shouldn't happen normally
                // but treat it as a recovery signal
                self.check_half_open_recovery(health)
            }
        }
    }

    /// Handle failed call
    fn on_failure(&self, health: &mut ModelHealth, result: &CallResult) -> TransitionResult {
        if let Some(ref error) = result.error {
            health.record_failure(error.clone());
        }

        match health.status {
            HealthStatus::Unknown => self.unknown_to_unhealthy(health, result),
            HealthStatus::Healthy => self.check_healthy_failure(health, result),
            HealthStatus::Degraded => self.check_degraded_failure(health, result),
            HealthStatus::Unhealthy => self.check_circuit_break(health),
            HealthStatus::HalfOpen => self.reopen_circuit(health),
            HealthStatus::CircuitOpen => TransitionResult::no_change(),
        }
    }

    // ========================================================================
    // Transition Handlers
    // ========================================================================

    /// Unknown → Healthy on first success
    fn unknown_to_healthy(&self, health: &mut ModelHealth) -> TransitionResult {
        let old_status = health.status;
        health.set_status(HealthStatus::Healthy);

        TransitionResult::changed(
            HealthStatus::Healthy,
            HealthEvent::StatusChanged {
                model_id: health.model_id.clone(),
                old_status,
                new_status: HealthStatus::Healthy,
                reason: Some("First successful call".to_string()),
            },
        )
    }

    /// Unknown → Unhealthy on first failure
    fn unknown_to_unhealthy(
        &self,
        health: &mut ModelHealth,
        result: &CallResult,
    ) -> TransitionResult {
        let old_status = health.status;
        health.set_status(HealthStatus::Unhealthy);
        health.unhealthy_reason = Some(UnhealthyReason::ConsecutiveFailures {
            count: 1,
            threshold: self.config.failure_threshold,
        });

        let reason = result
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_else(|| "First call failed".to_string());

        TransitionResult::changed(
            HealthStatus::Unhealthy,
            HealthEvent::StatusChanged {
                model_id: health.model_id.clone(),
                old_status,
                new_status: HealthStatus::Unhealthy,
                reason: Some(reason),
            },
        )
    }

    /// Check if healthy model should become degraded due to high latency
    fn check_degradation(&self, health: &mut ModelHealth, result: &CallResult) -> TransitionResult {
        let p95_ms = result
            .recent_p95_latency
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if p95_ms > self.config.latency_degradation_threshold_ms {
            let old_status = health.status;
            health.set_status(HealthStatus::Degraded);
            health.degradation_reason = Some(DegradationReason::HighLatency {
                current_p95_ms: p95_ms as f64,
                threshold_ms: self.config.latency_degradation_threshold_ms as f64,
            });

            TransitionResult::changed(
                HealthStatus::Degraded,
                HealthEvent::StatusChanged {
                    model_id: health.model_id.clone(),
                    old_status,
                    new_status: HealthStatus::Degraded,
                    reason: Some(format!("High latency: p95 {}ms", p95_ms)),
                },
            )
        } else {
            TransitionResult::no_change()
        }
    }

    /// Check if degraded model should recover to healthy
    fn check_degraded_recovery(
        &self,
        health: &mut ModelHealth,
        result: &CallResult,
    ) -> TransitionResult {
        // Check if we have enough consecutive successes
        if health.consecutive_successes < self.config.degraded_recovery_successes {
            return TransitionResult::no_change();
        }

        // Check if latency has returned to normal
        let p95_ms = result
            .recent_p95_latency
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if p95_ms <= self.config.latency_healthy_threshold_ms {
            let old_status = health.status;
            health.set_status(HealthStatus::Healthy);

            TransitionResult::changed(
                HealthStatus::Healthy,
                HealthEvent::StatusChanged {
                    model_id: health.model_id.clone(),
                    old_status,
                    new_status: HealthStatus::Healthy,
                    reason: Some(format!(
                        "Recovered: {} consecutive successes, latency normal",
                        health.consecutive_successes
                    )),
                },
            )
        } else {
            TransitionResult::no_change()
        }
    }

    /// Check if unhealthy model should recover to healthy
    fn check_unhealthy_recovery(&self, health: &mut ModelHealth) -> TransitionResult {
        if health.consecutive_successes >= self.config.recovery_successes {
            let old_status = health.status;
            health.set_status(HealthStatus::Healthy);
            health.circuit_breaker = Default::default();

            TransitionResult::changed(
                HealthStatus::Healthy,
                HealthEvent::StatusChanged {
                    model_id: health.model_id.clone(),
                    old_status,
                    new_status: HealthStatus::Healthy,
                    reason: Some(format!(
                        "Recovered after {} consecutive successes",
                        health.consecutive_successes
                    )),
                },
            )
        } else {
            TransitionResult::no_change()
        }
    }

    /// Check if half-open circuit should close (recover)
    fn check_half_open_recovery(&self, health: &mut ModelHealth) -> TransitionResult {
        health.circuit_breaker.half_open_successes += 1;

        if health.circuit_breaker.half_open_successes
            >= self.config.circuit_breaker.half_open_successes
        {
            let old_status = health.status;
            health.set_status(HealthStatus::Healthy);
            health.circuit_breaker = Default::default();

            TransitionResult::circuit_change(
                HealthStatus::Healthy,
                HealthEvent::CircuitClosed {
                    model_id: health.model_id.clone(),
                },
            )
        } else {
            TransitionResult::no_change()
        }
    }

    /// Check if healthy model should become unhealthy due to failures
    fn check_healthy_failure(
        &self,
        health: &mut ModelHealth,
        result: &CallResult,
    ) -> TransitionResult {
        if health.consecutive_failures >= self.config.failure_threshold {
            let old_status = health.status;
            health.set_status(HealthStatus::Unhealthy);
            health.unhealthy_reason = Some(UnhealthyReason::ConsecutiveFailures {
                count: health.consecutive_failures,
                threshold: self.config.failure_threshold,
            });

            let reason = result
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "Multiple consecutive failures".to_string());

            TransitionResult::changed(
                HealthStatus::Unhealthy,
                HealthEvent::StatusChanged {
                    model_id: health.model_id.clone(),
                    old_status,
                    new_status: HealthStatus::Unhealthy,
                    reason: Some(reason),
                },
            )
        } else {
            TransitionResult::no_change()
        }
    }

    /// Check if degraded model should become unhealthy due to failures
    fn check_degraded_failure(
        &self,
        health: &mut ModelHealth,
        result: &CallResult,
    ) -> TransitionResult {
        // Same threshold as healthy
        self.check_healthy_failure(health, result)
    }

    /// Check if unhealthy model should trigger circuit breaker
    fn check_circuit_break(&self, health: &mut ModelHealth) -> TransitionResult {
        health.circuit_breaker.failure_count += 1;

        // Check if we've hit the circuit breaker threshold
        if health.circuit_breaker.failure_count >= self.config.circuit_breaker.failure_threshold {
            self.open_circuit(health)
        } else {
            TransitionResult::no_change()
        }
    }

    /// Open the circuit breaker
    fn open_circuit(&self, health: &mut ModelHealth) -> TransitionResult {
        let old_status = health.status;
        health.set_status(HealthStatus::CircuitOpen);
        health.circuit_breaker.state = CircuitState::Open;
        health.circuit_breaker.open_count += 1;
        health.circuit_breaker.opened_at = Some(SystemTime::now());

        let cooldown = health
            .circuit_breaker
            .calculate_cooldown(self.config.circuit_breaker.cooldown_secs);
        health.circuit_breaker.next_attempt_at = Some(SystemTime::now() + cooldown);

        TransitionResult::circuit_change(
            HealthStatus::CircuitOpen,
            HealthEvent::CircuitOpened {
                model_id: health.model_id.clone(),
                failure_count: health.circuit_breaker.failure_count,
                cooldown_secs: cooldown.as_secs(),
            },
        )
    }

    /// Reopen circuit after half-open failure
    fn reopen_circuit(&self, health: &mut ModelHealth) -> TransitionResult {
        let old_status = health.status;
        health.set_status(HealthStatus::CircuitOpen);
        health.circuit_breaker.state = CircuitState::Open;
        health.circuit_breaker.half_open_successes = 0;
        health.circuit_breaker.open_count += 1;
        health.circuit_breaker.opened_at = Some(SystemTime::now());

        let cooldown = health
            .circuit_breaker
            .calculate_cooldown(self.config.circuit_breaker.cooldown_secs);
        health.circuit_breaker.next_attempt_at = Some(SystemTime::now() + cooldown);

        TransitionResult::circuit_change(
            HealthStatus::CircuitOpen,
            HealthEvent::CircuitOpened {
                model_id: health.model_id.clone(),
                failure_count: health.circuit_breaker.failure_count,
                cooldown_secs: cooldown.as_secs(),
            },
        )
    }

    // ========================================================================
    // Rate Limit Handling
    // ========================================================================

    /// Update rate limit info and check for warnings/degradation
    pub fn update_rate_limit(
        &self,
        health: &mut ModelHealth,
        remaining: u32,
        limit: u32,
        reset_at: Option<SystemTime>,
    ) -> Option<HealthEvent> {
        use super::health::RateLimitInfo;

        let info = RateLimitInfo::new(remaining, limit, reset_at);
        let remaining_percent = info.remaining_percent();
        health.update_rate_limit(info);

        // Check for rate limit exhaustion
        if remaining == 0 {
            health.set_status(HealthStatus::Unhealthy);
            health.unhealthy_reason = Some(UnhealthyReason::RateLimited { reset_at });
            return Some(HealthEvent::StatusChanged {
                model_id: health.model_id.clone(),
                old_status: health.status,
                new_status: HealthStatus::Unhealthy,
                reason: Some("Rate limit exhausted".to_string()),
            });
        }

        // Check for warning threshold
        if remaining_percent < self.config.rate_limit_warning_threshold {
            // If healthy, transition to degraded
            if health.status == HealthStatus::Healthy {
                health.set_status(HealthStatus::Degraded);
                health.degradation_reason =
                    Some(DegradationReason::NearRateLimit { remaining_percent });
            }

            return Some(HealthEvent::RateLimitWarning {
                model_id: health.model_id.clone(),
                remaining_percent,
                reset_at,
            });
        }

        None
    }

    /// Manually set model status (for admin override)
    pub fn set_manual_status(
        &self,
        health: &mut ModelHealth,
        status: HealthStatus,
        reason: String,
    ) -> TransitionResult {
        let old_status = health.status;

        match status {
            HealthStatus::Degraded => {
                health.set_status(status);
                health.degradation_reason = Some(DegradationReason::ManualOverride {
                    reason: reason.clone(),
                });
            }
            HealthStatus::Unhealthy => {
                health.set_status(status);
                health.unhealthy_reason = Some(UnhealthyReason::ManuallyDisabled {
                    reason: reason.clone(),
                });
            }
            _ => {
                health.set_status(status);
            }
        }

        TransitionResult::changed(
            status,
            HealthEvent::StatusChanged {
                model_id: health.model_id.clone(),
                old_status,
                new_status: status,
                reason: Some(format!("Manual override: {}", reason)),
            },
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::health::ErrorType;

    fn create_engine() -> HealthTransitionEngine {
        HealthTransitionEngine::with_defaults()
    }

    fn create_health(model_id: &str) -> ModelHealth {
        ModelHealth::new(model_id)
    }

    #[test]
    fn test_unknown_to_healthy_on_success() {
        let engine = create_engine();
        let mut health = create_health("test");
        assert_eq!(health.status, HealthStatus::Unknown);

        let result = CallResult::success(Duration::from_millis(100));
        let transition = engine.evaluate(&mut health, &result);

        assert_eq!(transition.new_status, Some(HealthStatus::Healthy));
        assert!(transition.event.is_some());
    }

    #[test]
    fn test_unknown_to_unhealthy_on_failure() {
        let engine = create_engine();
        let mut health = create_health("test");

        let error = HealthError::new(ErrorType::ServerError, "Internal error");
        let result = CallResult::failure(error);
        let transition = engine.evaluate(&mut health, &result);

        assert_eq!(transition.new_status, Some(HealthStatus::Unhealthy));
    }

    #[test]
    fn test_healthy_stays_healthy_on_success() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::Healthy;

        let result = CallResult::success(Duration::from_millis(100));
        let transition = engine.evaluate(&mut health, &result);

        assert!(transition.new_status.is_none());
        assert_eq!(health.consecutive_successes, 1);
    }

    #[test]
    fn test_healthy_to_degraded_on_high_latency() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::Healthy;

        // P95 latency > 10000ms threshold
        let result = CallResult::success_with_context(
            Duration::from_millis(12000),
            Duration::from_millis(12000),
        );
        let transition = engine.evaluate(&mut health, &result);

        assert_eq!(transition.new_status, Some(HealthStatus::Degraded));
    }

    #[test]
    fn test_healthy_to_unhealthy_on_failures() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::Healthy;

        let error = HealthError::new(ErrorType::ServerError, "Error");

        // Need 3 consecutive failures (default threshold)
        for i in 0..3 {
            let result = CallResult::failure(error.clone());
            let transition = engine.evaluate(&mut health, &result);

            if i < 2 {
                assert!(transition.new_status.is_none());
            } else {
                assert_eq!(transition.new_status, Some(HealthStatus::Unhealthy));
            }
        }
    }

    #[test]
    fn test_degraded_to_healthy_on_recovery() {
        let mut config = HealthConfig::default();
        config.degraded_recovery_successes = 3;
        let engine = HealthTransitionEngine::new(config);

        let mut health = create_health("test");
        health.status = HealthStatus::Degraded;

        // Need 3 consecutive successes with normal latency
        for i in 0..3 {
            let result = CallResult::success_with_context(
                Duration::from_millis(500),
                Duration::from_millis(500),
            );
            let transition = engine.evaluate(&mut health, &result);

            if i < 2 {
                assert!(transition.new_status.is_none());
            } else {
                assert_eq!(transition.new_status, Some(HealthStatus::Healthy));
            }
        }
    }

    #[test]
    fn test_unhealthy_to_healthy_on_recovery() {
        let mut config = HealthConfig::default();
        config.recovery_successes = 2;
        let engine = HealthTransitionEngine::new(config);

        let mut health = create_health("test");
        health.status = HealthStatus::Unhealthy;

        // Need 2 consecutive successes
        let result = CallResult::success(Duration::from_millis(100));

        engine.evaluate(&mut health, &result);
        assert!(health.status == HealthStatus::Unhealthy);

        let transition = engine.evaluate(&mut health, &result);
        assert_eq!(transition.new_status, Some(HealthStatus::Healthy));
    }

    #[test]
    fn test_circuit_breaker_opens() {
        let mut config = HealthConfig::default();
        config.circuit_breaker.failure_threshold = 3;
        let engine = HealthTransitionEngine::new(config);

        let mut health = create_health("test");
        health.status = HealthStatus::Unhealthy;

        let error = HealthError::new(ErrorType::ServerError, "Error");

        // Trigger circuit breaker after 3 failures in unhealthy state
        for i in 0..3 {
            let result = CallResult::failure(error.clone());
            let transition = engine.evaluate(&mut health, &result);

            if i < 2 {
                assert!(
                    transition.new_status.is_none()
                        || transition.new_status != Some(HealthStatus::CircuitOpen)
                );
            } else {
                assert_eq!(transition.new_status, Some(HealthStatus::CircuitOpen));
                assert!(transition.circuit_changed);
            }
        }
    }

    #[test]
    fn test_circuit_breaker_cooldown_to_half_open() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::CircuitOpen;
        health.circuit_breaker.state = CircuitState::Open;
        // Set cooldown to past
        health.circuit_breaker.next_attempt_at = Some(SystemTime::now() - Duration::from_secs(1));

        let transition = engine.check_cooldown(&mut health);

        assert_eq!(transition.new_status, Some(HealthStatus::HalfOpen));
    }

    #[test]
    fn test_half_open_to_healthy_on_success() {
        let mut config = HealthConfig::default();
        config.circuit_breaker.half_open_successes = 2;
        let engine = HealthTransitionEngine::new(config);

        let mut health = create_health("test");
        health.status = HealthStatus::HalfOpen;
        health.circuit_breaker.state = CircuitState::HalfOpen;

        let result = CallResult::success(Duration::from_millis(100));

        // First success
        let transition = engine.evaluate(&mut health, &result);
        assert!(transition.new_status.is_none());

        // Second success - should close circuit
        let transition = engine.evaluate(&mut health, &result);
        assert_eq!(transition.new_status, Some(HealthStatus::Healthy));
        assert!(transition.circuit_changed);
    }

    #[test]
    fn test_half_open_to_circuit_open_on_failure() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::HalfOpen;
        health.circuit_breaker.state = CircuitState::HalfOpen;

        let error = HealthError::new(ErrorType::ServerError, "Error");
        let result = CallResult::failure(error);

        let transition = engine.evaluate(&mut health, &result);

        assert_eq!(transition.new_status, Some(HealthStatus::CircuitOpen));
        assert!(transition.circuit_changed);
    }

    #[test]
    fn test_exponential_backoff() {
        let mut config = HealthConfig::default();
        config.circuit_breaker.failure_threshold = 3;
        let engine = HealthTransitionEngine::new(config);

        let mut health = create_health("test");
        health.status = HealthStatus::Unhealthy;

        let error = HealthError::new(ErrorType::ServerError, "Error");

        // Trigger circuit breaker
        let result = CallResult::failure(error.clone());
        for _ in 0..5 {
            engine.evaluate(&mut health, &result);
        }

        // Circuit should be open
        assert_eq!(health.status, HealthStatus::CircuitOpen);

        // First open: base cooldown (30s) with open_count = 1
        let first_cooldown = health.circuit_breaker.calculate_cooldown(30).as_secs();

        // Simulate another open - should have longer cooldown due to open_count = 2
        health.circuit_breaker.open_count = 2;
        let second_cooldown = health.circuit_breaker.calculate_cooldown(30).as_secs();

        assert!(second_cooldown > first_cooldown);
    }

    #[test]
    fn test_rate_limit_warning() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::Healthy;

        // Below warning threshold (20%)
        let event = engine.update_rate_limit(&mut health, 10, 100, None);

        assert!(event.is_some());
        match event.unwrap() {
            HealthEvent::RateLimitWarning {
                remaining_percent, ..
            } => {
                assert!((remaining_percent - 0.1).abs() < 0.001);
            }
            _ => panic!("Expected RateLimitWarning"),
        }

        // Status should be degraded
        assert_eq!(health.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_rate_limit_exhausted() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::Healthy;

        let event = engine.update_rate_limit(&mut health, 0, 100, None);

        assert!(event.is_some());
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_manual_status_override() {
        let engine = create_engine();
        let mut health = create_health("test");
        health.status = HealthStatus::Healthy;

        let transition = engine.set_manual_status(
            &mut health,
            HealthStatus::Degraded,
            "Maintenance".to_string(),
        );

        assert_eq!(transition.new_status, Some(HealthStatus::Degraded));
        assert!(health.degradation_reason.is_some());
    }

    #[test]
    fn test_transition_result_helpers() {
        let no_change = TransitionResult::no_change();
        assert!(no_change.new_status.is_none());
        assert!(no_change.event.is_none());

        let changed = TransitionResult::changed(
            HealthStatus::Healthy,
            HealthEvent::CircuitClosed {
                model_id: "test".to_string(),
            },
        );
        assert_eq!(changed.new_status, Some(HealthStatus::Healthy));
        assert!(!changed.circuit_changed);

        let circuit = TransitionResult::circuit_change(
            HealthStatus::CircuitOpen,
            HealthEvent::CircuitOpened {
                model_id: "test".to_string(),
                failure_count: 5,
                cooldown_secs: 30,
            },
        );
        assert!(circuit.circuit_changed);
    }
}
