//! Health Manager
//!
//! This module provides centralized health management for AI models,
//! including state tracking, event broadcasting, and recovery probing.

use super::health::{
    HealthConfig, HealthError, HealthEvent, HealthStatus, ModelHealth, ModelHealthSummary,
    RateLimitInfo,
};
use super::transition_engine::{CallResult, HealthTransitionEngine};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{broadcast, RwLock};

// ============================================================================
// Health Manager
// ============================================================================

/// Centralized health management for AI models
pub struct HealthManager {
    /// Health states for all models
    health_states: Arc<RwLock<HashMap<String, ModelHealth>>>,
    /// Transition engine for state changes
    transition_engine: Arc<RwLock<HealthTransitionEngine>>,
    /// Event broadcaster
    event_tx: broadcast::Sender<HealthEvent>,
    /// Configuration
    config: Arc<RwLock<HealthConfig>>,
}

impl HealthManager {
    /// Create a new health manager with configuration
    pub fn new(config: HealthConfig) -> Self {
        let (event_tx, _) = broadcast::channel(100);

        Self {
            health_states: Arc::new(RwLock::new(HashMap::new())),
            transition_engine: Arc::new(RwLock::new(HealthTransitionEngine::new(config.clone()))),
            event_tx,
            config: Arc::new(RwLock::new(config)),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(HealthConfig::default())
    }

    /// Subscribe to health events
    pub fn subscribe(&self) -> broadcast::Receiver<HealthEvent> {
        self.event_tx.subscribe()
    }

    /// Update configuration
    pub async fn set_config(&self, config: HealthConfig) {
        let mut engine = self.transition_engine.write().await;
        engine.set_config(config.clone());
        let mut cfg = self.config.write().await;
        *cfg = config;
    }

    /// Get current configuration
    pub async fn config(&self) -> HealthConfig {
        self.config.read().await.clone()
    }

    // ========================================================================
    // Health State Management
    // ========================================================================

    /// Get health status for a model
    pub async fn get_status(&self, model_id: &str) -> HealthStatus {
        let states = self.health_states.read().await;
        states
            .get(model_id)
            .map(|h| h.status)
            .unwrap_or(HealthStatus::Unknown)
    }

    /// Get full health state for a model
    pub async fn get_health(&self, model_id: &str) -> Option<ModelHealth> {
        let states = self.health_states.read().await;
        states.get(model_id).cloned()
    }

    /// Get all health states
    pub async fn all_health(&self) -> HashMap<String, ModelHealth> {
        self.health_states.read().await.clone()
    }

    /// Get health summary for all models
    pub async fn all_summaries(&self) -> Vec<ModelHealthSummary> {
        let states = self.health_states.read().await;
        let mut summaries: Vec<_> = states.values().map(|h| h.summary()).collect();
        // Sort by status priority (healthy first)
        summaries.sort_by_key(|s| s.status.priority());
        summaries
    }

    /// Get models by status
    pub async fn models_with_status(&self, status: HealthStatus) -> Vec<String> {
        let states = self.health_states.read().await;
        states
            .iter()
            .filter(|(_, h)| h.status == status)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get available models (can accept requests)
    pub async fn available_models(&self) -> Vec<String> {
        let states = self.health_states.read().await;
        states
            .iter()
            .filter(|(_, h)| h.status.can_call())
            .map(|(id, _)| id.clone())
            .collect()
    }

    // ========================================================================
    // Call Result Recording
    // ========================================================================

    /// Record a successful call
    pub async fn record_success(
        &self,
        model_id: &str,
        latency: std::time::Duration,
        recent_p95: Option<std::time::Duration>,
    ) {
        let result = if let Some(p95) = recent_p95 {
            CallResult::success_with_context(latency, p95)
        } else {
            CallResult::success(latency)
        };

        self.process_call_result(model_id, result).await;
    }

    /// Record a failed call
    pub async fn record_failure(&self, model_id: &str, error: HealthError) {
        let result = CallResult::failure(error);
        self.process_call_result(model_id, result).await;
    }

    /// Process a call result and update health state
    async fn process_call_result(&self, model_id: &str, result: CallResult) {
        let mut states = self.health_states.write().await;
        let engine = self.transition_engine.read().await;

        // Get or create health state
        let health = states
            .entry(model_id.to_string())
            .or_insert_with(|| ModelHealth::new(model_id));

        // Evaluate transition
        let transition = engine.evaluate(health, &result);

        // Emit event if status changed
        if let Some(event) = transition.event {
            let _ = self.event_tx.send(event);
        }
    }

    /// Update rate limit information
    pub async fn update_rate_limit(
        &self,
        model_id: &str,
        remaining: u32,
        limit: u32,
        reset_at: Option<SystemTime>,
    ) {
        let mut states = self.health_states.write().await;
        let engine = self.transition_engine.read().await;

        let health = states
            .entry(model_id.to_string())
            .or_insert_with(|| ModelHealth::new(model_id));

        if let Some(event) = engine.update_rate_limit(health, remaining, limit, reset_at) {
            let _ = self.event_tx.send(event);
        }
    }

    // ========================================================================
    // Circuit Breaker Management
    // ========================================================================

    /// Check all circuit breakers for cooldown expiration
    pub async fn check_cooldowns(&self) -> Vec<HealthEvent> {
        let mut states = self.health_states.write().await;
        let engine = self.transition_engine.read().await;
        let mut events = Vec::new();

        for health in states.values_mut() {
            if health.status == HealthStatus::CircuitOpen {
                let transition = engine.check_cooldown(health);
                if let Some(event) = transition.event {
                    events.push(event.clone());
                    let _ = self.event_tx.send(event);
                }
            }
        }

        events
    }

    /// Get models with open circuits
    pub async fn open_circuits(&self) -> Vec<String> {
        self.models_with_status(HealthStatus::CircuitOpen).await
    }

    /// Get models in half-open state (recovery testing)
    pub async fn half_open_models(&self) -> Vec<String> {
        self.models_with_status(HealthStatus::HalfOpen).await
    }

    // ========================================================================
    // Manual Controls
    // ========================================================================

    /// Manually set model status
    pub async fn set_status(&self, model_id: &str, status: HealthStatus, reason: String) {
        let mut states = self.health_states.write().await;
        let engine = self.transition_engine.read().await;

        let health = states
            .entry(model_id.to_string())
            .or_insert_with(|| ModelHealth::new(model_id));

        let transition = engine.set_manual_status(health, status, reason);

        if let Some(event) = transition.event {
            let _ = self.event_tx.send(event);
        }
    }

    /// Reset model to unknown state
    pub async fn reset(&self, model_id: &str) {
        let mut states = self.health_states.write().await;
        states.remove(model_id);
    }

    /// Reset all models
    pub async fn reset_all(&self) {
        let mut states = self.health_states.write().await;
        states.clear();
    }

    // ========================================================================
    // Health Checks
    // ========================================================================

    /// Check if a model can accept requests
    pub async fn can_call(&self, model_id: &str) -> bool {
        let states = self.health_states.read().await;
        states
            .get(model_id)
            .map(|h| h.status.can_call())
            .unwrap_or(true) // Unknown models are allowed
    }

    /// Check if a model can be used for recovery test
    pub async fn can_call_for_recovery(&self, model_id: &str) -> bool {
        let states = self.health_states.read().await;
        states
            .get(model_id)
            .map(|h| h.status.can_call_for_recovery() || h.circuit_breaker.allows_request())
            .unwrap_or(false)
    }

    /// Get models that need recovery probing
    pub async fn models_needing_probe(&self) -> Vec<String> {
        let states = self.health_states.read().await;
        states
            .iter()
            .filter(|(_, h)| {
                matches!(h.status, HealthStatus::Unhealthy | HealthStatus::CircuitOpen)
                    && h.circuit_breaker.allows_request()
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    /// Get health statistics
    pub async fn statistics(&self) -> HealthStatistics {
        let states = self.health_states.read().await;

        let mut stats = HealthStatistics::default();
        stats.total = states.len();

        for health in states.values() {
            match health.status {
                HealthStatus::Healthy => stats.healthy += 1,
                HealthStatus::Degraded => stats.degraded += 1,
                HealthStatus::Unhealthy => stats.unhealthy += 1,
                HealthStatus::CircuitOpen => stats.circuit_open += 1,
                HealthStatus::HalfOpen => stats.half_open += 1,
                HealthStatus::Unknown => stats.unknown += 1,
            }
        }

        stats
    }
}

// ============================================================================
// Health Statistics
// ============================================================================

/// Statistics about model health
#[derive(Debug, Clone, Default)]
pub struct HealthStatistics {
    /// Total number of tracked models
    pub total: usize,
    /// Number of healthy models
    pub healthy: usize,
    /// Number of degraded models
    pub degraded: usize,
    /// Number of unhealthy models
    pub unhealthy: usize,
    /// Number of models with open circuits
    pub circuit_open: usize,
    /// Number of models in half-open state
    pub half_open: usize,
    /// Number of models with unknown status
    pub unknown: usize,
}

impl HealthStatistics {
    /// Get percentage of healthy models
    pub fn healthy_percent(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            (self.healthy as f64 / self.total as f64) * 100.0
        }
    }

    /// Get percentage of available models (can accept requests)
    pub fn available_percent(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            let available = self.healthy + self.degraded + self.unknown;
            (available as f64 / self.total as f64) * 100.0
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::health::ErrorType;
    use std::time::Duration;

    #[tokio::test]
    async fn test_new_model_starts_unknown() {
        let manager = HealthManager::with_defaults();
        let status = manager.get_status("test-model").await;
        assert_eq!(status, HealthStatus::Unknown);
    }

    #[tokio::test]
    async fn test_success_transitions_to_healthy() {
        let manager = HealthManager::with_defaults();

        manager
            .record_success("test-model", Duration::from_millis(100), None)
            .await;

        let status = manager.get_status("test-model").await;
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_failure_transitions_to_unhealthy() {
        let manager = HealthManager::with_defaults();

        let error = HealthError::new(ErrorType::ServerError, "Error");
        manager.record_failure("test-model", error).await;

        let status = manager.get_status("test-model").await;
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_multiple_failures_trigger_circuit() {
        let mut config = HealthConfig::default();
        config.failure_threshold = 2;
        config.circuit_breaker.failure_threshold = 2;
        let manager = HealthManager::new(config);

        let error = HealthError::new(ErrorType::ServerError, "Error");

        // First failure: Unknown -> Unhealthy
        manager.record_failure("test-model", error.clone()).await;
        assert_eq!(manager.get_status("test-model").await, HealthStatus::Unhealthy);

        // More failures to trigger circuit breaker
        for _ in 0..3 {
            manager.record_failure("test-model", error.clone()).await;
        }

        let status = manager.get_status("test-model").await;
        assert_eq!(status, HealthStatus::CircuitOpen);
    }

    #[tokio::test]
    async fn test_event_subscription() {
        let manager = HealthManager::with_defaults();
        let mut rx = manager.subscribe();

        manager
            .record_success("test-model", Duration::from_millis(100), None)
            .await;

        // Should receive status change event
        let event = rx.try_recv();
        assert!(event.is_ok());
        match event.unwrap() {
            HealthEvent::StatusChanged { new_status, .. } => {
                assert_eq!(new_status, HealthStatus::Healthy);
            }
            _ => panic!("Expected StatusChanged event"),
        }
    }

    #[tokio::test]
    async fn test_all_summaries() {
        let manager = HealthManager::with_defaults();

        manager
            .record_success("model-a", Duration::from_millis(100), None)
            .await;

        let error = HealthError::new(ErrorType::ServerError, "Error");
        manager.record_failure("model-b", error).await;

        let summaries = manager.all_summaries().await;
        assert_eq!(summaries.len(), 2);

        // Should be sorted by priority (healthy first)
        assert_eq!(summaries[0].status, HealthStatus::Healthy);
        assert_eq!(summaries[1].status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_available_models() {
        let manager = HealthManager::with_defaults();

        manager
            .record_success("model-a", Duration::from_millis(100), None)
            .await;

        let error = HealthError::new(ErrorType::ServerError, "Error");
        manager.record_failure("model-b", error).await;

        let available = manager.available_models().await;
        assert_eq!(available.len(), 1);
        assert!(available.contains(&"model-a".to_string()));
    }

    #[tokio::test]
    async fn test_manual_status_override() {
        let manager = HealthManager::with_defaults();

        manager
            .record_success("test-model", Duration::from_millis(100), None)
            .await;
        assert_eq!(manager.get_status("test-model").await, HealthStatus::Healthy);

        manager
            .set_status("test-model", HealthStatus::Degraded, "Maintenance".to_string())
            .await;

        assert_eq!(manager.get_status("test-model").await, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn test_reset() {
        let manager = HealthManager::with_defaults();

        manager
            .record_success("test-model", Duration::from_millis(100), None)
            .await;
        assert_eq!(manager.get_status("test-model").await, HealthStatus::Healthy);

        manager.reset("test-model").await;
        assert_eq!(manager.get_status("test-model").await, HealthStatus::Unknown);
    }

    #[tokio::test]
    async fn test_can_call() {
        let manager = HealthManager::with_defaults();

        // Unknown model can be called
        assert!(manager.can_call("unknown").await);

        manager
            .record_success("healthy", Duration::from_millis(100), None)
            .await;
        assert!(manager.can_call("healthy").await);

        let error = HealthError::new(ErrorType::ServerError, "Error");
        manager.record_failure("unhealthy", error).await;
        assert!(!manager.can_call("unhealthy").await);
    }

    #[tokio::test]
    async fn test_rate_limit_warning() {
        let manager = HealthManager::with_defaults();
        let mut rx = manager.subscribe();

        // First make it healthy
        manager
            .record_success("test-model", Duration::from_millis(100), None)
            .await;
        let _ = rx.try_recv(); // Clear the status change event

        // Update with low rate limit
        manager.update_rate_limit("test-model", 10, 100, None).await;

        // Should receive rate limit warning
        let event = rx.try_recv();
        assert!(event.is_ok());
        match event.unwrap() {
            HealthEvent::RateLimitWarning { remaining_percent, .. } => {
                assert!((remaining_percent - 0.1).abs() < 0.001);
            }
            _ => panic!("Expected RateLimitWarning event"),
        }
    }

    #[tokio::test]
    async fn test_statistics() {
        let manager = HealthManager::with_defaults();

        manager
            .record_success("model-a", Duration::from_millis(100), None)
            .await;
        manager
            .record_success("model-b", Duration::from_millis(100), None)
            .await;

        let error = HealthError::new(ErrorType::ServerError, "Error");
        manager.record_failure("model-c", error).await;

        let stats = manager.statistics().await;
        assert_eq!(stats.total, 3);
        assert_eq!(stats.healthy, 2);
        assert_eq!(stats.unhealthy, 1);
        assert!((stats.healthy_percent() - 66.666).abs() < 1.0);
    }

    #[tokio::test]
    async fn test_check_cooldowns() {
        let mut config = HealthConfig::default();
        config.failure_threshold = 1;
        config.circuit_breaker.failure_threshold = 1;
        config.circuit_breaker.cooldown_secs = 0; // Immediate cooldown for test
        let manager = HealthManager::new(config);

        let error = HealthError::new(ErrorType::ServerError, "Error");

        // Trigger circuit breaker
        for _ in 0..3 {
            manager.record_failure("test-model", error.clone()).await;
        }

        let status = manager.get_status("test-model").await;
        assert_eq!(status, HealthStatus::CircuitOpen);

        // Wait a tiny bit then check cooldowns
        tokio::time::sleep(Duration::from_millis(10)).await;

        let events = manager.check_cooldowns().await;
        // Should transition to HalfOpen
        assert!(!events.is_empty() || manager.get_status("test-model").await == HealthStatus::HalfOpen);
    }

    #[test]
    fn test_health_statistics() {
        let mut stats = HealthStatistics::default();
        assert_eq!(stats.healthy_percent(), 100.0);
        assert_eq!(stats.available_percent(), 100.0);

        stats.total = 10;
        stats.healthy = 5;
        stats.degraded = 2;
        stats.unhealthy = 2;
        stats.unknown = 1;

        assert_eq!(stats.healthy_percent(), 50.0);
        assert_eq!(stats.available_percent(), 80.0); // healthy + degraded + unknown = 8
    }
}
