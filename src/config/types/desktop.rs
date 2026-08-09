//! `[desktop]` config section — the daemon-side consumers of the desktop
//! capability layer (FEATURE_LOCATOR §7.6).
//!
//! The desktop *capabilities* themselves are constructed at the single
//! per-OS injection point (`executor::builtin_registry::builder::constructor`)
//! and the power capability in the binary boot path. This section only carries
//! the **policy** knobs for the daemon-side *consumers* of those capabilities —
//! today exactly one, the input-rail policy the desktop tool enforces
//! (`allow_global_pointer`).
//!
//! `[desktop.presence]` and `[desktop.mic_level]` were the other two until
//! 2026-08-09; both reporters were removed (see `crate::tasks`). `Config` does
//! not `deny_unknown_fields`, so a `config.toml` still carrying those tables
//! parses unchanged — the keys are ignored rather than rejected, and no
//! migration is needed.

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

    #[test]
    fn empty_table_uses_inner_defaults() {
        let d: DesktopDaemonConfig = toml::from_str("").expect("parse empty");
        assert!(!d.allow_global_pointer);
    }
}
