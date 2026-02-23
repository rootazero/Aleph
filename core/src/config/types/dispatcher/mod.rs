//! Dispatcher configuration types
//!
//! Contains Dispatcher Layer (Aleph Cortex) configuration:
//! - DispatcherConfigToml: Multi-layer routing and confirmation settings
//! - AgentConfigToml: L3 Agent (multi-step planning) settings

mod backoff;
mod budget;
mod core;
mod retry;

// Re-export all types for backward compatibility
pub use budget::*;
pub use core::*;
pub use retry::*;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::backoff::BackoffConfigToml;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfigToml::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.attempt_timeout_ms, 30000);
        assert_eq!(config.total_timeout_ms, 90000);
        assert!(config.failover_on_non_retryable);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_retry_config_validation() {
        let mut config = RetryConfigToml::default();
        config.max_attempts = 0;
        assert!(config.validate().is_err());

        config.max_attempts = 3;
        config.backoff.strategy = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_backoff_config_default() {
        let config = BackoffConfigToml::default();
        assert_eq!(config.strategy, "exponential_jitter");
        assert_eq!(config.initial_ms, 100);
        assert_eq!(config.max_ms, 5000);
        assert_eq!(config.multiplier, 2.0);
        assert_eq!(config.jitter_factor, 0.2);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_backoff_config_validation() {
        let mut config = BackoffConfigToml::default();

        // Invalid strategy
        config.strategy = "invalid".to_string();
        assert!(config.validate().is_err());

        // Reset and test jitter factor
        config.strategy = "exponential_jitter".to_string();
        config.jitter_factor = 1.5;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_budget_config_default() {
        let config = BudgetConfigToml::default();
        assert!(config.enabled);
        assert_eq!(config.default_enforcement, "soft_block");
        assert_eq!(config.estimation_safety_margin, 1.2);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_budget_limit_config_validation() {
        let limit = BudgetLimitConfigToml {
            id: "test".to_string(),
            scope: "global".to_string(),
            scope_value: None,
            period: "daily".to_string(),
            reset_hour: 0,
            reset_day: 0,
            limit_usd: 10.0,
            warning_thresholds: vec![0.5, 0.8],
            enforcement: Some("soft_block".to_string()),
        };
        assert!(limit.validate().is_ok());

        // Test invalid scope
        let mut invalid = limit.clone();
        invalid.scope = "invalid".to_string();
        assert!(invalid.validate().is_err());

        // Test invalid period
        let mut invalid = limit.clone();
        invalid.period = "invalid".to_string();
        assert!(invalid.validate().is_err());

        // Test invalid limit_usd
        let mut invalid = limit.clone();
        invalid.limit_usd = -1.0;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_retry_config_to_policy() {
        let config = RetryConfigToml::default();
        let policy = config.to_retry_policy();

        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.attempt_timeout_ms, 30000);
        assert_eq!(policy.total_timeout_ms, Some(90000));
    }

    #[test]
    fn test_backoff_config_to_strategy() {
        let config = BackoffConfigToml::default();
        let strategy = config.to_backoff_strategy();

        match strategy {
            crate::dispatcher::model_router::BackoffStrategy::ExponentialJitter {
                initial_ms,
                max_ms,
                jitter_factor,
            } => {
                assert_eq!(initial_ms, 100);
                assert_eq!(max_ms, 5000);
                assert_eq!(jitter_factor, 0.2);
            }
            _ => panic!("Expected ExponentialJitter strategy"),
        }
    }

    #[test]
    fn test_budget_limit_to_internal() {
        let limit = BudgetLimitConfigToml {
            id: "test".to_string(),
            scope: "project".to_string(),
            scope_value: Some("my-project".to_string()),
            period: "weekly".to_string(),
            reset_hour: 8,
            reset_day: 1,
            limit_usd: 50.0,
            warning_thresholds: vec![0.5, 0.8],
            enforcement: Some("hard_block".to_string()),
        };

        let internal = limit.to_budget_limit("soft_block");

        assert_eq!(internal.id, "test");
        assert_eq!(internal.limit_usd, 50.0);
        assert_eq!(internal.warning_thresholds, vec![0.5, 0.8]);
    }
}
