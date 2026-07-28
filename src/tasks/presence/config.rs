//! `PresenceReporter` configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the `PresenceReporter` background task.
///
/// **Default is `enabled = false`.** Presence snapshots carry the device
/// hostname and the system username — both are PII on most systems — and
/// are broadcast on the Gateway event bus to every subscriber. Broadcasting
/// them without an explicit opt-in risks leaking identity to remote channels
/// or shared log sinks, so the reporter stays off until the operator turns
/// it on.
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
    false
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
    fn default_is_disabled_opt_in() {
        let c = PresenceConfig::default();
        assert!(
            !c.enabled,
            "presence must default to disabled to avoid PII leak"
        );
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
