//! Single source of truth for panel appearance preferences.
//!
//! Four orthogonal, client-side axes — all persisted to `localStorage` and
//! replayed on boot by [`init_appearance`]:
//!
//!   • mode      — System / Light / Dark / Vibrant → `<html>` class list
//!   • accent    — colour palette                  → `data-accent` attribute
//!   • font scale— accessibility text size         → `--control-ui-text-scale`
//!   • roundness — corner radius density           → `--control-ui-radius-scale`
//!
//! Each enum carries pure (web_sys-free) conversion logic so it unit-tests on
//! the host; the `read_*` / `apply_*` helpers touch the DOM. Both the topbar
//! `ThemeToggle` popover and the Appearance settings page consume this module,
//! so the read/apply/persist logic lives in exactly one place.

use wasm_bindgen::JsCast;
use web_sys::{Document, HtmlElement, Storage};

// localStorage keys. `theme`/`accent` predate this module — kept verbatim for
// backward compatibility with already-persisted user preferences.
const KEY_MODE: &str = "aleph-theme";
const KEY_ACCENT: &str = "aleph-accent";
const KEY_FONT_SCALE: &str = "aleph-font-scale";
const KEY_ROUNDNESS: &str = "aleph-roundness";

// ---------------------------------------------------------------------------
// Theme mode
// ---------------------------------------------------------------------------

/// Light/dark surface family. Drives the `<html>` class list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
    Vibrant,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 4] = [Self::System, Self::Light, Self::Dark, Self::Vibrant];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "明亮",
            Self::Dark => "暗黑",
            Self::Vibrant => "玻璃",
        }
    }

    /// `localStorage` value, or `None` for `System` (which clears the key).
    pub fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
            Self::Vibrant => Some("translucent"),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            Some("translucent") => Self::Vibrant,
            _ => Self::System,
        }
    }
}

// ---------------------------------------------------------------------------
// Accent palette
// ---------------------------------------------------------------------------

/// Accent colour. `Mauve` is the base theme (clears `data-accent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    Mauve,
    Ocean,
    Forest,
    Sunset,
    Rose,
}

impl Accent {
    pub const ALL: [Accent; 5] = [
        Self::Mauve,
        Self::Ocean,
        Self::Forest,
        Self::Sunset,
        Self::Rose,
    ];

    /// Stable id used for both the `data-accent` attribute and persistence.
    pub fn id(self) -> &'static str {
        match self {
            Self::Mauve => "mauve",
            Self::Ocean => "ocean",
            Self::Forest => "forest",
            Self::Sunset => "sunset",
            Self::Rose => "rose",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mauve => "魅紫",
            Self::Ocean => "海蓝",
            Self::Forest => "森绿",
            Self::Sunset => "暖橙",
            Self::Rose => "玫瑰",
        }
    }

    /// Representative swatch colour (oklch) for UI previews.
    pub fn swatch(self) -> &'static str {
        match self {
            Self::Mauve => "oklch(0.60 0.13 310)",
            Self::Ocean => "oklch(0.58 0.13 250)",
            Self::Forest => "oklch(0.55 0.12 150)",
            Self::Sunset => "oklch(0.66 0.135 60)",
            Self::Rose => "oklch(0.62 0.15 15)",
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("ocean") => Self::Ocean,
            Some("forest") => Self::Forest,
            Some("sunset") => Self::Sunset,
            Some("rose") => Self::Rose,
            _ => Self::Mauve,
        }
    }
}

// ---------------------------------------------------------------------------
// Font scale (accessibility text size)
// ---------------------------------------------------------------------------

/// Global UI text scale. Wires the `--control-ui-text-scale` CSS hook that
/// every `rem` in the panel keys off of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontScale {
    Compact,  // 90%
    Default,  // 100%
    Cozy,     // 110%
    Large,    // 125%
    Largest,  // 140%
}

impl FontScale {
    pub const ALL: [FontScale; 5] = [
        Self::Compact,
        Self::Default,
        Self::Cozy,
        Self::Large,
        Self::Largest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "紧凑",
            Self::Default => "标准",
            Self::Cozy => "舒适",
            Self::Large => "大",
            Self::Largest => "特大",
        }
    }

    /// CSS multiplier applied to the root font-size knob.
    pub fn css_value(self) -> &'static str {
        match self {
            Self::Compact => "0.9",
            Self::Default => "1",
            Self::Cozy => "1.1",
            Self::Large => "1.25",
            Self::Largest => "1.4",
        }
    }

    /// `localStorage` value, or `None` for the default (clears the key).
    pub fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            other => Some(other.css_value()),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("0.9") => Self::Compact,
            Some("1.1") => Self::Cozy,
            Some("1.25") => Self::Large,
            Some("1.4") => Self::Largest,
            _ => Self::Default,
        }
    }
}

// ---------------------------------------------------------------------------
// Roundness (corner radius density)
// ---------------------------------------------------------------------------

/// Corner-radius density. Drives `--control-ui-radius-scale`, the multiplier
/// the `--radius-*` design tokens key off of (`--radius-full` stays pill-shaped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roundness {
    Sharp,   // 0×  — square corners
    Slight,  // 0.5×
    Default, // 1×
    Round,   // 1.5×
    Extra,   // 2×
}

impl Roundness {
    pub const ALL: [Roundness; 5] = [
        Self::Sharp,
        Self::Slight,
        Self::Default,
        Self::Round,
        Self::Extra,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sharp => "直角",
            Self::Slight => "微圆",
            Self::Default => "标准",
            Self::Round => "圆润",
            Self::Extra => "极圆",
        }
    }

    /// CSS multiplier applied to the radius tokens.
    pub fn css_value(self) -> &'static str {
        match self {
            Self::Sharp => "0",
            Self::Slight => "0.5",
            Self::Default => "1",
            Self::Round => "1.5",
            Self::Extra => "2",
        }
    }

    /// `localStorage` value, or `None` for the default (clears the key).
    pub fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            other => Some(other.css_value()),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("0") => Self::Sharp,
            Some("0.5") => Self::Slight,
            Some("1.5") => Self::Round,
            Some("2") => Self::Extra,
            _ => Self::Default,
        }
    }
}

// ---------------------------------------------------------------------------
// DOM / storage plumbing
// ---------------------------------------------------------------------------

fn document() -> Option<Document> {
    web_sys::window().and_then(|w| w.document())
}

fn root() -> Option<HtmlElement> {
    document()
        .and_then(|d| d.document_element())
        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
}

fn storage() -> Option<Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Persist a key, or remove it when `value` is `None` (i.e. the default).
fn persist(key: &str, value: Option<&str>) {
    if let Some(s) = storage() {
        match value {
            Some(v) => {
                let _ = s.set_item(key, v);
            }
            None => {
                let _ = s.remove_item(key);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

fn read_key(key: &str) -> Option<String> {
    storage().and_then(|s| s.get_item(key).ok().flatten())
}

pub fn read_mode() -> ThemeMode {
    ThemeMode::from_storage(read_key(KEY_MODE).as_deref())
}

pub fn read_accent() -> Accent {
    Accent::from_storage(read_key(KEY_ACCENT).as_deref())
}

pub fn read_font_scale() -> FontScale {
    FontScale::from_storage(read_key(KEY_FONT_SCALE).as_deref())
}

pub fn read_roundness() -> Roundness {
    Roundness::from_storage(read_key(KEY_ROUNDNESS).as_deref())
}

// ---------------------------------------------------------------------------
// Applies (mutate the DOM + persist)
// ---------------------------------------------------------------------------

pub fn apply_mode(mode: ThemeMode) {
    if let Some(html) = root() {
        let classes = html.class_list();
        let _ = classes.remove_3("dark", "light", "translucent");
        match mode {
            ThemeMode::Light => {
                let _ = classes.add_1("light");
            }
            ThemeMode::Dark => {
                let _ = classes.add_1("dark");
            }
            ThemeMode::Vibrant => {
                let _ = classes.add_2("dark", "translucent");
            }
            ThemeMode::System => {}
        }
    }
    persist(KEY_MODE, mode.storage_value());
}

pub fn apply_accent(accent: Accent) {
    if let Some(html) = root() {
        if accent == Accent::Mauve {
            let _ = html.remove_attribute("data-accent");
        } else {
            let _ = html.set_attribute("data-accent", accent.id());
        }
    }
    // Mauve is the base palette → clear the key.
    let stored = (accent != Accent::Mauve).then_some(accent.id());
    persist(KEY_ACCENT, stored);
}

pub fn apply_font_scale(scale: FontScale) {
    if let Some(html) = root() {
        let _ = html
            .style()
            .set_property("--control-ui-text-scale", scale.css_value());
    }
    persist(KEY_FONT_SCALE, scale.storage_value());
}

pub fn apply_roundness(roundness: Roundness) {
    if let Some(html) = root() {
        let _ = html
            .style()
            .set_property("--control-ui-radius-scale", roundness.css_value());
    }
    persist(KEY_ROUNDNESS, roundness.storage_value());
}

/// Replay every persisted appearance axis onto the DOM. Called once on boot
/// (before the app mounts) so the first paint already reflects user choices.
pub fn init_appearance() {
    // Mode + accent: only touch the DOM for non-default values so System /
    // Mauve keep relying on the CSS `@media` / base-palette fallbacks.
    let mode = read_mode();
    if mode != ThemeMode::System {
        apply_mode(mode);
    }
    let accent = read_accent();
    if accent != Accent::Mauve {
        apply_accent(accent);
    }
    let scale = read_font_scale();
    if scale != FontScale::Default {
        apply_font_scale(scale);
    }
    let roundness = read_roundness();
    if roundness != Roundness::Default {
        apply_roundness(roundness);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_storage_round_trips() {
        for m in ThemeMode::ALL {
            assert_eq!(ThemeMode::from_storage(m.storage_value()), m);
        }
    }

    #[test]
    fn mode_system_clears_key() {
        assert_eq!(ThemeMode::System.storage_value(), None);
        assert_eq!(ThemeMode::from_storage(None), ThemeMode::System);
        assert_eq!(ThemeMode::from_storage(Some("garbage")), ThemeMode::System);
    }

    #[test]
    fn accent_id_round_trips() {
        for a in Accent::ALL {
            assert_eq!(Accent::from_storage(Some(a.id())), a);
        }
        // Unknown / mauve both fall back to the base palette.
        assert_eq!(Accent::from_storage(None), Accent::Mauve);
        assert_eq!(Accent::from_storage(Some("nope")), Accent::Mauve);
    }

    #[test]
    fn font_scale_round_trips_via_css_value() {
        for f in FontScale::ALL {
            assert_eq!(FontScale::from_storage(Some(f.css_value())), f);
        }
        assert_eq!(FontScale::Default.storage_value(), None);
        assert_eq!(FontScale::from_storage(None), FontScale::Default);
    }

    #[test]
    fn roundness_round_trips_via_css_value() {
        for r in Roundness::ALL {
            assert_eq!(Roundness::from_storage(Some(r.css_value())), r);
        }
        assert_eq!(Roundness::Default.storage_value(), None);
        assert_eq!(Roundness::from_storage(None), Roundness::Default);
    }

    #[test]
    fn non_default_values_persist_a_key() {
        // Every non-default variant must produce a storable value so the
        // choice survives a reload (the boot replay relies on this).
        assert!(FontScale::Largest.storage_value().is_some());
        assert!(Roundness::Sharp.storage_value().is_some());
        assert!(ThemeMode::Vibrant.storage_value().is_some());
    }
}
