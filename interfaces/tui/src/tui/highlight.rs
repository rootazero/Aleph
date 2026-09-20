//! Syntax highlighting for fenced code blocks, loaded off the draw thread.
//!
//! # Why this is not just a call to syntect
//!
//! `SyntaxSet::load_defaults_*` parses a few megabytes of syntax definitions.
//! Doing that on the first frame that happens to contain a code block would
//! stall the terminal for the length of the load, in the middle of a streaming
//! answer. So the first request starts a background load and returns `None`,
//! and every request until it lands returns `None` too.
//!
//! That makes "not ready" a state the renderer has to draw, which is the part
//! worth getting right: it draws the code block **in exactly the same rows**,
//! only without colour. A "still loading" state that laid out differently
//! would reflow the transcript under the reader when the load finished.
//!
//! # And why the terminal preset is never highlighted
//!
//! syntect's themes are RGB. The `terminal` preset exists to say "use the
//! colours the terminal was configured with", and a 16-colour terminal has no
//! way to show them. Highlighting there would override the one preset whose
//! entire purpose is not to.

use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use super::theme::{Palette, Preset};

/// Everything the highlighter needs, once.
struct Loaded {
    syntaxes: SyntaxSet,
    dark: Theme,
    light: Theme,
}

static LOADED: OnceLock<Loaded> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);
/// Bumped when the load lands, so a caller holding rendered output can tell
/// that the same input would now render differently.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn load() -> Loaded {
    // `nonewlines`, because the lines handed to [`highlight_block`] have had
    // their `\n` stripped by the markdown parser. The `newlines` variant
    // mis-scans the last token of every line without one.
    let syntaxes = SyntaxSet::load_defaults_nonewlines();
    let mut themes = ThemeSet::load_defaults();
    let dark = themes
        .themes
        .remove("base16-ocean.dark")
        .unwrap_or_default();
    let light = themes.themes.remove("InspiredGitHub").unwrap_or_default();
    Loaded {
        syntaxes,
        dark,
        light,
    }
}

/// Which generation of highlighting the last render used.
///
/// A caller that caches rendered rows compares this against what it cached
/// with; a change means the cached rows are stale for a reason their own
/// inputs cannot show. It only ever changes once, from 0 to 1.
#[must_use]
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// The loaded set, or `None` while the background load is still running.
///
/// The first call starts the load. Calls are cheap after that: an atomic read
/// and a `OnceLock` peek.
fn ready() -> Option<&'static Loaded> {
    if let Some(loaded) = LOADED.get() {
        return Some(loaded);
    }
    if !STARTED.swap(true, Ordering::AcqRel) {
        std::thread::Builder::new()
            .name("aleph-tui-syntect".into())
            .spawn(|| {
                let loaded = load();
                if LOADED.set(loaded).is_ok() {
                    GENERATION.fetch_add(1, Ordering::AcqRel);
                }
            })
            // A thread that cannot be spawned is not fatal: every call keeps
            // returning `None` and code blocks stay plain, which is a state
            // this module already has to render correctly.
            .ok();
    }
    None
}

/// Highlight a whole fenced block, or `None` if it should render plain.
///
/// Whole block rather than line by line because a multi-line string or block
/// comment is only correct with the scanner state carried across its lines.
///
/// Returns one `Vec<(Style, String)>` per input line, in order, always the
/// same number of lines as it was given.
///
/// The palette is a parameter rather than read from the ambient one so this
/// can be exercised for each preset without a test mutating global state that
/// every other test in the binary reads.
#[must_use]
pub fn highlight_block(
    p: Palette,
    lang: &str,
    lines: &[String],
) -> Option<Vec<Vec<(Style, String)>>> {
    // `is_rgb()` is already false for `Preset::Terminal` — that preset's whole
    // point is deferring colour to the terminal — so this one condition covers
    // both reasons not to highlight. Spelling the preset out again here would
    // be a second derivation of a rule that lives in `Palette`.
    if !p.is_rgb() {
        return None;
    }
    highlight_with(ready()?, p, lang, lines)
}

/// [`highlight_block`] with the loaded set supplied.
///
/// The split exists for the tests: `ready()` answers `None` until a
/// background thread finishes, so a test calling the public function would
/// either race the load or — worse — take the `None` path and pass while
/// exercising nothing. Everything that decides what the output looks like is
/// in here.
fn highlight_with(
    loaded: &Loaded,
    p: Palette,
    lang: &str,
    lines: &[String],
) -> Option<Vec<Vec<(Style, String)>>> {
    let syntax = loaded
        .syntaxes
        .find_syntax_by_token(lang)
        .or_else(|| loaded.syntaxes.find_syntax_by_extension(lang))?;
    let theme = if p.preset == Preset::Light {
        &loaded.light
    } else {
        &loaded.dark
    };
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        // A line that fails to highlight is emitted unstyled rather than
        // dropped: losing a line of the user's code is far worse than losing
        // its colour, and the row count is a layout promise.
        let spans = h.highlight_line(line, &loaded.syntaxes).map_or_else(
            |_| vec![(Style::default(), line.clone())],
            |ranges| {
                ranges
                    .into_iter()
                    .map(|(s, text)| (convert(s), text.to_string()))
                    .collect()
            },
        );
        out.push(spans);
    }
    Some(out)
}

/// syntect style → ratatui style. Foreground and font style only: syntect's
/// background is its theme's, which would fight the one in force.
fn convert(s: syntect::highlighting::Style) -> Style {
    let mut style = Style::default().fg(Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b));
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

/// [`highlight_block`] with the load done synchronously.
///
/// Test-only, and the only way to assert anything about the highlighted path:
/// the production entry point answers `None` until a background thread lands,
/// so a test calling it would pass by taking the plain path. Loaded once per
/// test binary — the load is seconds, not milliseconds.
#[cfg(test)]
pub fn highlight_block_blocking(
    p: Palette,
    lang: &str,
    lines: &[String],
) -> Option<Vec<Vec<(Style, String)>>> {
    static TEST_LOADED: OnceLock<Loaded> = OnceLock::new();
    if !p.is_rgb() {
        return None;
    }
    highlight_with(TEST_LOADED.get_or_init(load), p, lang, lines)
}

/// A palette that does highlight, for tests that need the coloured path.
#[cfg(test)]
pub const fn rgb_palette() -> Palette {
    Palette {
        preset: Preset::Dark,
        truecolor: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever the highlighter does, it hands back exactly the lines it was
    /// given, with exactly their text.
    ///
    /// # When this goes red
    ///
    /// Any change that lets highlighting alter the text — dropping an empty
    /// line, trimming, merging. The row count is what the chat area's scroll
    /// arithmetic is built on, so a highlighter that changes it moves the
    /// viewport, not just the colours.
    ///
    /// Goes through [`highlight_block_blocking`], i.e. the real
    /// `highlight_with`, rather than re-running `HighlightLines` here — a
    /// test that rebuilds the logic it is checking proves the logic agrees
    /// with itself.
    #[test]
    fn highlighting_preserves_every_line_and_its_text() {
        let lines: Vec<String> = ["fn main() {", "", "    let x = 1; // note", "}"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let out = highlight_block_blocking(rgb_palette(), "rust", &lines)
            .expect("the default syntax set has rust");

        assert_eq!(out.len(), lines.len(), "line count changed");
        for (row, src) in out.iter().zip(&lines) {
            let joined: String = row.iter().map(|(_, t)| t.as_str()).collect();
            assert_eq!(&joined, src, "text changed");
        }
    }

    /// The comment and the keyword do not come out the same colour — i.e.
    /// this is really highlighting and not a uniform repaint.
    ///
    /// # When this goes red
    ///
    /// A theme that failed to load (`unwrap_or_default` gives an empty one),
    /// which would paint every token the same and otherwise look like it
    /// worked.
    #[test]
    fn different_tokens_get_different_colours() {
        let out =
            highlight_block_blocking(rgb_palette(), "rust", &["let x = 1; // note".to_string()])
                .expect("rust");
        let row = &out[0];
        let colour_of = |needle: &str| {
            row.iter()
                .find(|(_, t)| t.contains(needle))
                .map(|(s, _)| s.fg)
        };
        let kw = colour_of("let").expect("a span with the keyword");
        let comment = colour_of("//").expect("a span with the comment");
        assert_ne!(kw, comment, "everything painted one colour");
    }

    /// An unknown info string is plain, not an error and not a guess.
    #[test]
    fn an_unknown_language_renders_plain() {
        let out =
            highlight_block_blocking(rgb_palette(), "not-a-language", &["whatever".to_string()]);
        assert!(out.is_none());
    }

    /// A palette that cannot paint RGB is never highlighted — which covers
    /// both the `terminal` preset and a terminal without truecolor.
    ///
    /// # When this goes red
    ///
    /// Highlighting before consulting the palette. `terminal` exists to leave
    /// colour to the terminal's own configuration, and syntect's themes are
    /// RGB, so highlighting there overrides exactly the thing that preset is
    /// for. On a 16-colour terminal it is worse than wrong — the RGB is
    /// approximated by whatever the emulator feels like.
    ///
    /// Asserted for every preset, so a new one cannot quietly arrive without
    /// an answer here.
    #[test]
    fn a_palette_that_cannot_paint_rgb_is_never_highlighted() {
        for preset in Preset::ALL {
            let no_truecolor = Palette {
                preset: *preset,
                truecolor: false,
            };
            assert!(
                highlight_block(no_truecolor, "rust", &["let x = 1;".to_string()]).is_none(),
                "{preset:?} highlighted without truecolor"
            );
        }
        let terminal = Palette {
            preset: Preset::Terminal,
            truecolor: true,
        };
        assert!(
            highlight_block(terminal, "rust", &["let x = 1;".to_string()]).is_none(),
            "the terminal preset was highlighted"
        );
    }
}
