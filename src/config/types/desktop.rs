//! `[desktop]` config section — the daemon-side consumers of the desktop
//! capability layer (FEATURE_LOCATOR §7.6).
//!
//! The desktop *capabilities* themselves are constructed at the single
//! per-OS injection point (`executor::builtin_registry::builder::constructor`)
//! and the power capability in the binary boot path. This section only carries
//! the **policy** knobs for the daemon-side *consumers* of those capabilities:
//! the presence broadcaster and the mic-level meter.
//!
//! Both inner structs are reused verbatim from `crate::tasks::*` (the modules
//! that own the reporters) so the config layer and the consumer share one
//! definition — no duplicated schema, no drift.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::tasks::mic_level::MicLevelConfig;
pub use crate::tasks::presence::PresenceConfig;

/// Desktop daemon-consumer settings.
///
/// When `[desktop]` is absent from `config.toml`, `Default` reproduces the
/// historical hardcoded behavior exactly: presence on at 30 s, mic-level off.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DesktopDaemonConfig {
    /// Periodic broadcast of `(hostname, username, platform, idle)` on the
    /// Gateway event bus. Enabled by default.
    #[serde(default)]
    pub presence: PresenceConfig,

    /// Periodic mic-level meter snapshots on the Gateway event bus. Disabled
    /// by default — the live meter keeps the OS "mic in use" indicator lit, so
    /// it is opt-in.
    #[serde(default)]
    pub mic_level: MicLevelConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_reporter_defaults() {
        let d = DesktopDaemonConfig::default();
        // Presence reporter ships on; mic-level ships off (opt-in).
        assert!(d.presence.enabled);
        assert!(!d.mic_level.enabled);
    }

    #[test]
    fn deserializes_nested_sections() {
        let toml = r#"
            [presence]
            enabled = false
            interval_secs = 120

            [mic_level]
            enabled = true
            interval_secs = 2
        "#;
        let d: DesktopDaemonConfig = toml::from_str(toml).expect("parse [desktop]");
        assert!(!d.presence.enabled);
        assert_eq!(d.presence.interval_secs, 120);
        assert!(d.mic_level.enabled);
        assert_eq!(d.mic_level.interval_secs, 2);
    }

    #[test]
    fn empty_table_uses_inner_defaults() {
        let d: DesktopDaemonConfig = toml::from_str("").expect("parse empty");
        assert!(d.presence.enabled);
        assert!(!d.mic_level.enabled);
    }
}
