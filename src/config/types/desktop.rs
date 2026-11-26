//! `[desktop]` config section — the daemon-side consumers of the desktop
//! capability layer (FEATURE_LOCATOR §7.6).
//!
//! The desktop *capabilities* themselves are constructed at the single
//! per-OS injection point (`executor::builtin_registry::builder::constructor`)
//! and the power and voice capabilities in the binary boot path. This section
//! holds daemon-side platform settings, not provider settings exposed to Panel.
//!
//! `[desktop.presence]` and `[desktop.mic_level]` were the other two until
//! 2026-08-09; both reporters were removed (see `crate::tasks`). `Config` does
//! not `deny_unknown_fields`, so a `config.toml` still carrying those tables
//! parses unchanged — the keys are ignored rather than rejected, and no
//! migration is needed.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Desktop daemon-consumer settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DesktopDaemonConfig {
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

    /// Optional provisioned recognizer for the Linux desktop fallback. Separate
    /// from `[voice.local]`; never starts a download or searches the PATH.
    /// Stop the daemon before provisioning or removing this local installation
    /// in `config.toml`. The installation is only read at startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_stt: Option<DesktopOfflineSttConfig>,
}

/// Installation paths for an administrator-provisioned offline recognizer.
/// Both paths must be present and absolute to mount it. An incomplete table
/// leaves the recognizer unmounted without preventing the daemon from starting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DesktopOfflineSttConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// An existing `config.toml` still carrying the two removed reporter
    /// tables must keep parsing. Deleting a config field is a wire change for
    /// every machine that already wrote it; this is the assertion that says
    /// the change needs no migration.
    #[test]
    fn stale_reporter_tables_are_ignored_not_rejected() {
        let toml = r#"
            allow_global_pointer = true

            [presence]
            enabled = true
            interval_secs = 120

            [mic_level]
            enabled = true
            interval_secs = 2
        "#;
        let d: DesktopDaemonConfig = toml::from_str(toml).expect("stale tables must not fail boot");
        assert!(d.allow_global_pointer);
    }

    #[derive(Default, Deserialize, Serialize)]
    struct DaemonConfigFixture {
        #[serde(default)]
        desktop: DesktopDaemonConfig,
    }

    #[test]
    fn offline_stt_defaults_to_unconfigured() {
        for input in ["", "[desktop]\nallow_global_pointer = true"] {
            let config: DaemonConfigFixture = toml::from_str(input).expect("desktop config parses");
            assert!(config.desktop.offline_stt.is_none());
            let serialized = toml::to_string(&config).expect("desktop config serializes");
            assert!(!serialized.contains("offline_stt"));
        }
    }

    #[test]
    fn offline_stt_round_trips_without_a_voice_provider() {
        let input = r#"
            [desktop.offline_stt]
            executable = "/opt/whisper/bin/whisper-cli"
            model = "/opt/whisper/models/ggml-base.bin"
        "#;
        let config: DaemonConfigFixture = toml::from_str(input).expect("desktop config parses");
        let specified = config
            .desktop
            .offline_stt
            .as_ref()
            .expect("installation configured");
        assert_eq!(
            specified.executable.as_deref(),
            Some(std::path::Path::new("/opt/whisper/bin/whisper-cli"))
        );
        assert_eq!(
            specified.model.as_deref(),
            Some(std::path::Path::new("/opt/whisper/models/ggml-base.bin"))
        );
        let serialized = toml::to_string(&config).expect("desktop config serializes");
        let reloaded: DaemonConfigFixture =
            toml::from_str(&serialized).expect("desktop config reloads");
        let installed = reloaded
            .desktop
            .offline_stt
            .expect("installation preserved");
        assert_eq!(installed.executable, specified.executable);
        assert_eq!(installed.model, specified.model);
        assert!(!serialized.contains("[voice"));
    }

    #[test]
    fn offline_stt_incomplete_installation_does_not_prevent_boot() {
        let input = "[desktop.offline_stt]\nexecutable = '/opt/whisper/bin/whisper-cli'";
        let config: DaemonConfigFixture =
            toml::from_str(input).expect("incomplete desktop config parses");
        let partial = config
            .desktop
            .offline_stt
            .expect("partial installation is observable");
        assert!(partial.executable.is_some());
        assert!(partial.model.is_none());
    }

    #[test]
    fn empty_table_uses_inner_defaults() {
        let d: DesktopDaemonConfig = toml::from_str("").expect("parse empty");
        assert!(!d.allow_global_pointer);
    }
}
