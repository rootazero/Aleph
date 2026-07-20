//! `[desktop]` config section — the daemon-side consumers of the desktop
//! capability layer (FEATURE_LOCATOR §7.6).
//!
//! The desktop *capabilities* themselves are constructed at the single
//! per-OS injection point (`executor::builtin_registry::builder::constructor`)
//! and the power capability in the binary boot path. This section only carries
//! the **policy** knobs for the daemon-side *consumers* of those capabilities:
//! the presence broadcaster, the mic-level meter, and the one input-rail policy
//! the desktop tool enforces (`allow_global_pointer`).
//!
//! The two reporter structs are reused verbatim from `crate::tasks::*` (the
//! modules that own the reporters) so the config layer and the consumer share
//! one definition — no duplicated schema, no drift.

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

    /// Permit coordinate-space desktop actions (click / drag / scroll /
    /// `type_text` / …) that name **no target process** and therefore run on the
    /// global HID event tap: they physically drag the user's cursor across the
    /// screen and only land where the target app is already frontmost.
    ///
    /// Default `false` — a platform that can deliver input into a single
    /// process (`ScreenCapability::supports_targeted_input`) refuses the
    /// intrusive path instead and tells the model to pass `app` / `pid`, or to
    /// use `set_value` / `ax_action`. The refusal is fail-closed by design: the
    /// tool never "tries targeted and silently falls back to global", because
    /// picking a recovery for the model is the harness overruling it (R10) —
    /// the error is compressed into the result and the model decides (A2).
    ///
    /// On a platform with **no** targeted rail (Windows, Linux today) this knob
    /// is never consulted and behavior is unchanged: there is nothing to refuse
    /// in favour of.
    #[serde(default)]
    pub allow_global_pointer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_reporter_defaults() {
        let d = DesktopDaemonConfig::default();
        // Both presence and mic-level ship off by default. Both publish
        // privacy-sensitive data (hostname + username / mic amplitude) on
        // the Gateway event bus and must be opted in explicitly.
        assert!(!d.presence.enabled);
        assert!(!d.mic_level.enabled);
    }

    #[test]
    fn global_pointer_is_denied_by_default() {
        // The intrusive rail (moves the user's real cursor, needs the app
        // frontmost) is opt-in. Nothing about the platform is consulted here —
        // the tool gates on supports_targeted_input(), so a platform without a
        // background rail is unaffected by this default.
        assert!(!DesktopDaemonConfig::default().allow_global_pointer);
    }

    #[test]
    fn global_pointer_can_be_opted_into() {
        let d: DesktopDaemonConfig =
            toml::from_str("allow_global_pointer = true").expect("parse [desktop]");
        assert!(d.allow_global_pointer);
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
        assert!(!d.presence.enabled);
        assert!(!d.mic_level.enabled);
    }
}
