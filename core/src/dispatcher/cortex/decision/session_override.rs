//! SessionOverride - Session-level configuration overrides
//!
//! Allows per-session customization of decision configuration without
//! modifying the global settings. Useful for temporary adjustments
//! like disabling confirmations or adjusting timeouts.

use serde::{Deserialize, Serialize};

use super::config::DecisionConfig;

/// Session-level configuration overrides
///
/// Contains optional overrides for specific configuration values.
/// Only specified fields will override the global configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionOverride {
    /// Routing configuration overrides
    pub routing: Option<RoutingOverride>,
    /// Confirmation configuration overrides
    pub confirmation: Option<ConfirmationOverride>,
}

/// Routing-specific session overrides
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingOverride {
    /// Enable/disable L3 semantic matching
    pub l3_enabled: Option<bool>,
    /// Override L3 timeout in milliseconds
    pub l3_timeout_ms: Option<u64>,
}

/// Confirmation-specific session overrides
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationOverride {
    /// Enable/disable confirmation prompts
    pub enabled: Option<bool>,
    /// Override auto-execute threshold
    pub auto_execute_threshold: Option<f32>,
}

/// Merge global configuration with session overrides
///
/// Creates a new configuration by applying session overrides to the
/// global configuration. Only specified overrides are applied; unset
/// fields retain their global values.
///
/// # Arguments
///
/// * `global` - The base global configuration
/// * `session` - Session-specific overrides to apply
///
/// # Returns
///
/// A new `DecisionConfig` with overrides applied
pub fn merge_config(global: &DecisionConfig, session: &SessionOverride) -> DecisionConfig {
    let mut merged = global.clone();

    if let Some(ref routing) = session.routing {
        if let Some(timeout) = routing.l3_timeout_ms {
            merged.routing.l3_timeout_ms = timeout;
        }
        // Note: l3_enabled would typically be handled at a higher level
        // since it's a boolean that affects routing behavior, not just thresholds
    }

    if let Some(ref confirmation) = session.confirmation {
        if let Some(enabled) = confirmation.enabled {
            merged.confirmation.enabled = enabled;
        }
        if let Some(threshold) = confirmation.auto_execute_threshold {
            merged.confirmation.auto_execute_threshold = threshold;
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_override_default() {
        let override_config = SessionOverride::default();
        assert!(override_config.routing.is_none());
        assert!(override_config.confirmation.is_none());
    }

    #[test]
    fn test_merge_empty_override() {
        let global = DecisionConfig::default();
        let session = SessionOverride::default();

        let merged = merge_config(&global, &session);

        // Should be identical to global
        assert_eq!(merged.routing.l1_threshold, global.routing.l1_threshold);
        assert_eq!(merged.routing.l2_threshold, global.routing.l2_threshold);
        assert_eq!(merged.routing.l3_threshold, global.routing.l3_threshold);
        assert_eq!(merged.routing.l3_timeout_ms, global.routing.l3_timeout_ms);
        assert_eq!(
            merged.routing.no_match_threshold,
            global.routing.no_match_threshold
        );
        assert_eq!(merged.confirmation.enabled, global.confirmation.enabled);
        assert_eq!(
            merged.confirmation.require_threshold,
            global.confirmation.require_threshold
        );
        assert_eq!(
            merged.confirmation.auto_execute_threshold,
            global.confirmation.auto_execute_threshold
        );
        assert_eq!(
            merged.confirmation.timeout_ms,
            global.confirmation.timeout_ms
        );
    }

    #[test]
    fn test_merge_with_routing_override() {
        let global = DecisionConfig::default();
        let session = SessionOverride {
            routing: Some(RoutingOverride {
                l3_enabled: Some(false),
                l3_timeout_ms: Some(10000),
            }),
            confirmation: None,
        };

        let merged = merge_config(&global, &session);

        // L3 timeout should be overridden
        assert_eq!(merged.routing.l3_timeout_ms, 10000);

        // Other routing values should remain unchanged
        assert_eq!(merged.routing.l1_threshold, global.routing.l1_threshold);
        assert_eq!(merged.routing.l2_threshold, global.routing.l2_threshold);
        assert_eq!(merged.routing.l3_threshold, global.routing.l3_threshold);
    }

    #[test]
    fn test_merge_with_confirmation_override() {
        let global = DecisionConfig::default();
        let session = SessionOverride {
            routing: None,
            confirmation: Some(ConfirmationOverride {
                enabled: Some(false),
                auto_execute_threshold: Some(0.8),
            }),
        };

        let merged = merge_config(&global, &session);

        // Confirmation should be overridden
        assert!(!merged.confirmation.enabled);
        assert_eq!(merged.confirmation.auto_execute_threshold, 0.8);

        // Other confirmation values should remain unchanged
        assert_eq!(
            merged.confirmation.require_threshold,
            global.confirmation.require_threshold
        );
        assert_eq!(
            merged.confirmation.timeout_ms,
            global.confirmation.timeout_ms
        );
    }

    #[test]
    fn test_merge_with_partial_confirmation_override() {
        let global = DecisionConfig::default();
        let session = SessionOverride {
            routing: None,
            confirmation: Some(ConfirmationOverride {
                enabled: None, // Not overriding
                auto_execute_threshold: Some(0.75),
            }),
        };

        let merged = merge_config(&global, &session);

        // Only auto_execute_threshold should be overridden
        assert_eq!(merged.confirmation.enabled, global.confirmation.enabled);
        assert_eq!(merged.confirmation.auto_execute_threshold, 0.75);
    }

    #[test]
    fn test_merge_with_all_overrides() {
        let global = DecisionConfig::default();
        let session = SessionOverride {
            routing: Some(RoutingOverride {
                l3_enabled: Some(true),
                l3_timeout_ms: Some(15000),
            }),
            confirmation: Some(ConfirmationOverride {
                enabled: Some(false),
                auto_execute_threshold: Some(0.85),
            }),
        };

        let merged = merge_config(&global, &session);

        // Routing overrides
        assert_eq!(merged.routing.l3_timeout_ms, 15000);

        // Confirmation overrides
        assert!(!merged.confirmation.enabled);
        assert_eq!(merged.confirmation.auto_execute_threshold, 0.85);

        // Execution should be unchanged (no execution override)
        assert_eq!(
            merged.execution.max_parallel_calls,
            global.execution.max_parallel_calls
        );
    }

    #[test]
    fn test_serialization() {
        let session = SessionOverride {
            routing: Some(RoutingOverride {
                l3_enabled: Some(true),
                l3_timeout_ms: Some(8000),
            }),
            confirmation: Some(ConfirmationOverride {
                enabled: Some(false),
                auto_execute_threshold: Some(0.95),
            }),
        };

        let json = serde_json::to_string(&session).unwrap();
        let deserialized: SessionOverride = serde_json::from_str(&json).unwrap();

        assert!(deserialized.routing.is_some());
        assert!(deserialized.confirmation.is_some());

        let routing = deserialized.routing.unwrap();
        assert_eq!(routing.l3_enabled, Some(true));
        assert_eq!(routing.l3_timeout_ms, Some(8000));

        let confirmation = deserialized.confirmation.unwrap();
        assert_eq!(confirmation.enabled, Some(false));
        assert_eq!(confirmation.auto_execute_threshold, Some(0.95));
    }
}
