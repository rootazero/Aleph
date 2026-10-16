//! Theme loading and semantic terminal colours.
//!
//! Built-in and community themes use the same versioned JSON loader. Community
//! files may extend one built-in theme and override semantic roles and chrome;
//! they contain no executable code and are bounded to 128 KiB.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ratatui::style::Color;
use serde::Deserialize;
use shared_ui_logic::transcript::{SemanticColor, ALL_SEMANTIC_COLORS};

pub use shared_ui_logic::transcript::SPINNER_FRAMES;

const MAX_THEME_BYTES: u64 = 128 * 1024;
const BUILTIN_NAMES: &[&str] = &["amber", "tide", "jade", "iris", "paper"];
const LEGACY_NAMES: &[&str] = &["dark", "light", "terminal"];

#[must_use]
pub fn spinner_at(tick: usize) -> char {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

/// The named built-in palettes and compatibility aliases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preset {
    #[default]
    Amber,
    Tide,
    Jade,
    Iris,
    Paper,
    /// Compatibility alias for Amber.
    Dark,
    /// Compatibility alias for Paper.
    Light,
    Terminal,
    /// A validated community theme.
    Custom,
}

impl Preset {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "amber" => Some(Self::Amber),
            "tide" => Some(Self::Tide),
            "jade" => Some(Self::Jade),
            "iris" => Some(Self::Iris),
            "paper" => Some(Self::Paper),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Amber => "amber",
            Self::Tide => "tide",
            Self::Jade => "jade",
            Self::Iris => "iris",
            Self::Paper => "paper",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Terminal => "terminal",
            Self::Custom => "custom",
        }
    }

    pub const ALL: &'static [Self] = &[
        Self::Amber,
        Self::Tide,
        Self::Jade,
        Self::Iris,
        Self::Paper,
        Self::Dark,
        Self::Light,
        Self::Terminal,
    ];

    #[must_use]
    pub fn choices() -> String {
        BUILTIN_NAMES.join(" | ")
    }

    #[must_use]
    fn builtin_name(self) -> Option<&'static str> {
        match self {
            Self::Amber | Self::Dark => Some("amber"),
            Self::Tide => Some("tide"),
            Self::Jade => Some("jade"),
            Self::Iris => Some("iris"),
            Self::Paper | Self::Light => Some("paper"),
            Self::Terminal | Self::Custom => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ColorSpec {
    Rgb(u8, u8, u8),
    Indexed(u8),
    Terminal,
}

impl ColorSpec {
    fn parse_json(value: JsonColor) -> Result<Self, String> {
        match value {
            JsonColor::Number(index) => Ok(Self::Indexed(index)),
            JsonColor::String(value) if value == "terminal" => Ok(Self::Terminal),
            JsonColor::String(value) => {
                let bytes = value.as_bytes();
                if bytes.len() != 7 || bytes[0] != b'#' {
                    return Err(format!(
                        "invalid color {value:?}; use #rrggbb, terminal, or 0..255"
                    ));
                }
                let parse = |part: &[u8]| {
                    std::str::from_utf8(part)
                        .ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                };
                match (
                    parse(&bytes[1..3]),
                    parse(&bytes[3..5]),
                    parse(&bytes[5..7]),
                ) {
                    (Some(r), Some(g), Some(b)) => Ok(Self::Rgb(r, g, b)),
                    _ => Err(format!(
                        "invalid color {value:?}; use #rrggbb, terminal, or 0..255"
                    )),
                }
            }
        }
    }

    fn rgb(&self) -> Option<(u8, u8, u8)> {
        match self {
            Self::Rgb(r, g, b) => Some((*r, *g, *b)),
            Self::Indexed(_) | Self::Terminal => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum JsonColor {
    String(String),
    Number(u8),
}

// Missing optional fields inherit; an explicitly present null is a malformed value.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ColorOverrides {
    #[serde(default, deserialize_with = "present")]
    fg: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    dim: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    accent: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    prompt: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    user_bar: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    tool_pending: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    tool_running: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    tool_ok: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    tool_err: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    tool_rail: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    diff_add: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    diff_del: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    diff_ctx: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    diff_gutter: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    admonition_note: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    admonition_tip: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    admonition_important: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    admonition_warning: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    admonition_caution: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    code_border: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    link: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    context_ok: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    context_warn: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    context_danger: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    cost: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    spinner: Option<JsonColor>,
}

impl ColorOverrides {
    fn get(&self, role: SemanticColor) -> Option<&JsonColor> {
        match role {
            SemanticColor::Fg => self.fg.as_ref(),
            SemanticColor::Dim => self.dim.as_ref(),
            SemanticColor::Accent => self.accent.as_ref(),
            SemanticColor::Prompt => self.prompt.as_ref(),
            SemanticColor::UserBar => self.user_bar.as_ref(),
            SemanticColor::ToolPending => self.tool_pending.as_ref(),
            SemanticColor::ToolRunning => self.tool_running.as_ref(),
            SemanticColor::ToolOk => self.tool_ok.as_ref(),
            SemanticColor::ToolErr => self.tool_err.as_ref(),
            SemanticColor::ToolRail => self.tool_rail.as_ref(),
            SemanticColor::DiffAdd => self.diff_add.as_ref(),
            SemanticColor::DiffDel => self.diff_del.as_ref(),
            SemanticColor::DiffCtx => self.diff_ctx.as_ref(),
            SemanticColor::DiffGutter => self.diff_gutter.as_ref(),
            SemanticColor::AdmonitionNote => self.admonition_note.as_ref(),
            SemanticColor::AdmonitionTip => self.admonition_tip.as_ref(),
            SemanticColor::AdmonitionImportant => self.admonition_important.as_ref(),
            SemanticColor::AdmonitionWarning => self.admonition_warning.as_ref(),
            SemanticColor::AdmonitionCaution => self.admonition_caution.as_ref(),
            SemanticColor::CodeBorder => self.code_border.as_ref(),
            SemanticColor::Link => self.link.as_ref(),
            SemanticColor::ContextOk => self.context_ok.as_ref(),
            SemanticColor::ContextWarn => self.context_warn.as_ref(),
            SemanticColor::ContextDanger => self.context_danger.as_ref(),
            SemanticColor::Cost => self.cost.as_ref(),
            SemanticColor::Spinner => self.spinner.as_ref(),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ChromeOverrides {
    #[serde(default, deserialize_with = "present")]
    background: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    surface: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    selection_bg: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    user_bg: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    border: Option<JsonColor>,
    #[serde(default, deserialize_with = "present")]
    border_focused: Option<JsonColor>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeDocument {
    #[serde(rename = "schemaVersion")]
    schema_version: u8,
    name: String,
    #[serde(default, deserialize_with = "present")]
    light: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    extends: Option<String>,
    #[serde(default)]
    colors: ColorOverrides,
    #[serde(default)]
    chrome: ChromeOverrides,
}

#[derive(Debug, Clone)]
struct ChromeData {
    background: ColorSpec,
    surface: ColorSpec,
    selection_bg: ColorSpec,
    user_bg: ColorSpec,
    border: ColorSpec,
    border_focused: ColorSpec,
}

#[derive(Debug, Clone)]
struct PaletteData {
    name: String,
    colors: Vec<ColorSpec>,
    chrome: ChromeData,
    light: bool,
}

impl PaletteData {
    fn role(&self, role: SemanticColor) -> &ColorSpec {
        &self.colors[role_index(role)]
    }
}

fn role_index(role: SemanticColor) -> usize {
    ALL_SEMANTIC_COLORS
        .iter()
        .position(|candidate| *candidate == role)
        .expect("semantic roster must contain every role")
}

fn parse_color(value: Option<JsonColor>, field: &str) -> Result<Option<ColorSpec>, String> {
    value
        .map(|value| ColorSpec::parse_json(value).map_err(|error| format!("{field}: {error}")))
        .transpose()
}

fn parse_document(text: &str, requested_name: &str) -> Result<ThemeDocument, String> {
    let document: ThemeDocument =
        serde_json::from_str(text).map_err(|error| format!("invalid theme JSON: {error}"))?;
    if document.schema_version != 1 {
        return Err(format!(
            "unsupported schemaVersion {}; expected 1",
            document.schema_version
        ));
    }
    if document.name != requested_name {
        return Err(format!(
            "theme name {:?} does not match requested name {:?}",
            document.name, requested_name
        ));
    }
    Ok(document)
}

fn resolve_document(
    document: ThemeDocument,
    base: Option<&PaletteData>,
) -> Result<PaletteData, String> {
    let mut colors = base.map(|data| data.colors.clone()).unwrap_or_default();
    if colors.is_empty() {
        colors = vec![ColorSpec::Terminal; ALL_SEMANTIC_COLORS.len()];
    }
    for role in ALL_SEMANTIC_COLORS {
        if let Some(value) = document.colors.get(*role) {
            colors[role_index(*role)] = ColorSpec::parse_json(value.clone())?;
        } else if base.is_none() {
            return Err(format!("missing semantic color {role:?}"));
        }
    }

    let base_chrome = base
        .map(|data| data.chrome.clone())
        .unwrap_or_else(|| ChromeData {
            background: ColorSpec::Terminal,
            surface: ColorSpec::Terminal,
            selection_bg: ColorSpec::Terminal,
            user_bg: ColorSpec::Terminal,
            border: ColorSpec::Terminal,
            border_focused: ColorSpec::Terminal,
        });
    let mut chrome = base_chrome;
    {
        let overrides = document.chrome;
        chrome.background =
            parse_color(overrides.background, "chrome.background")?.unwrap_or(chrome.background);
        chrome.surface =
            parse_color(overrides.surface, "chrome.surface")?.unwrap_or(chrome.surface);
        chrome.selection_bg = parse_color(overrides.selection_bg, "chrome.selectionBg")?
            .unwrap_or(chrome.selection_bg);
        chrome.user_bg = parse_color(overrides.user_bg, "chrome.userBg")?.unwrap_or(chrome.user_bg);
        chrome.border = parse_color(overrides.border, "chrome.border")?.unwrap_or(chrome.border);
        chrome.border_focused = parse_color(overrides.border_focused, "chrome.borderFocused")?
            .unwrap_or(chrome.border_focused);
    }
    let light = document.light.unwrap_or_else(|| {
        chrome
            .background
            .rgb()
            .map(|(r, g, b)| {
                u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 >= 150_000
            })
            .unwrap_or_else(|| base.is_some_and(|data| data.light))
    });
    Ok(PaletteData {
        name: document.name,
        colors,
        chrome,
        light,
    })
}

fn builtin_text(name: &str) -> Option<&'static str> {
    match name {
        "amber" => Some(include_str!("../../themes/amber.json")),
        "tide" => Some(include_str!("../../themes/tide.json")),
        "jade" => Some(include_str!("../../themes/jade.json")),
        "iris" => Some(include_str!("../../themes/iris.json")),
        "paper" => Some(include_str!("../../themes/paper.json")),
        _ => None,
    }
}

fn builtin_data(name: &str) -> Result<PaletteData, String> {
    let text = builtin_text(name).ok_or_else(|| format!("unknown built-in theme {name:?}"))?;
    let document = parse_document(text, name)?;
    resolve_document(document, None)
}

fn validate_name(name: &str) -> Result<(), String> {
    if matches!(name, "list" | "reload") {
        return Err(format!("reserved theme command name {name:?}"));
    }
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 || !bytes[0].is_ascii_lowercase() {
        return Err("theme names must match [a-z][a-z0-9-]{0,63}".to_owned());
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err("theme names must match [a-z][a-z0-9-]{0,63}".to_owned());
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<String, String> {
    let size = fs::metadata(path)
        .map_err(|error| format!("cannot read theme file: {error}"))?
        .len();
    if size > MAX_THEME_BYTES {
        return Err("theme file exceeds 128 KiB".to_owned());
    }
    let file = fs::File::open(path).map_err(|error| format!("cannot read theme file: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("theme path must be a regular file".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(MAX_THEME_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read theme file: {error}"))?;
    if bytes.len() as u64 > MAX_THEME_BYTES {
        return Err("theme file exceeds 128 KiB".to_owned());
    }
    String::from_utf8(bytes).map_err(|error| format!("theme must be UTF-8: {error}"))
}

/// Load a named built-in or community theme from an Aleph home directory.
/// Built-ins cannot be shadowed by a community file.
pub fn load_named(root: &Path, name: &str, truecolor: bool) -> Result<Palette, String> {
    validate_name(name)?;
    if let Some(preset) = Preset::parse(name) {
        if preset == Preset::Terminal {
            return Ok(Palette::for_preset(Preset::Terminal, truecolor));
        }
        if let Some(builtin) = preset.builtin_name() {
            let data = builtin_data(builtin)?;
            return Ok(Palette::from_data(data, preset, truecolor));
        }
    }
    if LEGACY_NAMES.contains(&name) || BUILTIN_NAMES.contains(&name) {
        return Err(format!("reserved built-in theme name {name:?}"));
    }
    let path = root.join("themes").join(format!("{name}.json"));
    let text = read_bounded(&path)?;
    let document = parse_document(&text, name)?;
    let extends = document
        .extends
        .clone()
        .unwrap_or_else(|| "amber".to_owned());
    validate_name(&extends)?;
    let base_name = Preset::parse(&extends)
        .and_then(Preset::builtin_name)
        .ok_or_else(|| format!("theme {name:?} may extend only a built-in theme"))?;
    let base = builtin_data(base_name)?;
    let data = resolve_document(document, Some(&base))?;
    Ok(Palette::from_data(data, Preset::Custom, truecolor))
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub preset: Preset,
    pub truecolor: bool,
    data: Arc<PaletteData>,
}

impl Palette {
    #[must_use]
    pub fn for_preset(preset: Preset, truecolor: bool) -> Self {
        if preset == Preset::Terminal {
            return Self {
                preset,
                truecolor,
                data: Arc::new(terminal_data()),
            };
        }
        let name = preset.builtin_name().unwrap_or("amber");
        let data = builtin_data(name).expect("checked-in built-in theme must validate");
        Self::from_data(data, preset, truecolor)
    }

    #[must_use]
    pub fn detect(preset: Preset) -> Self {
        Self::for_preset(preset, supports_truecolor())
    }

    fn from_data(data: PaletteData, preset: Preset, truecolor: bool) -> Self {
        Self {
            preset,
            truecolor,
            data: Arc::new(data),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.data.name
    }

    #[must_use]
    pub fn is_rgb(&self) -> bool {
        self.truecolor && self.preset != Preset::Terminal
    }

    #[must_use]
    pub fn is_light(&self) -> bool {
        self.data.light
    }

    #[must_use]
    pub fn rgb(&self, role: SemanticColor) -> Option<(u8, u8, u8)> {
        self.is_rgb().then(|| self.data.role(role).rgb()).flatten()
    }

    #[must_use]
    pub fn color(&self, role: SemanticColor) -> Color {
        if self.preset == Preset::Terminal {
            return Color::Reset;
        }
        paint(self.data.role(role), self.is_rgb())
    }

    fn chrome(&self, which: ChromeField) -> Color {
        if self.preset == Preset::Terminal {
            return Color::Reset;
        }
        let spec = match which {
            ChromeField::Background => &self.data.chrome.background,
            ChromeField::Surface => &self.data.chrome.surface,
            ChromeField::Selection => &self.data.chrome.selection_bg,
            ChromeField::User => &self.data.chrome.user_bg,
            ChromeField::Border => &self.data.chrome.border,
            ChromeField::BorderFocused => &self.data.chrome.border_focused,
        };
        paint(spec, self.is_rgb())
    }
}

fn paint(spec: &ColorSpec, rgb: bool) -> Color {
    match spec {
        ColorSpec::Rgb(r, g, b) if rgb => Color::Rgb(*r, *g, *b),
        ColorSpec::Rgb(r, g, b) => Color::Indexed(rgb_to_index(*r, *g, *b)),
        ColorSpec::Indexed(index) => Color::Indexed(*index),
        ColorSpec::Terminal => Color::Reset,
    }
}

fn rgb_to_index(r: u8, g: u8, b: u8) -> u8 {
    let r = u16::from(r) * 5 / 255;
    let g = u16::from(g) * 5 / 255;
    let b = u16::from(b) * 5 / 255;
    (16 + 36 * r + 6 * g + b) as u8
}

fn terminal_data() -> PaletteData {
    PaletteData {
        name: "terminal".to_owned(),
        colors: vec![ColorSpec::Terminal; ALL_SEMANTIC_COLORS.len()],
        chrome: ChromeData {
            background: ColorSpec::Terminal,
            surface: ColorSpec::Terminal,
            selection_bg: ColorSpec::Terminal,
            user_bg: ColorSpec::Terminal,
            border: ColorSpec::Terminal,
            border_focused: ColorSpec::Terminal,
        },
        light: false,
    }
}

/// Terminal chrome and semantic fields flattened for widget consumers.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub user: Color,
    pub assistant: Color,
    pub system: Color,
    pub tool_running: Color,
    pub tool_success: Color,
    pub tool_failed: Color,
    pub tool_name: Color,
    pub heading: Color,
    pub primary: Color,
    pub muted: Color,
    pub reasoning: Color,
    pub error: Color,
    pub warning: Color,
    pub background: Color,
    pub surface: Color,
    pub selection_bg: Color,
    pub user_bg: Color,
    pub border: Color,
    pub border_focused: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub connected: Color,
    pub disconnected: Color,
}

#[derive(Clone, Copy)]
enum ChromeField {
    Background,
    Surface,
    Selection,
    User,
    Border,
    BorderFocused,
}

impl Theme {
    #[must_use]
    pub fn from_palette(palette: Palette) -> Self {
        let c = |role| palette.color(role);
        Self {
            user: c(SemanticColor::UserBar),
            assistant: c(SemanticColor::Fg),
            system: c(SemanticColor::AdmonitionNote),
            tool_running: c(SemanticColor::ToolRunning),
            tool_success: c(SemanticColor::ToolOk),
            tool_failed: c(SemanticColor::ToolErr),
            tool_name: c(SemanticColor::Accent),
            heading: c(SemanticColor::Accent),
            primary: c(SemanticColor::Fg),
            muted: c(SemanticColor::Dim),
            reasoning: c(SemanticColor::Dim),
            error: c(SemanticColor::ToolErr),
            warning: c(SemanticColor::AdmonitionWarning),
            background: palette.chrome(ChromeField::Background),
            surface: palette.chrome(ChromeField::Surface),
            selection_bg: palette.chrome(ChromeField::Selection),
            user_bg: palette.chrome(ChromeField::User),
            border: palette.chrome(ChromeField::Border),
            border_focused: palette.chrome(ChromeField::BorderFocused),
            status_bg: palette.chrome(ChromeField::Background),
            status_fg: c(SemanticColor::Dim),
            connected: c(SemanticColor::ToolOk),
            disconnected: c(SemanticColor::ToolErr),
        }
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Pure capability selection for tests and callers with explicit environment values.
#[must_use]
pub fn detect_truecolor(
    color_override: Option<&str>,
    colorterm: Option<&str>,
    wt_session: Option<&str>,
    term: Option<&str>,
) -> bool {
    match color_override.map(str::to_ascii_lowercase).as_deref() {
        Some("truecolor") => return true,
        Some("ansi" | "256") => return false,
        _ => {}
    }
    if colorterm.is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.contains("truecolor") || value.contains("24bit")
    }) || wt_session.is_some_and(|value| !value.is_empty())
    {
        return true;
    }
    term.is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.ends_with("-direct")
            || matches!(
                value.as_str(),
                "xterm-kitty" | "alacritty" | "wezterm" | "foot"
            )
    })
}

#[must_use]
pub fn supports_truecolor() -> bool {
    detect_truecolor(
        env_value("ALEPH_TUI_COLOR").as_deref(),
        env_value("COLORTERM").as_deref(),
        env_value("WT_SESSION").as_deref(),
        env_value("TERM").as_deref(),
    )
}

#[cfg(not(test))]
fn aleph_home() -> Option<PathBuf> {
    aleph_protocol::paths::aleph_home()
}

#[must_use]
pub fn theme_file() -> Option<PathBuf> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        aleph_home().map(|home| home.join("tui-theme"))
    }
}

fn theme_root() -> Option<PathBuf> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        aleph_home()
    }
}

#[derive(Debug)]
struct BootState {
    palette: Palette,
    warnings: Vec<String>,
}

fn boot_at(root: &Path, env: Option<&str>, stored: Option<&str>, truecolor: bool) -> BootState {
    let mut warnings = Vec::new();
    for (source, candidate) in [("ALEPH_TUI_THEME", env), ("stored preference", stored)] {
        let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) else {
            continue;
        };
        match load_named(root, candidate, truecolor) {
            Ok(palette) => return BootState { palette, warnings },
            Err(error) => warnings.push(format!("{source} {candidate:?} ignored: {error}")),
        }
    }
    let palette = load_named(root, "amber", truecolor).expect("amber is a checked-in built-in");
    BootState { palette, warnings }
}

#[derive(Debug)]
struct SessionTheme {
    palette: Palette,
    warnings: Vec<String>,
}

impl SessionTheme {
    fn new(palette: Palette) -> Self {
        Self {
            palette,
            warnings: Vec::new(),
        }
    }

    fn select(&mut self, root: &Path, name: &str, truecolor: bool) -> Result<(), String> {
        let loaded = load_named(root, name, truecolor)?;
        self.palette = loaded;
        Ok(())
    }
}

static CURRENT: OnceLock<Mutex<SessionTheme>> = OnceLock::new();

fn current() -> &'static Mutex<SessionTheme> {
    CURRENT.get_or_init(|| {
        #[cfg(test)]
        let state = SessionTheme::new(Palette::for_preset(Preset::Amber, false));
        #[cfg(not(test))]
        let state = {
            let root = theme_root().unwrap_or_default();
            let mut read_warnings = Vec::new();
            let stored = theme_file().and_then(|path| match read_bounded(&path) {
                Ok(text) => Some(text),
                Err(_) if !path.exists() => None,
                Err(error) => {
                    read_warnings.push(format!("stored preference ignored: {error}"));
                    None
                }
            });
            let boot = boot_at(
                &root,
                env_value("ALEPH_TUI_THEME").as_deref(),
                stored.as_deref(),
                supports_truecolor(),
            );
            let mut state = SessionTheme::new(boot.palette);
            state.warnings = read_warnings;
            state.warnings.extend(boot.warnings);
            state
        };
        Mutex::new(state)
    })
}

/// Initialize the lazy process-wide TUI theme.
pub fn initialize() {
    let _ = current();
}

#[cfg(test)]
thread_local! {
    static TEST_PALETTE: std::cell::RefCell<Option<Palette>> = const { std::cell::RefCell::new(None) };
}

/// Scope renderer tests to a thread-local palette, restoring it even on panic.
#[cfg(test)]
pub(crate) fn with_test_palette<T>(palette: Palette, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<Palette>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_PALETTE.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(TEST_PALETTE.with(|slot| slot.replace(Some(palette))));
    run()
}

#[must_use]
pub fn palette() -> Palette {
    #[cfg(test)]
    if let Some(palette) = TEST_PALETTE.with(|slot| slot.borrow().clone()) {
        return palette;
    }
    current()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .palette
        .clone()
}

#[must_use]
pub fn theme() -> Theme {
    Theme::from_palette(palette())
}

#[must_use]
pub fn current_preset() -> Preset {
    palette().preset
}

#[must_use]
pub fn current_theme_name() -> String {
    palette().name().to_owned()
}

/// Validate and install a theme atomically. The active theme is untouched on error.
pub fn select_theme(name: &str) -> Result<(), String> {
    let root = theme_root().unwrap_or_else(|| PathBuf::from("."));
    current()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .select(&root, name, supports_truecolor())
}

fn remember_theme_at(root: &Path, name: &str, truecolor: bool) -> io::Result<PathBuf> {
    load_named(root, name, truecolor)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let path = root.join("tui-theme");
    fs::create_dir_all(root)
        .and_then(|_| fs::write(&path, name))
        .map(|_| path)
}

pub fn remember_theme(name: &str) -> io::Result<PathBuf> {
    let root = theme_root()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Aleph home directory"))?;
    remember_theme_at(&root, name, supports_truecolor())
}

/// Return valid built-in and community names, plus diagnostics for skipped files.
#[must_use]
pub fn available_themes() -> (Vec<String>, Vec<String>) {
    let root = theme_root().unwrap_or_else(|| PathBuf::from("."));
    available_themes_at(&root, supports_truecolor())
}

fn available_themes_at(root: &Path, truecolor: bool) -> (Vec<String>, Vec<String>) {
    let mut names = BUILTIN_NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    let directory = root.join("themes");
    let Ok(entries) = fs::read_dir(&directory) else {
        return (names, diagnostics);
    };
    let mut community = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some("json"))
                .then(|| {
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .map(str::to_owned)
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    community.sort();
    for name in community {
        if BUILTIN_NAMES.contains(&name.as_str()) || LEGACY_NAMES.contains(&name.as_str()) {
            diagnostics.push(format!(
                "{name:?} skipped: reserved built-in name cannot be overridden"
            ));
            continue;
        }
        match load_named(root, &name, truecolor) {
            Ok(_) => names.push(name),
            Err(error) => diagnostics.push(format!("{name:?} skipped: {error}")),
        }
    }
    (names, diagnostics)
}

#[must_use]
pub fn take_boot_warnings() -> Vec<String> {
    std::mem::take(
        &mut current()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .warnings,
    )
}

// Compatibility with the old preset-only caller surface.
pub fn set_preset(preset: Preset) {
    current()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .palette = Palette::detect(preset);
}

pub fn remember_preset(preset: Preset) -> io::Result<PathBuf> {
    remember_theme(preset.name())
}

#[cfg(test)]
#[path = "theme/tests.rs"]
mod tests;
