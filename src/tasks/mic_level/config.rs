//! Config for the mic-level reporter daemon.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the mic-level reporter.
///
/// Disabled by default — keeping the `AVAudioEngine` tap alive shows the
/// orange "mic in use" indicator persistently, which is user-visible and not
/// something to opt people into silently.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MicLevelConfig {
    /// Master switch. Default `false`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Poll interval in seconds. Default 5s. Floored to 1s to avoid pinning
    /// a CPU on the bridge meter actor.
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    /// RMS threshold above which a sample is classified `Active`. Default
    /// 0.05 (~ a quiet voice at conversational distance).
    #[serde(default = "default_active_threshold")]
    pub active_threshold: f32,
    /// Only emit an event when the categorical `state` changes (Active ↔
    /// Quiet ↔ Unavailable). Reduces bus noise. Default `true`.
    #[serde(default = "default_emit_on_change_only")]
    pub emit_on_change_only: bool,
}

const fn default_enabled() -> bool {
    false
}

const fn default_interval_secs() -> u64 {
    5
}

const fn default_active_threshold() -> f32 {
    0.05
}

const fn default_emit_on_change_only() -> bool {
    true
}

impl Default for MicLevelConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            interval_secs: default_interval_secs(),
            active_threshold: default_active_threshold(),
            emit_on_change_only: default_emit_on_change_only(),
        }
    }
}

impl MicLevelConfig {
    #[must_use]
    pub fn effective_interval_secs(&self) -> u64 {
        self.interval_secs.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_with_5s_interval() {
        let c = MicLevelConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.effective_interval_secs(), 5);
        assert!((c.active_threshold - 0.05).abs() < 1e-6);
        assert!(c.emit_on_change_only);
    }

    #[test]
    fn interval_floors_at_one_second() {
        let c = MicLevelConfig {
            interval_secs: 0,
            ..Default::default()
        };
        assert_eq!(c.effective_interval_secs(), 1);
    }
}
