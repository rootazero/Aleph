//! Appearance axes shared by the Leptos Panel and the TSX Panel.
//!
//! The enums and their storage values used to live in
//! `interfaces/webchat/src/appearance.rs`; they moved here unchanged so both
//! Panels read one definition. Storage values stay byte-identical to the
//! Leptos `localStorage` values that are already persisted on user machines.
//!
//! `theme_tokens` turns an [`Appearance`] into the NSH `setTheme` payload.
//! Accent colours are copied from `interfaces/webchat/styles/tailwind.css`;
//! `interfaces/webchat/src/appearance_drift.rs` fails when the two drift.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Light/dark surface family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// `localStorage` value, or `None` for `System` (which clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    #[must_use]
    pub fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("light") => Self::Light,
            // Legacy values: the retired Glass theme ("glass") and its
            // Vibrant-era predecessor ("translucent") were dark-based —
            // both load as Dark.
            Some("dark" | "glass" | "translucent") => Self::Dark,
            _ => Self::System,
        }
    }
}

/// Accent colour. `Mauve` is the base palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    #[default]
    Mauve,
    Ocean,
    Forest,
    Sunset,
    Rose,
}

/// The two accent colours a palette contributes per colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPalette {
    /// Primary action colour (`--color-primary` in the Leptos stylesheet).
    pub primary: &'static str,
    /// Selected-row tint (`--color-sidebar-active` in the Leptos stylesheet).
    pub selected: &'static str,
}

impl Accent {
    pub const ALL: [Self; 5] = [
        Self::Mauve,
        Self::Ocean,
        Self::Forest,
        Self::Sunset,
        Self::Rose,
    ];

    /// Stable id used for both the `data-accent` attribute and persistence.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Mauve => "mauve",
            Self::Ocean => "ocean",
            Self::Forest => "forest",
            Self::Sunset => "sunset",
            Self::Rose => "rose",
        }
    }

    /// Representative swatch colour (oklch) for UI previews.
    #[must_use]
    pub const fn swatch(self) -> &'static str {
        match self {
            Self::Mauve => "oklch(0.60 0.13 310)",
            Self::Ocean => "oklch(0.58 0.13 250)",
            Self::Forest => "oklch(0.55 0.12 150)",
            Self::Sunset => "oklch(0.66 0.135 60)",
            Self::Rose => "oklch(0.62 0.15 15)",
        }
    }

    /// `localStorage` value, or `None` for `Mauve` (which clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Mauve => None,
            other => Some(other.id()),
        }
    }

    #[must_use]
    pub fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("ocean") => Self::Ocean,
            Some("forest") => Self::Forest,
            Some("sunset") => Self::Sunset,
            Some("rose") => Self::Rose,
            _ => Self::Mauve,
        }
    }

    /// Palette colours for the light (`dark == false`) or dark scheme.
    #[must_use]
    pub const fn palette(self, dark: bool) -> AccentPalette {
        let (primary, selected) = match (self, dark) {
            (Self::Mauve, false) => ("oklch(0.55 0.120 310)", "oklch(0.95 0.015 310)"),
            (Self::Mauve, true) => ("oklch(0.65 0.120 310)", "oklch(0.22 0.035 310)"),
            (Self::Ocean, false) => ("oklch(0.55 0.130 250)", "oklch(0.95 0.022 250)"),
            (Self::Ocean, true) => ("oklch(0.68 0.130 250)", "oklch(0.24 0.040 250)"),
            (Self::Forest, false) => ("oklch(0.53 0.115 150)", "oklch(0.95 0.022 150)"),
            (Self::Forest, true) => ("oklch(0.66 0.120 150)", "oklch(0.24 0.038 150)"),
            (Self::Sunset, false) => ("oklch(0.62 0.135 60)", "oklch(0.95 0.025 60)"),
            (Self::Sunset, true) => ("oklch(0.74 0.135 65)", "oklch(0.26 0.040 65)"),
            (Self::Rose, false) => ("oklch(0.57 0.150 15)", "oklch(0.95 0.025 15)"),
            (Self::Rose, true) => ("oklch(0.70 0.150 15)", "oklch(0.25 0.045 15)"),
        };
        AccentPalette { primary, selected }
    }
}

/// Text size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontScale {
    Compact, // 90%
    #[default]
    Default, // 100%
    Cozy,    // 110%
    Large,   // 125%
    Largest, // 140%
}

impl FontScale {
    pub const ALL: [Self; 5] = [
        Self::Compact,
        Self::Default,
        Self::Cozy,
        Self::Large,
        Self::Largest,
    ];

    /// CSS multiplier applied to the root font-size knob.
    #[must_use]
    pub const fn css_value(self) -> &'static str {
        match self {
            Self::Compact => "0.9",
            Self::Default => "1",
            Self::Cozy => "1.1",
            Self::Large => "1.25",
            Self::Largest => "1.4",
        }
    }

    /// The same multiplier as a number, for hosts that take a scale factor.
    #[must_use]
    pub const fn factor(self) -> f32 {
        match self {
            Self::Compact => 0.9,
            Self::Default => 1.0,
            Self::Cozy => 1.1,
            Self::Large => 1.25,
            Self::Largest => 1.4,
        }
    }

    /// `localStorage` value, or `None` for the default (clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            other => Some(other.css_value()),
        }
    }

    #[must_use]
    pub fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("0.9") => Self::Compact,
            Some("1.1") => Self::Cozy,
            Some("1.25") => Self::Large,
            Some("1.4") => Self::Largest,
            _ => Self::Default,
        }
    }
}

/// Corner-radius density.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Roundness {
    Sharp,  // 0×  — square corners
    Slight, // 0.5×
    #[default]
    Default, // 1×
    Round,  // 1.5×
    Extra,  // 2×
}

impl Roundness {
    pub const ALL: [Self; 5] = [
        Self::Sharp,
        Self::Slight,
        Self::Default,
        Self::Round,
        Self::Extra,
    ];

    /// CSS multiplier applied to the radius tokens.
    #[must_use]
    pub const fn css_value(self) -> &'static str {
        match self {
            Self::Sharp => "0",
            Self::Slight => "0.5",
            Self::Default => "1",
            Self::Round => "1.5",
            Self::Extra => "2",
        }
    }

    /// The same multiplier as a number.
    #[must_use]
    pub const fn factor(self) -> f32 {
        match self {
            Self::Sharp => 0.0,
            Self::Slight => 0.5,
            Self::Default => 1.0,
            Self::Round => 1.5,
            Self::Extra => 2.0,
        }
    }

    /// `localStorage` value, or `None` for the default (clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            other => Some(other.css_value()),
        }
    }

    #[must_use]
    pub fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("0") => Self::Sharp,
            Some("0.5") => Self::Slight,
            Some("1.5") => Self::Round,
            Some("2") => Self::Extra,
            _ => Self::Default,
        }
    }
}

/// Whitespace compactness. `Compact` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Compact, // 1×
    Cozy,     // 1.13×
    Spacious, // 1.25×
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Cozy, Self::Spacious];

    /// CSS multiplier applied to the `--spacing` base.
    #[must_use]
    pub const fn css_value(self) -> &'static str {
        match self {
            Self::Compact => "1",
            Self::Cozy => "1.13",
            Self::Spacious => "1.25",
        }
    }

    /// `localStorage` value, or `None` for the default (clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Compact => None,
            other => Some(other.css_value()),
        }
    }

    #[must_use]
    pub fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("1.13") => Self::Cozy,
            Some("1.25") => Self::Spacious,
            _ => Self::Compact,
        }
    }
}

/// All five appearance axes the TSX Panel stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Appearance {
    pub mode: ThemeMode,
    pub accent: Accent,
    pub font_scale: FontScale,
    pub roundness: Roundness,
    pub density: Density,
}

/// NSH default corner radius, in rem. Roundness multiplies it.
const BASE_RADIUS_REM: f32 = 0.625;

/// Foreground drawn on top of the primary colour in both schemes.
const ON_PRIMARY: &str = "oklch(0.985 0 0)";

/// The NSH `setTheme` payload for `appearance`: a partial theme over the
/// shadcn tokens, carrying both colour schemes so switching light/dark needs
/// no second call. Mode itself is applied through `setColorScheme`.
#[must_use]
pub fn theme_tokens(appearance: &Appearance) -> Value {
    let radius = BASE_RADIUS_REM * appearance.roundness.factor();
    json!({
        "cssVars": {
            "theme": { "radius": format!("{radius}rem") },
            "light": scheme_tokens(appearance.accent.palette(false)),
            "dark": scheme_tokens(appearance.accent.palette(true)),
        }
    })
}

fn scheme_tokens(palette: AccentPalette) -> Value {
    let mut tokens = Map::new();
    for name in ["primary", "sidebar-primary", "ring", "chart-1"] {
        tokens.insert(name.to_owned(), Value::from(palette.primary));
    }
    tokens.insert("sidebar-accent".to_owned(), Value::from(palette.selected));
    for name in ["primary-foreground", "sidebar-primary-foreground"] {
        tokens.insert(name.to_owned(), Value::from(ON_PRIMARY));
    }
    Value::Object(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_values_roundtrip_for_all() {
        for value in ThemeMode::ALL {
            assert_eq!(ThemeMode::from_storage(value.storage_value()), value);
        }
        for value in Accent::ALL {
            assert_eq!(Accent::from_storage(value.storage_value()), value);
            assert_eq!(Accent::from_storage(Some(value.id())), value);
        }
        for value in FontScale::ALL {
            assert_eq!(FontScale::from_storage(value.storage_value()), value);
            assert_eq!(FontScale::from_storage(Some(value.css_value())), value);
        }
        for value in Roundness::ALL {
            assert_eq!(Roundness::from_storage(value.storage_value()), value);
            assert_eq!(Roundness::from_storage(Some(value.css_value())), value);
        }
        for value in Density::ALL {
            assert_eq!(Density::from_storage(value.storage_value()), value);
            assert_eq!(Density::from_storage(Some(value.css_value())), value);
        }
    }

    #[test]
    fn defaults_clear_the_key_and_unknown_values_fall_back() {
        assert_eq!(ThemeMode::System.storage_value(), None);
        assert_eq!(Accent::Mauve.storage_value(), None);
        assert_eq!(FontScale::Default.storage_value(), None);
        assert_eq!(Roundness::Default.storage_value(), None);
        assert_eq!(Density::Compact.storage_value(), None);
        assert_eq!(ThemeMode::from_storage(Some("garbage")), ThemeMode::System);
        assert_eq!(Accent::from_storage(Some("nope")), Accent::Mauve);
        assert_eq!(Density::from_storage(Some("garbage")), Density::Compact);
        assert!(FontScale::Largest.storage_value().is_some());
        assert!(Roundness::Sharp.storage_value().is_some());
        assert!(Density::Spacious.storage_value().is_some());
    }

    #[test]
    fn legacy_mode_values_load_dark() {
        assert_eq!(ThemeMode::from_storage(Some("glass")), ThemeMode::Dark);
        assert_eq!(
            ThemeMode::from_storage(Some("translucent")),
            ThemeMode::Dark
        );
    }

    #[test]
    fn factors_match_css_values() {
        for value in FontScale::ALL {
            assert_eq!(value.css_value().parse::<f32>().ok(), Some(value.factor()));
        }
        for value in Roundness::ALL {
            assert_eq!(value.css_value().parse::<f32>().ok(), Some(value.factor()));
        }
    }

    #[test]
    fn theme_tokens_emit_both_schemes() {
        let ocean = Appearance {
            accent: Accent::Ocean,
            ..Appearance::default()
        };
        let tokens = theme_tokens(&ocean);
        let vars = &tokens["cssVars"];
        assert_eq!(vars["light"]["primary"], "oklch(0.55 0.130 250)");
        assert_eq!(vars["dark"]["primary"], "oklch(0.68 0.130 250)");
        assert_eq!(vars["light"]["sidebar-accent"], "oklch(0.95 0.022 250)");
        assert_eq!(vars["dark"]["primary-foreground"], ON_PRIMARY);
        assert_eq!(vars["theme"]["radius"], "0.625rem");

        let sharp = Appearance {
            roundness: Roundness::Sharp,
            ..Appearance::default()
        };
        assert_eq!(theme_tokens(&sharp)["cssVars"]["theme"]["radius"], "0rem");
        let extra = Appearance {
            roundness: Roundness::Extra,
            ..Appearance::default()
        };
        assert_eq!(
            theme_tokens(&extra)["cssVars"]["theme"]["radius"],
            "1.25rem"
        );
    }

    #[test]
    fn appearance_serializes_with_camel_case_axes() {
        let value = serde_json::to_value(Appearance::default()).unwrap();
        assert_eq!(
            value,
            json!({"mode":"system","accent":"mauve","fontScale":"default","roundness":"default","density":"compact"})
        );
    }
}
