//! Config for the mic-level reporter daemon.

use serde::{Deserialize, Serialize};

/// Configuration for the mic-level reporter.
///
/// Disabled by default — keeping the `AVAudioEngine` tap alive shows the
/// orange "mic in use" indicator persistently, which is user-visible and not
/// something to opt people into silently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicLevelConfig {
    /// Master switch. Default `false`.
    pub enabled: bool,
    /// Poll interval in seconds. Default 5s. Floored to 1s to avoid pinning
    /// a CPU on the bridge meter actor.
    pub interval_secs: u64,
    /// RMS threshold above which a sample is classified `Active`. Default
    /// 0.05 (~ a quiet voice at conversational distance).
    pub active_threshold: f32,
    /// Only emit an event when the categorical `state` changes (Active ↔
    /// Quiet ↔ Unavailable). Reduces bus noise. Default `true`.
    pub emit_on_change_only: bool,
}

impl Default for MicLevelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 5,
            active_threshold: 0.05,
            emit_on_change_only: true,
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
