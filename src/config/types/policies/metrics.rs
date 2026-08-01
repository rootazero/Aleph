//! Performance metrics policies
//!
//! Configurable performance targets for monitoring and alerting
//! across different pipeline stages.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Policy for performance monitoring and alerting
///
/// Defines the threshold multiplier for `StageTimer` warnings and the
/// logging toggles. Stage-target latencies are intentionally not on this
/// type — they live with the `StageTimer` call site (see
/// `metrics::StageTimer::with_target`) because no two stages share a
/// target any caller wanted to configure.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetricsPolicy {
    /// Warning threshold multiplier
    /// Operations exceeding target * multiplier trigger warnings
    /// Default: 2.0
    #[serde(default = "default_warning_multiplier")]
    pub warning_multiplier: f64,

    /// Enable performance logging
    /// Default: true
    #[serde(default = "default_enable_logging")]
    pub enable_logging: bool,

    /// Enable performance warnings
    /// Default: true
    #[serde(default = "default_enable_warnings")]
    pub enable_warnings: bool,
}

impl Default for MetricsPolicy {
    fn default() -> Self {
        Self {
            warning_multiplier: default_warning_multiplier(),
            enable_logging: default_enable_logging(),
            enable_warnings: default_enable_warnings(),
        }
    }
}

const fn default_warning_multiplier() -> f64 {
    2.0
}

const fn default_enable_logging() -> bool {
    true
}

const fn default_enable_warnings() -> bool {
    true
}

impl MetricsPolicy {
    /// Get the warning threshold for a given target in milliseconds.
    /// Bound by warning_multiplier (default 2.0); falls back to the
    /// compile-time default when the configured value is non-finite or
    /// negative so a TOML typo can never silently disable warnings.
    pub fn warning_threshold_ms(&self, target_ms: u64) -> u64 {
        let multiplier = if self.warning_multiplier.is_finite() && self.warning_multiplier >= 0.0 {
            self.warning_multiplier
        } else {
            default_warning_multiplier()
        };
        (target_ms as f64 * multiplier).clamp(0.0, f64::MAX) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let policy = MetricsPolicy::default();
        assert_eq!(policy.warning_multiplier, 2.0);
        assert!(policy.enable_logging);
        assert!(policy.enable_warnings);
    }

    #[test]
    fn test_warning_threshold() {
        let policy = MetricsPolicy::default();
        // With 2.0 multiplier, 50ms target has 100ms threshold
        assert_eq!(policy.warning_threshold_ms(50), 100);
        assert_eq!(policy.warning_threshold_ms(100), 200);
    }

    #[test]
    fn test_partial_deserialization() {
        let toml = r#"
            warning_multiplier = 1.5
        "#;
        let policy: MetricsPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.warning_multiplier, 1.5);
        assert!(policy.enable_logging);
    }
}
