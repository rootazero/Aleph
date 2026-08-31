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

/// Canonical default for `MetricsPolicy::warning_multiplier`. Exposed so
/// other modules (e.g. `crate::metrics`) can source their compiled-time
/// fallback from the same definition instead of duplicating the literal —
/// the metrics runtime's pre-init `StageTimer` uses this to avoid drifting
/// from any operator-configured default.
pub(crate) const DEFAULT_WARNING_MULTIPLIER: f64 = 2.0;

const fn default_warning_multiplier() -> f64 {
    DEFAULT_WARNING_MULTIPLIER
}

const fn default_enable_logging() -> bool {
    true
}

const fn default_enable_warnings() -> bool {
    true
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
    fn test_partial_deserialization() {
        let toml = r#"
            warning_multiplier = 1.5
        "#;
        let policy: MetricsPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.warning_multiplier, 1.5);
        assert!(policy.enable_logging);
    }
}
