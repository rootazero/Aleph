//! Single source of truth for panel appearance preferences.
//!
//! Six orthogonal, client-side axes — all persisted to `localStorage` and
//! replayed on boot by [`init_appearance`]:
//!
//!   • mode      — System / Light / Dark           → `<html>` class list
//!   • accent    — colour palette                  → `data-accent` attribute
//!   • font scale— accessibility text size         → `--control-ui-text-scale`
//!   • roundness — corner radius density           → `--control-ui-radius-scale`
//!   • density   — whitespace compactness           → `--control-ui-density`
//!   • material  — glass material family           → `data-material` attribute
//!
//! Each enum carries pure (web_sys-free) conversion logic so it unit-tests on
//! the host; the `read_*` / `apply_*` helpers touch the DOM. Both the topbar
//! `ThemeToggle` popover and the Appearance settings page consume this module,
//! so the read/apply/persist logic lives in exactly one place.

use crate::i18n::{t_string, Locale};
use leptos_i18n::I18nContext;
use wasm_bindgen::JsCast;
use web_sys::{Document, HtmlElement, Storage};

// localStorage keys. `theme`/`accent` predate this module — kept verbatim for
// backward compatibility with already-persisted user preferences.
const KEY_MODE: &str = "aleph-theme";
const KEY_ACCENT: &str = "aleph-accent";
const KEY_FONT_SCALE: &str = "aleph-font-scale";
const KEY_ROUNDNESS: &str = "aleph-roundness";
const KEY_MATERIAL: &str = "aleph-material";
const KEY_DENSITY: &str = "aleph-density";

// ---------------------------------------------------------------------------
// Theme mode
// ---------------------------------------------------------------------------

/// Light/dark surface family. Drives the `<html>` class list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// Localised display name.
    ///
    /// Takes the context rather than returning a key: `t_string!` resolves a
    /// *static* path at compile time, so a runtime key would have to be
    /// re-matched by every caller — which is how the Chinese literals that
    /// used to live here reached both the phone and the desktop Appearance
    /// screens while the phone i18n census stayed green.
    #[must_use]
    pub fn label(self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::System => t_string!(i18n, appearance.mode.system).to_string(),
            Self::Light => t_string!(i18n, appearance.mode.light).to_string(),
            Self::Dark => t_string!(i18n, appearance.mode.dark).to_string(),
        }
    }

    /// `localStorage` value, or `None` for `System` (which clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("light") => Self::Light,
            // Legacy values: the retired Glass theme ("glass") and its
            // Vibrant-era predecessor ("translucent") were dark-based —
            // both load as Dark. `legacy_glass_migration` (run once on
            // boot) rewrites storage to dark + liquid material.
            Some("dark" | "glass" | "translucent") => Self::Dark,
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

    /// Localised display name.
    ///
    /// Takes the context rather than returning a key: `t_string!` resolves a
    /// *static* path at compile time, so a runtime key would have to be
    /// re-matched by every caller — which is how the Chinese literals that
    /// used to live here reached both the phone and the desktop Appearance
    /// screens while the phone i18n census stayed green.
    #[must_use]
    pub fn label(self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::Mauve => t_string!(i18n, appearance.accent.mauve).to_string(),
            Self::Ocean => t_string!(i18n, appearance.accent.ocean).to_string(),
            Self::Forest => t_string!(i18n, appearance.accent.forest).to_string(),
            Self::Sunset => t_string!(i18n, appearance.accent.sunset).to_string(),
            Self::Rose => t_string!(i18n, appearance.accent.rose).to_string(),
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
// Material (glass material family)
// ---------------------------------------------------------------------------

/// Glass material family. `Luxe` is the base look (clears `data-material`);
/// `Liquid` / `Aurora` re-skin every glass surface via the `--mat-*` primitive
/// blocks keyed off `<html data-material="…">`. Orthogonal to mode + accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    Luxe,
    Liquid,
    Aurora,
}

impl Material {
    pub const ALL: [Self; 3] = [Self::Luxe, Self::Liquid, Self::Aurora];

    /// Stable id used for both the `data-material` attribute and persistence.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Luxe => "luxe",
            Self::Liquid => "liquid",
            Self::Aurora => "aurora",
        }
    }

    /// Localised display name.
    ///
    /// Takes the context rather than returning a key: `t_string!` resolves a
    /// *static* path at compile time, so a runtime key would have to be
    /// re-matched by every caller — which is how the Chinese literals that
    /// used to live here reached both the phone and the desktop Appearance
    /// screens while the phone i18n census stayed green.
    #[must_use]
    pub fn label(self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::Luxe => t_string!(i18n, appearance.material.luxe).to_string(),
            Self::Liquid => t_string!(i18n, appearance.material.liquid).to_string(),
            Self::Aurora => t_string!(i18n, appearance.material.aurora).to_string(),
        }
    }

    /// Preview swatch background (CSS) for picker chips.
    #[must_use]
    pub const fn preview(self) -> &'static str {
        match self {
            Self::Luxe => "linear-gradient(145deg, oklch(0.95 0.010 300), oklch(0.84 0.030 310))",
            Self::Liquid => {
                "linear-gradient(145deg, oklch(0.82 0.100 310 / 0.9), oklch(0.66 0.130 250 / 0.75))"
            }
            Self::Aurora => {
                "linear-gradient(135deg, oklch(0.75 0.140 350 / 0.85), oklch(0.68 0.120 280 / 0.85), oklch(0.78 0.100 200 / 0.85))"
            }
        }
    }

    /// `localStorage` value, or `None` for `Luxe` (which clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Luxe => None,
            Self::Liquid => Some("liquid"),
            Self::Aurora => Some("aurora"),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            // "luxe" is never persisted (storage_value → None), but a stray
            // stored value must still resolve to the default explicitly.
            Some("luxe") => Self::Luxe,
            Some("liquid") => Self::Liquid,
            Some("aurora") => Self::Aurora,
            _ => Self::Luxe,
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
    Compact, // 90%
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

    /// Localised display name.
    ///
    /// Takes the context rather than returning a key: `t_string!` resolves a
    /// *static* path at compile time, so a runtime key would have to be
    /// re-matched by every caller — which is how the Chinese literals that
    /// used to live here reached both the phone and the desktop Appearance
    /// screens while the phone i18n census stayed green.
    #[must_use]
    pub fn label(self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::Compact => t_string!(i18n, appearance.font_scale.compact).to_string(),
            Self::Default => t_string!(i18n, appearance.font_scale.default).to_string(),
            Self::Cozy => t_string!(i18n, appearance.font_scale.cozy).to_string(),
            Self::Large => t_string!(i18n, appearance.font_scale.large).to_string(),
            Self::Largest => t_string!(i18n, appearance.font_scale.largest).to_string(),
        }
    }

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

    /// `localStorage` value, or `None` for the default (clears the key).
    #[must_use]
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
    pub const ALL: [Self; 5] = [
        Self::Sharp,
        Self::Slight,
        Self::Default,
        Self::Round,
        Self::Extra,
    ];

    /// Localised display name.
    ///
    /// Takes the context rather than returning a key: `t_string!` resolves a
    /// *static* path at compile time, so a runtime key would have to be
    /// re-matched by every caller — which is how the Chinese literals that
    /// used to live here reached both the phone and the desktop Appearance
    /// screens while the phone i18n census stayed green.
    #[must_use]
    pub fn label(self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::Sharp => t_string!(i18n, appearance.roundness.sharp).to_string(),
            Self::Slight => t_string!(i18n, appearance.roundness.slight).to_string(),
            Self::Default => t_string!(i18n, appearance.roundness.default).to_string(),
            Self::Round => t_string!(i18n, appearance.roundness.round).to_string(),
            Self::Extra => t_string!(i18n, appearance.roundness.extra).to_string(),
        }
    }

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

    /// `localStorage` value, or `None` for the default (clears the key).
    #[must_use]
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
// Density (whitespace compactness)
// ---------------------------------------------------------------------------

/// Whitespace compactness. Drives `--control-ui-density`, the multiplier that
/// Tailwind v4's `--spacing` base unit keys off of — so every numeric
/// padding/margin/gap/size utility re-scales from one value. `Compact` is the
/// cleared-key default: the baked baseline is already ~12% tighter than stock,
/// and the knob only adds breathing room from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,  // 1×    — the new compact baseline (default, clears the key)
    Cozy,     // 1.13× — restores the original ~0.25rem whitespace
    Spacious, // 1.25× — roomier
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Cozy, Self::Spacious];

    /// Localised display name.
    ///
    /// Takes the context rather than returning a key: `t_string!` resolves a
    /// *static* path at compile time, so a runtime key would have to be
    /// re-matched by every caller — which is how the Chinese literals that
    /// used to live here reached both the phone and the desktop Appearance
    /// screens while the phone i18n census stayed green.
    #[must_use]
    pub fn label(self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::Compact => t_string!(i18n, appearance.density.compact).to_string(),
            Self::Cozy => t_string!(i18n, appearance.density.cozy).to_string(),
            Self::Spacious => t_string!(i18n, appearance.density.spacious).to_string(),
        }
    }

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
    pub fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Compact => None,
            other => Some(other.css_value()),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("1.13") => Self::Cozy,
            Some("1.25") => Self::Spacious,
            _ => Self::Compact,
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

#[must_use]
pub fn read_mode() -> ThemeMode {
    ThemeMode::from_storage(read_key(KEY_MODE).as_deref())
}

#[must_use]
pub fn read_accent() -> Accent {
    Accent::from_storage(read_key(KEY_ACCENT).as_deref())
}

#[must_use]
pub fn read_font_scale() -> FontScale {
    FontScale::from_storage(read_key(KEY_FONT_SCALE).as_deref())
}

#[must_use]
pub fn read_roundness() -> Roundness {
    Roundness::from_storage(read_key(KEY_ROUNDNESS).as_deref())
}

#[must_use]
pub fn read_material() -> Material {
    Material::from_storage(read_key(KEY_MATERIAL).as_deref())
}

#[must_use]
pub fn read_density() -> Density {
    Density::from_storage(read_key(KEY_DENSITY).as_deref())
}

// ---------------------------------------------------------------------------
// Applies (mutate the DOM + persist)
// ---------------------------------------------------------------------------

pub fn apply_mode(mode: ThemeMode) {
    if let Some(html) = root() {
        let classes = html.class_list();
        let _ = classes.remove_4("dark", "light", "glass", "translucent");
        match mode {
            ThemeMode::Light => {
                let _ = classes.add_1("light");
            }
            ThemeMode::Dark => {
                let _ = classes.add_1("dark");
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

pub fn apply_density(density: Density) {
    if let Some(html) = root() {
        let _ = html
            .style()
            .set_property("--control-ui-density", density.css_value());
    }
    persist(KEY_DENSITY, density.storage_value());
}

pub fn apply_material(material: Material) {
    if let Some(html) = root() {
        if material == Material::Luxe {
            let _ = html.remove_attribute("data-material");
        } else {
            let _ = html.set_attribute("data-material", material.id());
        }
    }
    // Luxe is the base material → clear the key.
    persist(KEY_MATERIAL, material.storage_value());
}

/// Decide the legacy-Glass storage rewrite: a stored "glass"/"translucent"
/// mode becomes dark + liquid material. Pure (host-testable); returns the
/// `(aleph-theme, aleph-material)` values to write, or `None` when no
/// migration applies.
fn legacy_glass_migration(raw_mode: Option<&str>) -> Option<(&'static str, &'static str)> {
    matches!(raw_mode, Some("glass" | "translucent")).then_some(("dark", "liquid"))
}

/// Replay every persisted appearance axis onto the DOM. Called once on boot
/// (before the app mounts) so the first paint already reflects user choices.
pub fn init_appearance() {
    // One-shot legacy migration: Glass-theme users land on dark + liquid.
    if let Some((mode_v, material_v)) = legacy_glass_migration(read_key(KEY_MODE).as_deref()) {
        persist(KEY_MODE, Some(mode_v));
        // Safe to overwrite material unconditionally: the key didn't exist
        // before the Material axis was introduced.
        persist(KEY_MATERIAL, Some(material_v));
    }
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
    let density = read_density();
    if density != Density::Compact {
        apply_density(density);
    }
    let material = read_material();
    if material != Material::Luxe {
        apply_material(material);
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
    fn density_round_trips_via_css_value() {
        for d in Density::ALL {
            assert_eq!(Density::from_storage(Some(d.css_value())), d);
        }
        // Compact is the cleared-key default (the new compact baseline).
        assert_eq!(Density::Compact.storage_value(), None);
        assert_eq!(Density::from_storage(None), Density::Compact);
        assert_eq!(Density::from_storage(Some("garbage")), Density::Compact);
    }

    #[test]
    fn density_non_default_values_persist_a_key() {
        assert!(Density::Cozy.storage_value().is_some());
        assert!(Density::Spacious.storage_value().is_some());
    }

    #[test]
    fn non_default_values_persist_a_key() {
        // Every non-default variant must produce a storable value so the
        // choice survives a reload (the boot replay relies on this).
        assert!(FontScale::Largest.storage_value().is_some());
        assert!(Roundness::Sharp.storage_value().is_some());
        assert!(ThemeMode::Dark.storage_value().is_some());
        assert!(Material::Liquid.storage_value().is_some());
        assert!(Material::Aurora.storage_value().is_some());
    }

    #[test]
    fn legacy_glass_values_load_as_dark() {
        // The retired Glass theme (and its Vibrant-era "translucent"
        // predecessor) must keep PARSING — they map to Dark; the material
        // half of the migration is decided by `legacy_glass_migration`.
        assert_eq!(ThemeMode::from_storage(Some("glass")), ThemeMode::Dark);
        assert_eq!(
            ThemeMode::from_storage(Some("translucent")),
            ThemeMode::Dark
        );
    }

    #[test]
    fn legacy_glass_migration_targets_liquid_dark() {
        assert_eq!(
            legacy_glass_migration(Some("glass")),
            Some(("dark", "liquid"))
        );
        assert_eq!(
            legacy_glass_migration(Some("translucent")),
            Some(("dark", "liquid"))
        );
        assert_eq!(legacy_glass_migration(Some("dark")), None);
        assert_eq!(legacy_glass_migration(None), None);
    }

    #[test]
    fn material_id_round_trips() {
        for m in Material::ALL {
            assert_eq!(Material::from_storage(Some(m.id())), m);
        }
        // Unknown / luxe both fall back to the default material.
        assert_eq!(Material::from_storage(None), Material::Luxe);
        assert_eq!(Material::from_storage(Some("nope")), Material::Luxe);
    }

    #[test]
    fn material_default_clears_key() {
        assert_eq!(Material::Luxe.storage_value(), None);
        assert_eq!(Material::Liquid.storage_value(), Some("liquid"));
        assert_eq!(Material::Aurora.storage_value(), Some("aurora"));
    }

    /// Body lines of the CSS rule whose selector line, after trimming,
    /// equals `selector` exactly. Exact-line matching keeps `.dark {` from
    /// matching `html[data-material="liquid"].dark {` or the kill block's
    /// `.dark, .dark[data-material] {`. Lines come back trimmed (the system
    /// mirrors sit one nesting level deeper than the `.dark` blocks they
    /// copy) with empties dropped; brace depth is tracked so a nested rule
    /// added later won't truncate the body. Callers pass one
    /// banner-delimited SECTION of the stylesheet (token layer = before the
    /// banner, material primitives = after) — the exactly-once assertion
    /// holds within each slice.
    fn css_block_body<'a>(css: &'a str, selector: &str) -> Vec<&'a str> {
        let lines: Vec<&str> = css.lines().collect();
        let starts: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.trim() == selector)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            starts.len(),
            1,
            "selector line {selector:?} must appear exactly once in the \
             given section of tailwind.css"
        );
        let mut depth: i64 = 1;
        let mut body = Vec::new();
        for line in &lines[starts[0] + 1..] {
            let opens = line.matches('{').count() as i64;
            let closes = line.matches('}').count() as i64;
            if depth + opens - closes <= 0 {
                return body;
            }
            depth += opens - closes;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                body.push(trimmed);
            }
        }
        panic!("unclosed block for selector {selector:?} in tailwind.css");
    }

    /// Assert every `(dark, mirror)` selector pair in `section` has a verbatim-
    /// identical body (trimmed lines, comments included — mirrors are copies).
    fn assert_mirror_pairs(section: &str, pairs: &[(&str, &str)]) {
        for (dark, mirror) in pairs {
            let dark_body = css_block_body(section, dark);
            let mirror_body = css_block_body(section, mirror);
            for (i, (d, m)) in dark_body.iter().zip(mirror_body.iter()).enumerate() {
                assert_eq!(
                    d,
                    m,
                    "system-mode mirror {mirror:?} drifted from {dark:?} at body \
                     line {} — copy the `.dark` block verbatim",
                    i + 1
                );
            }
            assert_eq!(
                dark_body.len(),
                mirror_body.len(),
                "system-mode mirror {mirror:?} and {dark:?} have different line \
                 counts — copy the `.dark` block verbatim"
            );
        }
    }

    #[test]
    fn mirror_blocks_are_verbatim_copies() {
        // The stylesheet keeps a hand-synced `@media (prefers-color-scheme:
        // dark)` mirror of every `.dark` primitive block (the media query
        // can't reference the class-based selector). Drift would be silent
        // and only visible to System-mode users — enforce the copy-verbatim
        // discipline here. Comment lines participate on purpose: mirrors
        // are verbatim copies, comments included.
        let css = include_str!("../styles/tailwind.css");
        let (_, material_section) = css
            .split_once("Material primitives")
            .expect("material primitives banner present in tailwind.css");
        let pairs = [
            (".dark {", ":root:not(.light) {"),
            (
                r#"html[data-material="liquid"].dark {"#,
                r#":root:not(.light)[data-material="liquid"] {"#,
            ),
            (
                r#"html[data-material="aurora"].dark {"#,
                r#":root:not(.light)[data-material="aurora"] {"#,
            ),
        ];
        assert_mirror_pairs(material_section, &pairs);
    }

    #[test]
    fn token_mirror_blocks_are_verbatim_copies() {
        // Same copy-verbatim discipline as the material primitives, applied to
        // the colour-token layer ABOVE the banner: the `.dark` token block and
        // the four dark accent overrides each keep a hand-synced
        // `@media (prefers-color-scheme: dark)` mirror for System-mode users.
        let css = include_str!("../styles/tailwind.css");
        let (token_section, _) = css
            .split_once("Material primitives")
            .expect("material primitives banner present in tailwind.css");
        assert_mirror_pairs(
            token_section,
            &[
                (".dark {", ":root:not(.light) {"),
                (
                    r#"html.dark[data-accent="ocean"] {"#,
                    r#":root:not(.light)[data-accent="ocean"] {"#,
                ),
                (
                    r#"html.dark[data-accent="forest"] {"#,
                    r#":root:not(.light)[data-accent="forest"] {"#,
                ),
                (
                    r#"html.dark[data-accent="sunset"] {"#,
                    r#":root:not(.light)[data-accent="sunset"] {"#,
                ),
                (
                    r#"html.dark[data-accent="rose"] {"#,
                    r#":root:not(.light)[data-accent="rose"] {"#,
                ),
            ],
        );
    }
    /// Every selector that turns a backdrop filter ON must be turned back OFF
    /// by a `html[data-flat="1"]` rule.
    ///
    /// Flat mode's whole content is "the expensive materials are gone", and on
    /// Linux it is applied unconditionally with no opt-out — so a surface that
    /// keeps its blur there is that goal not met, on precisely the machines
    /// the degradation exists to protect.
    ///
    /// The required set is DERIVED from both places a blur can be declared,
    /// never hand-listed. `.aleph-todo-wrap` was missing from the flat block
    /// for as long as it existed because it sets its blur from a Rust `const`
    /// in `todo_panel.rs`: nobody reading the stylesheet could see that the
    /// list was short. A hand-written expectation here would be that same
    /// defect one level up.
    ///
    /// A universal `html[data-flat="1"] *` rule would also be rot-proof and is
    /// deliberately not used: `!important` on every element costs style recalc
    /// on exactly the weak machines flat mode targets. The cheap list stays;
    /// the list gets a rule.
    #[test]
    fn no_backdrop_filter_survives_flat_mode() {
        const CSS: &str = include_str!("../styles/tailwind.css");
        const TODO_CSS: &str = include_str!("platform/wide/views/chat/todo_panel.rs");

        // Selectors a `html[data-flat="1"]` rule nulls the backdrop filter for.
        //
        // Walk RULES, not occurrences of the prefix. A rule commonly lists
        // several selectors, each repeating the prefix — splitting on the
        // prefix keeps only the last of each group, which reads as a much
        // shorter flat block than the file actually has.
        let mut nulled: Vec<String> = Vec::new();
        let css_lines: Vec<&str> = CSS.lines().collect();
        for (i, line) in css_lines.iter().enumerate() {
            if !line.trim().starts_with("backdrop-filter: none") {
                continue;
            }
            // Walk back to the selector line that opened this rule. A rule
            // often lists several selectors, each repeating the prefix, so
            // take all of them — keeping only the last would read as a much
            // shorter flat block than the file actually has.
            let head = css_lines[..i]
                .iter()
                .rev()
                .find(|l| l.contains('{'))
                .copied()
                .unwrap_or_default();
            for sel in head.split('{').next().unwrap_or_default().split(',') {
                let Some(sel) = sel.trim().strip_prefix("html[data-flat=\"1\"]") else {
                    continue;
                };
                let sel = sel.trim();
                if !sel.is_empty() {
                    nulled.push(sel.to_string());
                }
            }
        }
        assert!(
            nulled.len() >= 5,
            "the flat block should null several selectors; found {nulled:?} — \
             if this shrank to nothing the scanner stopped matching, which \
             would make this guard silently vacuous"
        );

        // Selectors that SET a backdrop filter, from both declaration sites.
        let mut setters: Vec<(String, &str)> = Vec::new();
        for (source, label) in [(CSS, "tailwind.css"), (TODO_CSS, "todo_panel.rs")] {
            let lines: Vec<&str> = source.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let decl = line.trim();
                if !decl.starts_with("backdrop-filter:") && !decl.contains(";backdrop-filter:") {
                    continue;
                }
                if decl.contains("backdrop-filter: none") || decl.contains("backdrop-filter:none") {
                    continue;
                }
                // Walk back to the nearest selector line opening this block.
                let head = lines[..i]
                    .iter()
                    .rev()
                    .find(|l| l.contains('{'))
                    .copied()
                    .unwrap_or_default();
                for sel in head.split('{').next().unwrap_or_default().split(',') {
                    let sel = sel
                        .trim()
                        .trim_start_matches("html[data-flat=\"1\"]")
                        .trim();
                    if sel.starts_with('.') {
                        setters.push((sel.to_string(), label));
                    }
                }
            }
        }
        assert!(
            !setters.is_empty(),
            "found no backdrop-filter declarations at all — the scanner is broken"
        );

        let missing: Vec<String> = setters
            .iter()
            .filter(|(sel, _)| {
                !nulled
                    .iter()
                    .any(|n| n == sel || n.starts_with(&format!("{sel}:")))
            })
            .map(|(sel, src)| format!("{sel} (set in {src})"))
            .collect();
        assert!(
            missing.is_empty(),
            "these selectors set a backdrop filter that flat mode never turns off: {missing:?}. \
             Flat mode is unconditional on Linux, so each one keeps its blur on the machines \
             the degradation exists for. Add a `html[data-flat=\"1\"] <selector>` rule nulling \
             both the prefixed and unprefixed property."
        );
    }
}
