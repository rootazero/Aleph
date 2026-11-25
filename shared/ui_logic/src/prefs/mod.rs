//! Personal Panel preferences: appearance and keyboard shortcuts.
//!
//! Preferences are per device and stored by the Panel host under
//! [`PREFS_STORAGE_KEY`]. Unlike `panel::layout`, they are not scoped to a
//! Gateway endpoint or subject. This module is not behind the `panel`
//! feature, because the Leptos Panel's default build reads the appearance
//! enums from here too.

pub mod appearance;
pub mod commands;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use appearance::{Accent, Appearance, Density, FontScale, Roundness, ThemeMode};

/// Storage key for [`PanelPrefs`] JSON.
pub const PREFS_STORAGE_KEY: &str = "aleph.panel.prefs.v1";

/// The only preference format this build reads.
pub const PREFS_VERSION: u32 = 1;

/// A user's choice for one command's key.
///
/// Serialised as `{"key": "mod+k"}` or `"cleared"`. A command with no entry
/// uses its default key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Binding {
    Key(String),
    Cleared,
}

/// Everything the Panel stores about personal preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelPrefs {
    pub version: u32,
    pub appearance: Appearance,
    pub keybindings: BTreeMap<String, Binding>,
}

impl Default for PanelPrefs {
    fn default() -> Self {
        Self {
            version: PREFS_VERSION,
            appearance: Appearance::default(),
            keybindings: BTreeMap::new(),
        }
    }
}

impl PanelPrefs {
    /// The JSON written back to storage.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Why stored preferences were replaced by defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsRepair {
    /// The stored text was not a readable preference object.
    Corrupt,
    /// The stored object carries a version this build does not read.
    UnknownVersion(u32),
}

/// Preferences read from storage, plus the repair applied, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredPrefs {
    pub prefs: PanelPrefs,
    pub repair: Option<PrefsRepair>,
}

/// Read stored preferences. Nothing stored gives defaults with no repair;
/// unreadable text or a missing/unknown version gives defaults with a repair
/// so the caller writes the defaults back. Unknown fields are ignored.
#[must_use]
pub fn restore(stored: Option<&str>) -> RestoredPrefs {
    let Some(text) = stored else {
        return RestoredPrefs {
            prefs: PanelPrefs::default(),
            repair: None,
        };
    };
    let reset = |repair| RestoredPrefs {
        prefs: PanelPrefs::default(),
        repair: Some(repair),
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return reset(PrefsRepair::Corrupt);
    };
    let version = value.get("version").and_then(serde_json::Value::as_u64);
    match version {
        None => return reset(PrefsRepair::Corrupt),
        Some(version) if version != u64::from(PREFS_VERSION) => {
            let version = u32::try_from(version).unwrap_or(u32::MAX);
            return reset(PrefsRepair::UnknownVersion(version));
        }
        Some(_) => {}
    }
    match serde_json::from_value::<PanelPrefs>(value) {
        Ok(prefs) => RestoredPrefs {
            prefs,
            repair: None,
        },
        Err(_) => reset(PrefsRepair::Corrupt),
    }
}

/// Set one appearance axis from its serialised name and value, e.g.
/// `("accent", "ocean")`. Returns false and changes nothing when the axis or
/// value is unknown.
pub fn set_appearance(prefs: &mut PanelPrefs, axis: &str, value: &str) -> bool {
    fn parse<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
        serde_json::from_value(serde_json::Value::from(value)).ok()
    }
    let appearance = &mut prefs.appearance;
    let applied = match axis {
        "mode" => parse::<ThemeMode>(value).map(|v| appearance.mode = v),
        "accent" => parse::<Accent>(value).map(|v| appearance.accent = v),
        "fontScale" => parse::<FontScale>(value).map(|v| appearance.font_scale = v),
        "roundness" => parse::<Roundness>(value).map(|v| appearance.roundness = v),
        "density" => parse::<Density>(value).map(|v| appearance.density = v),
        _ => None,
    };
    applied.is_some()
}

/// Set or remove a user binding. `None` restores the command's default.
pub fn set_binding(prefs: &mut PanelPrefs, command: &str, binding: Option<Binding>) {
    match binding {
        Some(binding) => {
            prefs.keybindings.insert(command.to_owned(), binding);
        }
        None => {
            prefs.keybindings.remove(command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_none_is_default_without_repair() {
        let restored = restore(None);
        assert_eq!(restored.prefs, PanelPrefs::default());
        assert_eq!(restored.repair, None);
    }

    #[test]
    fn restore_corrupt_reports_corrupt() {
        for text in [
            "{",
            "[]",
            "{}",
            r#"{"version":1,"appearance":{"accent":"plaid"}}"#,
        ] {
            let restored = restore(Some(text));
            assert_eq!(restored.prefs, PanelPrefs::default(), "{text}");
            assert_eq!(restored.repair, Some(PrefsRepair::Corrupt), "{text}");
        }
    }

    #[test]
    fn restore_unknown_version_resets() {
        let restored = restore(Some(r#"{"version":9}"#));
        assert_eq!(restored.prefs, PanelPrefs::default());
        assert_eq!(restored.repair, Some(PrefsRepair::UnknownVersion(9)));
    }

    #[test]
    fn restore_ignores_unknown_fields() {
        let restored = restore(Some(
            r#"{"version":1,"future":true,"appearance":{"accent":"rose","glow":3}}"#,
        ));
        assert_eq!(restored.repair, None);
        assert_eq!(restored.prefs.appearance.accent, Accent::Rose);
        assert_eq!(restored.prefs.appearance.mode, ThemeMode::System);
    }

    #[test]
    fn roundtrip_keeps_overrides() {
        let mut prefs = PanelPrefs::default();
        set_binding(
            &mut prefs,
            "shell.palette",
            Some(Binding::Key("mod+j".into())),
        );
        set_binding(&mut prefs, "shell.settings", Some(Binding::Cleared));
        let json = prefs.to_json();
        assert!(
            json.contains(r#""shell.palette":{"key":"mod+j"}"#),
            "{json}"
        );
        assert!(json.contains(r#""shell.settings":"cleared""#), "{json}");
        let restored = restore(Some(&json));
        assert_eq!(restored.repair, None);
        assert_eq!(restored.prefs, prefs);
    }

    #[test]
    fn set_binding_none_restores_default() {
        let mut prefs = PanelPrefs::default();
        set_binding(&mut prefs, "shell.palette", Some(Binding::Cleared));
        set_binding(&mut prefs, "shell.palette", None);
        assert!(prefs.keybindings.is_empty());
    }

    #[test]
    fn set_appearance_accepts_known_axes_only() {
        let mut prefs = PanelPrefs::default();
        assert!(set_appearance(&mut prefs, "accent", "ocean"));
        assert!(set_appearance(&mut prefs, "fontScale", "large"));
        assert!(set_appearance(&mut prefs, "mode", "dark"));
        assert_eq!(prefs.appearance.accent, Accent::Ocean);
        assert_eq!(prefs.appearance.font_scale, FontScale::Large);
        assert_eq!(prefs.appearance.mode, ThemeMode::Dark);
        let before = prefs.clone();
        assert!(!set_appearance(&mut prefs, "material", "liquid"));
        assert!(!set_appearance(&mut prefs, "accent", "plaid"));
        assert_eq!(prefs, before);
    }
}
