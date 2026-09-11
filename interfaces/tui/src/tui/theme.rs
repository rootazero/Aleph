//! Colour for the terminal surface.
//!
//! # Two tables, and why the split is not arbitrary
//!
//! * **Transcript semantics** come from
//!   [`shared_ui_logic::transcript::SemanticColor`] — the one roster the Panel
//!   paints from as well, which is the whole point of it living in the shared
//!   crate. [`Palette::color`] is an exhaustive `match`, so a role added over
//!   there does not *compile* here until this surface paints it. That is the
//!   real guard; the roster test below only covers what is already listed.
//! * **Terminal chrome** — status-bar ground, pane borders, connection glyphs —
//!   has no Panel counterpart, because the Panel's chrome is CSS that shares
//!   nothing with a status bar. Those are the only colours this crate names on
//!   its own, and they are marked as such in [`Theme`].
//!
//! [`Theme`] itself is **derived**: every field that corresponds to a semantic
//! role is `Palette::color(that role)`, never a second literal. A palette and a
//! theme const holding their own copies of "what colour is an error" is the
//! shape that drifts (判据 §1), and it is what this file used to be.
//!
//! # Why the current theme is process-global rather than injected
//!
//! It replaces a global `const`, so this is not new globality — but it is a
//! deliberate choice over threading a `&Theme` into ~20 widget signatures to
//! express a value that is, by construction, the same everywhere in a
//! single-surface TUI. What it makes harder: a future split pane that wants two
//! themes at once would have to do that threading after all. Writers: `/theme`
//! and boot. Readers: every widget, through [`theme()`].

use std::sync::RwLock;

use ratatui::style::Color;
use shared_ui_logic::transcript::SemanticColor;

/// Braille spinner frames, re-exported so this crate has no second copy of
/// them. They used to be spelled out here as `&[&str]` with the identical
/// sequence — the same fact in two places, where only one of them can be the
/// one a future change updates (判据 §1).
pub use shared_ui_logic::transcript::SPINNER_FRAMES;

/// Frame for a tick counter, for the call sites that count ticks rather than
/// milliseconds.
///
/// The millisecond form (`shared_ui_logic::transcript::spinner_frame`) is the
/// one to prefer: being a pure function of wall-clock time, every spinner on
/// screen repaints in step without a shared counter. This exists so the
/// tick-driven widgets keep working while they still count ticks.
#[must_use]
pub fn spinner_at(tick: usize) -> char {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

/// Which colour set the surface is painting with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preset {
    #[default]
    Dark,
    Light,
    /// Yield to whatever the terminal is already themed as: ANSI-16 plus
    /// [`Color::Reset`] for plain foreground. Chosen by users who have
    /// configured their terminal and want it respected — and the automatic
    /// answer when the terminal cannot do 24-bit colour, because the
    /// alternative is emitting RGB the terminal will approximate badly.
    Terminal,
}

impl Preset {
    /// Parse a `/theme` argument. `None` for an unknown name — the caller
    /// reports the name back rather than silently picking a default, which
    /// would leave the user believing a typo'd theme had been applied.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Terminal => "terminal",
        }
    }

    /// Every preset a user may name, for `/theme` completion and its error
    /// message. Derived from nothing — this IS the list, and [`Self::parse`]
    /// is checked against it below.
    pub const ALL: &'static [Self] = &[Self::Dark, Self::Light, Self::Terminal];

    /// The comma-separated choices, for a message that has to name them.
    #[must_use]
    pub fn choices() -> String {
        Self::ALL
            .iter()
            .map(|p| p.name())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// The preset the session starts in.
    ///
    /// # Why an env var and not a settings file
    ///
    /// R4: an interface does not persist. This crate has no config file of its
    /// own and must not grow one — and a terminal's colour preference is the
    /// same *kind* of fact as `COLORTERM` or `NO_COLOR`, which is to say a
    /// property of the terminal the user launched, not of the conversation.
    /// Putting it in the session would also be wrong in the other direction: it
    /// would follow the conversation to the Panel, which does not paint with
    /// these colours at all.
    ///
    /// So `/theme` switches for this run and `ALEPH_TUI_THEME` is how it
    /// sticks. An unset or unrecognised value falls back to the default rather
    /// than failing to start.
    #[must_use]
    pub fn from_env() -> Self {
        std::env::var("ALEPH_TUI_THEME")
            .ok()
            .and_then(|v| Self::parse(&v))
            .unwrap_or_default()
    }
}

/// Whether the terminal advertises 24-bit colour.
///
/// `COLORTERM` is the only portable signal, and its absence is not proof of
/// absence — plenty of capable terminals do not set it. Treating "unknown" as
/// "no" is the fail-closed direction here: an indexed palette renders
/// acceptably everywhere, while RGB on a 16-colour terminal is approximated
/// per-terminal and can come out unreadable.
#[must_use]
pub fn supports_truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| {
        let v = v.to_ascii_lowercase();
        v.contains("truecolor") || v.contains("24bit")
    })
}

/// A preset plus what the terminal can actually show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub preset: Preset,
    pub truecolor: bool,
}

impl Palette {
    /// The palette for `preset` on this terminal.
    #[must_use]
    pub fn detect(preset: Preset) -> Self {
        Self {
            preset,
            truecolor: supports_truecolor(),
        }
    }

    /// Whether this palette paints in RGB. `false` means [`Self::rgb`] answers
    /// `None` for every role and derived backgrounds must degrade rather than
    /// invent an approximation.
    #[must_use]
    pub const fn is_rgb(self) -> bool {
        self.truecolor && !matches!(self.preset, Preset::Terminal)
    }

    /// The RGB triple for a role, or `None` when this palette is not painting
    /// in RGB.
    ///
    /// Callers that mix (diff row and word-emphasis backgrounds, through
    /// `shared_ui_logic::transcript::mix_rgb`) need the components, and there
    /// is nothing honest to mix on an indexed palette: `None` is "I cannot
    /// give you a background", not "black" (判据 §8).
    #[must_use]
    pub fn rgb(self, role: SemanticColor) -> Option<(u8, u8, u8)> {
        self.is_rgb().then(|| match self.preset {
            Preset::Light => light_rgb(role),
            // `is_rgb()` already excluded `Terminal`.
            Preset::Dark | Preset::Terminal => dark_rgb(role),
        })
    }

    /// The colour for a role.
    #[must_use]
    pub fn color(self, role: SemanticColor) -> Color {
        match self.rgb(role) {
            Some((r, g, b)) => Color::Rgb(r, g, b),
            None => indexed(role, self.preset),
        }
    }
}

/// Dark-preset RGB. One arm per role: adding a [`SemanticColor`] fails to
/// compile here until it is given a value.
const fn dark_rgb(role: SemanticColor) -> (u8, u8, u8) {
    match role {
        SemanticColor::Fg => (220, 220, 220),
        SemanticColor::Dim => (128, 128, 128),
        SemanticColor::Accent => (215, 119, 87),
        SemanticColor::Prompt => (212, 175, 55),
        SemanticColor::UserBar => (97, 175, 239),
        SemanticColor::ToolPending => (110, 110, 110),
        SemanticColor::ToolRunning => (229, 192, 123),
        SemanticColor::ToolOk => (152, 195, 121),
        SemanticColor::ToolErr => (224, 108, 117),
        SemanticColor::ToolRail => (90, 90, 90),
        SemanticColor::DiffAdd => (152, 195, 121),
        SemanticColor::DiffDel => (224, 108, 117),
        SemanticColor::DiffCtx => (150, 150, 150),
        SemanticColor::DiffGutter => (110, 110, 110),
        SemanticColor::AdmonitionNote => (97, 175, 239),
        SemanticColor::AdmonitionTip => (152, 195, 121),
        SemanticColor::AdmonitionImportant => (198, 120, 221),
        SemanticColor::AdmonitionWarning => (229, 192, 123),
        SemanticColor::AdmonitionCaution => (224, 108, 117),
        SemanticColor::CodeBorder => (80, 80, 80),
        SemanticColor::Link => (97, 175, 239),
        SemanticColor::ContextOk => (152, 195, 121),
        SemanticColor::ContextWarn => (229, 192, 123),
        SemanticColor::ContextDanger => (224, 108, 117),
        SemanticColor::Cost => (152, 195, 121),
        SemanticColor::Spinner => (229, 192, 123),
    }
}

/// Light-preset RGB: the same roles darkened enough to sit on a light ground.
const fn light_rgb(role: SemanticColor) -> (u8, u8, u8) {
    match role {
        SemanticColor::Fg => (40, 40, 40),
        SemanticColor::Dim => (120, 120, 120),
        SemanticColor::Accent => (175, 80, 50),
        SemanticColor::Prompt => (160, 120, 20),
        SemanticColor::UserBar => (30, 110, 190),
        SemanticColor::ToolPending => (140, 140, 140),
        SemanticColor::ToolRunning => (170, 120, 20),
        SemanticColor::ToolOk => (40, 130, 60),
        SemanticColor::ToolErr => (190, 45, 55),
        SemanticColor::ToolRail => (175, 175, 175),
        SemanticColor::DiffAdd => (40, 130, 60),
        SemanticColor::DiffDel => (190, 45, 55),
        SemanticColor::DiffCtx => (95, 95, 95),
        SemanticColor::DiffGutter => (140, 140, 140),
        SemanticColor::AdmonitionNote => (30, 110, 190),
        SemanticColor::AdmonitionTip => (40, 130, 60),
        SemanticColor::AdmonitionImportant => (130, 70, 165),
        SemanticColor::AdmonitionWarning => (170, 120, 20),
        SemanticColor::AdmonitionCaution => (190, 45, 55),
        SemanticColor::CodeBorder => (190, 190, 190),
        SemanticColor::Link => (30, 110, 190),
        SemanticColor::ContextOk => (40, 130, 60),
        SemanticColor::ContextWarn => (170, 120, 20),
        SemanticColor::ContextDanger => (190, 45, 55),
        SemanticColor::Cost => (40, 130, 60),
        SemanticColor::Spinner => (170, 120, 20),
    }
}

/// ANSI-16 for the `terminal` preset and for any terminal that cannot do
/// 24-bit colour. `Color::Reset` where "the terminal's own foreground" is the
/// honest answer.
fn indexed(role: SemanticColor, preset: Preset) -> Color {
    match role {
        SemanticColor::Fg => match preset {
            Preset::Terminal => Color::Reset,
            Preset::Dark => Color::White,
            Preset::Light => Color::Black,
        },
        SemanticColor::Dim | SemanticColor::ToolPending | SemanticColor::DiffGutter => {
            Color::DarkGray
        }
        SemanticColor::Accent | SemanticColor::Prompt => Color::Yellow,
        SemanticColor::UserBar | SemanticColor::AdmonitionNote | SemanticColor::Link => Color::Blue,
        SemanticColor::ToolRunning
        | SemanticColor::AdmonitionWarning
        | SemanticColor::ContextWarn
        | SemanticColor::Spinner => Color::Yellow,
        SemanticColor::ToolOk
        | SemanticColor::DiffAdd
        | SemanticColor::AdmonitionTip
        | SemanticColor::ContextOk
        | SemanticColor::Cost => Color::Green,
        SemanticColor::ToolErr
        | SemanticColor::DiffDel
        | SemanticColor::AdmonitionCaution
        | SemanticColor::ContextDanger => Color::Red,
        SemanticColor::ToolRail | SemanticColor::CodeBorder | SemanticColor::DiffCtx => Color::Gray,
        SemanticColor::AdmonitionImportant => Color::Magenta,
    }
}

/// The painted palette, flattened into the field names the widgets use.
///
/// Everything above the `// --- terminal chrome ---` line is
/// [`Palette::color`] of a semantic role and holds no colour of its own; a
/// test below asserts that, so "derived" stays a fact rather than a comment.
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

    // --- terminal chrome: no Panel counterpart, no semantic role ---
    pub border: Color,
    pub border_focused: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub connected: Color,
    pub disconnected: Color,
}

impl Theme {
    #[must_use]
    pub fn from_palette(p: Palette) -> Self {
        let c = |role| p.color(role);
        Self {
            user: c(SemanticColor::UserBar),
            assistant: c(SemanticColor::Fg),
            system: c(SemanticColor::AdmonitionNote),
            tool_running: c(SemanticColor::ToolRunning),
            tool_success: c(SemanticColor::ToolOk),
            tool_failed: c(SemanticColor::ToolErr),
            tool_name: c(SemanticColor::Accent),
            heading: c(SemanticColor::Fg),
            primary: c(SemanticColor::Fg),
            muted: c(SemanticColor::Dim),
            reasoning: c(SemanticColor::Dim),
            error: c(SemanticColor::ToolErr),
            warning: c(SemanticColor::AdmonitionWarning),

            border: Color::DarkGray,
            border_focused: match p.preset {
                Preset::Terminal => Color::Reset,
                Preset::Dark => Color::White,
                Preset::Light => Color::Black,
            },
            status_bg: match p.preset {
                Preset::Terminal => Color::Reset,
                Preset::Dark => Color::DarkGray,
                Preset::Light => Color::Gray,
            },
            status_fg: match p.preset {
                Preset::Terminal => Color::Reset,
                Preset::Dark => Color::White,
                Preset::Light => Color::Black,
            },
            connected: c(SemanticColor::ToolOk),
            disconnected: c(SemanticColor::ToolErr),
        }
    }

    /// The role each derived field is painted from. Exists so the test that
    /// proves the derivation cannot go stale independently of
    /// [`Self::from_palette`] — the two are read side by side.
    #[cfg(test)]
    const DERIVED: &'static [(&'static str, SemanticColor)] = &[
        ("user", SemanticColor::UserBar),
        ("assistant", SemanticColor::Fg),
        ("system", SemanticColor::AdmonitionNote),
        ("tool_running", SemanticColor::ToolRunning),
        ("tool_success", SemanticColor::ToolOk),
        ("tool_failed", SemanticColor::ToolErr),
        ("tool_name", SemanticColor::Accent),
        ("heading", SemanticColor::Fg),
        ("primary", SemanticColor::Fg),
        ("muted", SemanticColor::Dim),
        ("reasoning", SemanticColor::Dim),
        ("error", SemanticColor::ToolErr),
        ("warning", SemanticColor::AdmonitionWarning),
        ("connected", SemanticColor::ToolOk),
        ("disconnected", SemanticColor::ToolErr),
    ];

    #[cfg(test)]
    fn field(&self, name: &str) -> Color {
        match name {
            "user" => self.user,
            "assistant" => self.assistant,
            "system" => self.system,
            "tool_running" => self.tool_running,
            "tool_success" => self.tool_success,
            "tool_failed" => self.tool_failed,
            "tool_name" => self.tool_name,
            "heading" => self.heading,
            "primary" => self.primary,
            "muted" => self.muted,
            "reasoning" => self.reasoning,
            "error" => self.error,
            "warning" => self.warning,
            "connected" => self.connected,
            "disconnected" => self.disconnected,
            other => panic!("Theme::DERIVED names a field that does not exist: {other}"),
        }
    }
}

/// The palette in force. One writer (`/theme`, and boot), many readers.
static CURRENT: RwLock<Option<Palette>> = RwLock::new(None);

/// Install `preset` as the surface's palette. Called at boot and by `/theme`.
pub fn set_preset(preset: Preset) {
    let p = Palette::detect(preset);
    // 判据 P7: a poisoned lock is a panic elsewhere, not a reason to lose the
    // theme — take the guard and carry on.
    *CURRENT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(p);
}

/// The palette in force, detecting the default on first read.
#[must_use]
pub fn palette() -> Palette {
    if let Some(p) = *CURRENT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return p;
    }
    let p = Palette::detect(Preset::from_env());
    *CURRENT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(p);
    p
}

/// The preset in force, for a command that has to report it.
#[must_use]
pub fn current_preset() -> Preset {
    palette().preset
}

/// The theme in force. `Theme` is `Copy`, so widgets take it by value.
#[must_use]
pub fn theme() -> Theme {
    Theme::from_palette(palette())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared_ui_logic::transcript::ALL_SEMANTIC_COLORS;

    /// Every role resolves in every preset, in both colour depths.
    ///
    /// # When this goes red
    ///
    /// It cannot go red by a role being *added* — `Palette::color` is an
    /// exhaustive `match`, so that is a compile error, which is the stronger
    /// guard and the reason the mapping is written as a `match` rather than a
    /// table lookup with a default. What this catches is a role resolving to
    /// the same colour as [`Color::Reset`] outside the `terminal` preset,
    /// i.e. a role that was added to the `match` with a placeholder arm and
    /// never given a value.
    #[test]
    fn every_role_resolves_in_every_preset() {
        for preset in Preset::ALL {
            for truecolor in [false, true] {
                let p = Palette {
                    preset: *preset,
                    truecolor,
                };
                for role in ALL_SEMANTIC_COLORS {
                    let c = p.color(*role);
                    if *preset != Preset::Terminal {
                        assert_ne!(
                            c,
                            Color::Reset,
                            "{role:?} is unpainted in {:?}/{truecolor}",
                            preset
                        );
                    }
                }
            }
        }
    }

    /// The `terminal` preset must not emit RGB whatever the terminal claims:
    /// its whole purpose is to defer to colours the user already chose.
    #[test]
    fn the_terminal_preset_never_emits_rgb() {
        let p = Palette {
            preset: Preset::Terminal,
            truecolor: true,
        };
        assert!(!p.is_rgb());
        for role in ALL_SEMANTIC_COLORS {
            assert!(
                !matches!(p.color(*role), Color::Rgb(..)),
                "{role:?} emitted RGB under the terminal preset"
            );
            assert!(p.rgb(*role).is_none(), "{role:?} offered RGB components");
        }
    }

    /// A 16-colour terminal gets indexed colours, and `rgb()` says so rather
    /// than handing back a triple nothing will paint (判据 §8).
    #[test]
    fn an_indexed_terminal_is_told_it_has_no_components() {
        let p = Palette {
            preset: Preset::Dark,
            truecolor: false,
        };
        for role in ALL_SEMANTIC_COLORS {
            assert!(p.rgb(*role).is_none(), "{role:?} offered RGB components");
            assert!(!matches!(p.color(*role), Color::Rgb(..)));
        }
    }

    /// `Theme`'s semantic fields hold no colour of their own.
    ///
    /// This is what makes the module doc's "derived" claim true. Give
    /// `Theme::from_palette` a literal for one of these fields and this goes
    /// red for that field, in every preset.
    #[test]
    fn the_semantic_theme_fields_are_derived_not_restated() {
        for preset in Preset::ALL {
            for truecolor in [false, true] {
                let p = Palette {
                    preset: *preset,
                    truecolor,
                };
                let t = Theme::from_palette(p);
                for (name, role) in Theme::DERIVED {
                    assert_eq!(
                        t.field(name),
                        p.color(*role),
                        "Theme::{name} is not {role:?} under {:?}/{truecolor}",
                        preset
                    );
                }
            }
        }
    }

    #[test]
    fn every_preset_round_trips_through_its_name() {
        for preset in Preset::ALL {
            assert_eq!(Preset::parse(preset.name()), Some(*preset));
        }
        assert_eq!(Preset::parse("  DARK  "), Some(Preset::Dark));
        assert_eq!(
            Preset::parse("solarized"),
            None,
            "an unknown name must not silently become the default"
        );
    }

    /// The tick-indexed helper and the shared millisecond form walk the same
    /// table — the thing a second local copy of the frames used to make
    /// unprovable.
    #[test]
    fn the_spinner_table_has_one_source() {
        for tick in 0..SPINNER_FRAMES.len() * 3 {
            let by_tick = spinner_at(tick);
            let by_ms = shared_ui_logic::transcript::spinner_frame(
                tick as u64 * shared_ui_logic::transcript::SPINNER_PERIOD_MS,
            );
            assert_eq!(by_tick, by_ms, "tick {tick}");
        }
    }
}
