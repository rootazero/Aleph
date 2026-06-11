//! `PresenceReporter` configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the `PresenceReporter` background task.
///
/// Defaults are conservative: enabled, 30-second tick.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PresenceConfig {
    /// Whether to emit presence snapshots on the Gateway event bus.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// How often to collect and publish a snapshot (seconds).
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
}

const fn default_enabled() -> bool {
    true
}

const fn default_interval_secs() -> u64 {
    30
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            interval_secs: default_interval_secs(),
        }
    }
}

impl PresenceConfig {
    /// Clamp interval to a sane range — anything below 5s would spam the bus.
    #[must_use]
    pub fn effective_interval_secs(&self) -> u64 {
        self.interval_secs.max(5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled_at_thirty_seconds() {
        let c = PresenceConfig::default();
        assert!(c.enabled);
        assert_eq!(c.interval_secs, 30);
        assert_eq!(c.effective_interval_secs(), 30);
    }

    #[test]
    fn interval_clamped_to_five_seconds_minimum() {
        let c = PresenceConfig {
            enabled: true,
            interval_secs: 1,
        };
        assert_eq!(c.effective_interval_secs(), 5);
    }
}
