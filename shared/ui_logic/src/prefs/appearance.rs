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

/// The colours a palette contributes per colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPalette {
    /// Primary action colour (`--color-primary` in the Leptos stylesheet).
    pub primary: &'static str,
    /// Selected-row tint (`--color-sidebar-active` in the Leptos stylesheet).
    pub selected: &'static str,
    /// Text drawn on the primary colour, chosen per palette so the pair
    /// reaches the 4.5:1 contrast normal text needs.
    pub on_primary: &'static str,
}

/// Near-white text for a dark primary colour.
const LIGHT_TEXT: &str = "oklch(0.985 0 0)";
/// Near-black text for a light primary colour.
const DARK_TEXT: &str = "oklch(0.205 0 0)";

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
        let (primary, selected, on_primary) = match (self, dark) {
            (Self::Mauve, false) => ("oklch(0.55 0.120 310)", "oklch(0.95 0.015 310)", LIGHT_TEXT),
            (Self::Mauve, true) => ("oklch(0.65 0.120 310)", "oklch(0.22 0.035 310)", DARK_TEXT),
            (Self::Ocean, false) => ("oklch(0.55 0.130 250)", "oklch(0.95 0.022 250)", LIGHT_TEXT),
            (Self::Ocean, true) => ("oklch(0.68 0.130 250)", "oklch(0.24 0.040 250)", DARK_TEXT),
            (Self::Forest, false) => ("oklch(0.53 0.115 150)", "oklch(0.95 0.022 150)", LIGHT_TEXT),
            (Self::Forest, true) => ("oklch(0.66 0.120 150)", "oklch(0.24 0.038 150)", DARK_TEXT),
            (Self::Sunset, false) => ("oklch(0.62 0.135 60)", "oklch(0.95 0.025 60)", DARK_TEXT),
            (Self::Sunset, true) => ("oklch(0.74 0.135 65)", "oklch(0.26 0.040 65)", DARK_TEXT),
            (Self::Rose, false) => ("oklch(0.57 0.150 15)", "oklch(0.95 0.025 15)", LIGHT_TEXT),
            (Self::Rose, true) => ("oklch(0.70 0.150 15)", "oklch(0.25 0.045 15)", DARK_TEXT),
        };
        AccentPalette {
            primary,
            selected,
            on_primary,
        }
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
        tokens.insert(name.to_owned(), Value::from(palette.on_primary));
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
        assert_eq!(vars["light"]["primary-foreground"], LIGHT_TEXT);
        assert_eq!(vars["dark"]["primary-foreground"], DARK_TEXT);
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

    /// WCAG relative luminance of an `oklch(L C h)` colour, clamped to sRGB.
    fn luminance(oklch: &str) -> f64 {
        let inner = oklch.trim_start_matches("oklch(").trim_end_matches(')');
        let parts: Vec<f64> = inner
            .split_whitespace()
            .map(|part| part.parse().unwrap())
            .collect();
        let (lightness, chroma, hue) = (parts[0], parts[1], parts[2].to_radians());
        let (a, b) = (chroma * hue.cos(), chroma * hue.sin());
        let l = (lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
        let m = (lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
        let s = (lightness - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
        let red = 4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s;
        let green = -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s;
        let blue = -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s;
        0.2126 * red.clamp(0.0, 1.0)
            + 0.7152 * green.clamp(0.0, 1.0)
            + 0.0722 * blue.clamp(0.0, 1.0)
    }

    #[test]
    fn text_on_every_primary_reaches_normal_text_contrast() {
        for accent in Accent::ALL {
            for dark in [false, true] {
                let palette = accent.palette(dark);
                let (first, second) = (luminance(palette.primary), luminance(palette.on_primary));
                let ratio = (first.max(second) + 0.05) / (first.min(second) + 0.05);
                assert!(ratio >= 4.5, "{accent:?} dark={dark}: {ratio:.2}:1");
            }
        }
    }
}
