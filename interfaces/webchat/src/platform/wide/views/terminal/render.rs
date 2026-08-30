//! Canvas2d grid renderer.
//!
//! Only dirty-free full repaints are attempted here: the client screen is
//! already the diff's result, and a 200x50 grid of style runs paints in well
//! under a frame. Run-level `fill_text` (not per-cell) is what keeps it cheap.

use aleph_protocol::pty::{PtyAttrs, PtyColor};
use unicode_width::UnicodeWidthStr;
use web_sys::CanvasRenderingContext2d;

use super::session::ClientScreen;

/// One cell's pixel size, measured once from the loaded font. No longer
/// `Copy` now that it carries a `String` -- callers that need the same
/// value twice hold it by reference or `.clone()`, which is one extra
/// character at each of the two call sites this cost, in exchange for the
/// family living in exactly one place instead of two.
#[derive(Debug, Clone, PartialEq)]
pub struct CellMetrics {
    pub width: f64,
    pub height: f64,
    /// The font size, in CSS px, these metrics were measured at.
    ///
    /// Carried rather than re-stated because `paint` builds its own
    /// `set_font` string per run (bold and italic vary per run) and would
    /// otherwise be free to draw at a size the layout was not measured
    /// for -- the grid advancing by one size while glyphs draw at
    /// another, with nothing anywhere to report it.
    pub font_px: f64,
    /// The CSS `font-family` list these metrics were measured with, and the
    /// same value `paint` must draw every run with.
    ///
    /// This field is the fix for a double-write this file shipped once: the
    /// family used to also be a literal inside `paint`, matching
    /// `measure`'s caller only because the two literals happened to read
    /// the same. The moment the family became configurable, `measure`
    /// would have measured one font while `paint` drew another and the
    /// whole grid would drift. Carrying it here, the way `font_px` already
    /// was carried, makes that drift structurally impossible instead of a
    /// thing to remember: `paint` has no family literal to drift FROM (see
    /// `run_font`, and `paint_never_states_a_font_family_literal` below).
    pub font_family: String,
}

impl CellMetrics {
    /// A zero or non-finite metric means the font has not loaded. Painting
    /// with it divides by zero; callers check this first.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

/// How many cells fit. Floors, and never returns zero: a pane measured
/// mid-layout is 0x0, and a zero-column PTY is not a thing.
#[must_use]
pub fn viewport_cells(px_w: f64, px_h: f64, m: &CellMetrics) -> (u16, u16) {
    if !m.is_usable() {
        return (1, 1);
    }
    let cols = (px_w / m.width).floor().max(1.0).min(f64::from(u16::MAX));
    let rows = (px_h / m.height).floor().max(1.0).min(f64::from(u16::MAX));
    (rows as u16, cols as u16)
}

/// Colour resolution. The server never sends a concrete palette because it
/// does not know the client's theme; `Default` and `Indexed` resolve here.
pub struct Theme {
    pub fg: &'static str,
    pub bg: &'static str,
    pub palette: [&'static str; 16],
}

impl Theme {
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            fg: "#e8e6e1",
            bg: "#0d0d12",
            palette: [
                "#0d0d12", "#e05561", "#8cc265", "#d18f52", "#4aa5f0", "#c162de", "#42b3c2",
                "#a1a8b3", "#4d5566", "#ff6b7f", "#a5e075", "#f0a45d", "#63b0ff", "#d977f5",
                "#5fd0e0", "#e8e6e1",
            ],
        }
    }

    #[must_use]
    pub fn resolve_fg(&self, c: PtyColor) -> String {
        match c {
            PtyColor::Default => self.fg.to_string(),
            PtyColor::Indexed(n) => self.palette[(n as usize) % 16].to_string(),
            PtyColor::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    #[must_use]
    pub fn resolve_bg(&self, c: PtyColor) -> Option<String> {
        match c {
            PtyColor::Default => None,
            other => Some(self.resolve_fg(other)),
        }
    }
}

/// The font this module falls back to when a requested family or size
/// cannot be trusted -- the RPC failed, or (see `apply_font`) a font string
/// the browser silently rejected. Hand-written, never user input, so it is
/// the one font string in this module allowed to be a literal.
///
/// Deliberately just `monospace`, not a font stack: `TerminalConfig` in
/// `alephcore` already owns the opinion about what a good DEFAULT stack is
/// (`DEFAULT_TERMINAL_FONT_FAMILY`, with Nerd Font names) and the Panel
/// reads it as the server's EFFECTIVE config -- defaults already applied,
/// see `mod.rs`'s font-config effect. A second, differently-composed stack
/// here would be the identical double-write this task exists to close one
/// level up, except unrecoverable: this constant is reached only when there
/// is NO answer from the server at all, so a competing opinion here could
/// never be corrected by fixing the config. `monospace` alone carries no
/// opinion to disagree with -- it is a visible, honest "nothing loaded"
/// signal (plain letters, no icons), not a second guess at the real default.
pub(super) const FALLBACK_FONT_FAMILY: &str = "monospace";
pub(super) const FALLBACK_FONT_SIZE_PX: f64 = 14.0;

/// Bounds on a configured font size. `0` (or a negative/garbage value from a
/// config typo) must not be able to reach `ctx.measure_text` at all -- a
/// zero or negative cell height divides by zero everywhere downstream. 6px
/// is small enough that anything below it means the operator meant a
/// different unit or field, not a terminal anyone could read; 96px just
/// keeps a fat-fingered extra digit from producing an unusable page. Either
/// end of this range still measures to a positive, finite cell size.
const MIN_FONT_SIZE_PX: f64 = 6.0;
const MAX_FONT_SIZE_PX: f64 = 96.0;

/// A CSS font string that will never occur by accident: used as a
/// before/after sentinel in `apply_font` to detect a `set_font` call the
/// browser silently rejected. Deliberately quoted (syntactically a valid,
/// if fictitious, custom family) so setting it can never itself fail.
const FONT_PROBE_SENTINEL: &str = "1px 'aleph-terminal-font-probe-sentinel'";

/// Apply `family`/`size_px` to `ctx`, verifying the browser actually
/// accepted the string rather than trusting that it did, and returning
/// whatever ended up active.
///
/// `CanvasRenderingContext2d::set_font` on a syntactically malformed string
/// is a silent no-op per spec: the browser leaves the PREVIOUS font in
/// place rather than erroring. A naive caller that just calls `set_font`
/// and moves on gets a confidently wrong grid from one missing quote in a
/// config value -- `measure_text` would report the OLD font's metrics for
/// cell sizing while nothing ever says so.
///
/// Detected with a before/after sentinel rather than by comparing
/// `ctx.font()` against the REQUESTED string: a browser is free to
/// re-serialize a syntactically valid font string (re-quoting a family
/// name, reordering, ...), so exact-string comparison against the request
/// would false-positive on that canonicalization -- on literally every
/// successful call, since this runs on every resize and every repaint with
/// the same configured font. Comparing against a sentinel we control (set
/// immediately before the real attempt) only asks "did the string we just
/// tried change anything", which is the question that actually matters.
fn apply_font(ctx: &CanvasRenderingContext2d, family: &str, size_px: f64) -> (String, f64) {
    ctx.set_font(FONT_PROBE_SENTINEL);
    let requested = format!("{size_px}px {family}");
    ctx.set_font(&requested);
    if ctx.font() != FONT_PROBE_SENTINEL {
        return (family.to_string(), size_px);
    }
    // Rejected: the sentinel is still active, meaning `requested` never
    // took. Fall back to the hand-written literal, trusted syntactically
    // valid because it is ours, not configuration.
    let fallback = format!("{FALLBACK_FONT_SIZE_PX}px {FALLBACK_FONT_FAMILY}");
    ctx.set_font(&fallback);
    (FALLBACK_FONT_FAMILY.to_string(), FALLBACK_FONT_SIZE_PX)
}

/// Measure the monospace cell. `measure_text` on a wide sample divided by its
/// length is steadier than measuring one glyph, which sub-pixel rounding can
/// skew enough to drift a column over 200 cells.
///
/// `size_px` is clamped before it ever reaches the canvas (see
/// `MIN_FONT_SIZE_PX`/`MAX_FONT_SIZE_PX`); `family` is verified by
/// `apply_font` and silently replaced with the fallback stack if the
/// browser rejected it. Either way, the `(family, size)` carried on the
/// returned `CellMetrics` is what is ACTUALLY active on `ctx`, never what
/// was merely requested.
#[must_use]
pub fn measure(ctx: &CanvasRenderingContext2d, family: &str, size_px: f64) -> CellMetrics {
    let size_px = size_px.clamp(MIN_FONT_SIZE_PX, MAX_FONT_SIZE_PX);
    let (font_family, font_px) = apply_font(ctx, family, size_px);
    // ASCII only, and it must stay ASCII: the divisor below is `len()`, a BYTE
    // count. A non-ASCII sample would silently make every cell too narrow.
    const SAMPLE: &str = "MMMMMMMMMMMMMMMMMMMM";
    let width = ctx
        .measure_text(SAMPLE)
        .map(|m| m.width() / SAMPLE.len() as f64)
        .unwrap_or(0.0);
    // Line height is not measurable portably; the canonical ratio for a
    // terminal is ~1.2x the em box.
    CellMetrics {
        width,
        height: (font_px * 1.2).round(),
        font_px,
        font_family,
    }
}

/// Pixel width a run occupies on screen. NOT `text.chars().count()`: a wide
/// glyph (CJK / emoji) is one `char` of text but occupies two columns of
/// screen — the server's `diff.rs::row_runs` drops the spacer cell that
/// would have otherwise carried the second column. `UnicodeWidthStr::width`
/// is the same crate and the same per-char semantics the server's `grid.rs`
/// uses to decide that in the first place; a hand-rolled width table here
/// would be a second, driftable copy of that same fact.
#[must_use]
fn run_extent(text: &str, cell_width: f64) -> f64 {
    UnicodeWidthStr::width(text) as f64 * cell_width
}

/// The CSS `font` shorthand for one run, given its bold/italic attributes.
///
/// Pure and separated out so it is testable without a live canvas, and so
/// `paint` itself has no font-family or font-size literal to state --
/// both come from `m`, the SAME `CellMetrics` `measure` produced. That is
/// what makes `measure` and `paint` sharing one font a structural fact
/// rather than a coincidence of two literals reading the same (see
/// `CellMetrics::font_family`'s doc, and
/// `run_font_is_derived_from_the_metrics_it_is_given_not_a_literal` below).
#[must_use]
fn run_font(m: &CellMetrics, attrs: PtyAttrs) -> String {
    let weight = if attrs.has(PtyAttrs::BOLD) {
        "bold "
    } else {
        ""
    };
    let style = if attrs.has(PtyAttrs::ITALIC) {
        "italic "
    } else {
        ""
    };
    format!("{style}{weight}{}px {}", m.font_px, m.font_family)
}

/// Repaint the whole grid.
pub fn paint(ctx: &CanvasRenderingContext2d, screen: &ClientScreen, m: CellMetrics, theme: &Theme) {
    if !m.is_usable() {
        return;
    }
    let (rows, cols) = screen.dims();
    let (w, h) = (f64::from(cols) * m.width, f64::from(rows) * m.height);

    ctx.set_fill_style_str(theme.bg);
    ctx.fill_rect(0.0, 0.0, w, h);

    for row in 0..rows {
        let y = f64::from(row) * m.height;
        let mut x = 0.0_f64;
        for run in screen.row_runs(row) {
            let run_w = run_extent(&run.text, m.width);
            if let Some(bg) = theme.resolve_bg(run.bg) {
                ctx.set_fill_style_str(&bg);
                ctx.fill_rect(x, y, run_w, m.height);
            }
            ctx.set_fill_style_str(&theme.resolve_fg(run.fg));
            ctx.set_font(&run_font(&m, run.attrs));
            let _ = ctx.fill_text(&run.text, x, y + m.height * 0.8);
            x += run_w;
        }
    }

    // Cursor as a block overlay.
    let (cr, cc) = screen.cursor();
    ctx.set_fill_style_str(theme.fg);
    ctx.set_global_alpha(0.6);
    ctx.fill_rect(
        f64::from(cc) * m.width,
        f64::from(cr) * m.height,
        m.width,
        m.height,
    );
    ctx.set_global_alpha(1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Viewport → grid dimensions. Off-by-one here means the server sizes the
    /// PTY to a screen the client cannot show, and the bottom row is cut off
    /// on every single session.
    #[test]
    fn viewport_cells_floors_and_never_returns_zero() {
        let m = CellMetrics {
            width: 8.0,
            height: 17.0,
            font_px: 14.0,
            font_family: "monospace".to_string(),
        };
        assert_eq!(viewport_cells(800.0, 340.0, &m), (20, 100));
        assert_eq!(
            viewport_cells(7.0, 3.0, &m),
            (1, 1),
            "a tiny pane still needs one cell"
        );
        assert_eq!(
            viewport_cells(0.0, 0.0, &m),
            (1, 1),
            "a pane mid-layout must not divide by zero"
        );
    }

    /// A zero or NaN metric means the font has not loaded yet. Rendering with
    /// it produces a division by zero, so the caller must be able to tell.
    #[test]
    fn metrics_report_whether_they_are_usable() {
        assert!(CellMetrics {
            width: 8.0,
            height: 17.0,
            font_px: 14.0,
            font_family: "monospace".to_string()
        }
        .is_usable());
        assert!(!CellMetrics {
            width: 0.0,
            height: 17.0,
            font_px: 14.0,
            font_family: "monospace".to_string()
        }
        .is_usable());
        assert!(!CellMetrics {
            width: f64::NAN,
            height: 17.0,
            font_px: 14.0,
            font_family: "monospace".to_string()
        }
        .is_usable());
    }

    #[test]
    fn indexed_colours_map_into_the_sixteen_colour_palette() {
        use aleph_protocol::pty::PtyColor;
        let t = Theme::dark();
        assert_eq!(t.resolve_fg(PtyColor::indexed(1)), t.palette[1]);
        assert_eq!(t.resolve_fg(PtyColor::Default), t.fg);
        assert_eq!(t.resolve_fg(PtyColor::rgb(1, 2, 3)), "#010203");
    }

    /// The regression this brief exists to prevent: `chars().count()` and
    /// `UnicodeWidthStr::width()` agree for ASCII and diverge the moment a
    /// wide glyph appears. An ASCII-only test structurally cannot catch this
    /// — the server side of this same invariant was missed three times by
    /// exactly that blind spot. This exercises `run_extent`, the same helper
    /// `paint` advances `x` with, rather than re-deriving the arithmetic.
    #[test]
    fn wide_glyphs_advance_two_columns_not_one_char() {
        let m = CellMetrics {
            width: 8.0,
            height: 17.0,
            font_px: 14.0,
            font_family: "monospace".to_string(),
        };
        // "你好" is 2 CJK chars — 1 unit each under `chars().count()` — but 2
        // display columns each, 4 columns total, not 2.
        let cjk = "你好";
        assert_ne!(
            UnicodeWidthStr::width(cjk),
            cjk.chars().count(),
            "this sample must exercise the divergence, or the assertions below prove nothing"
        );

        // Position of a second run, following the CJK run in the same row —
        // this is exactly the `x` a real `paint()` call would hand `fill_text`
        // for it. `paint()` itself needs a live `CanvasRenderingContext2d`
        // that only exists in a browser; Part 2's real-device rig covers that.
        let mut x = 0.0_f64;
        x += run_extent(cjk, m.width);
        let second_run_x = x;
        x += run_extent("!", m.width);

        assert_eq!(
            second_run_x,
            4.0 * m.width,
            "a run after two wide glyphs must be offset by 4 columns, not 2"
        );
        assert_eq!(
            x,
            second_run_x + m.width,
            "an ASCII run after it must still advance by exactly 1 column"
        );
    }

    /// `measure` and `paint` (via `run_font`) sharing one font is a
    /// structural fact now, not two literals that happen to agree -- so
    /// this asserts the OUTPUT tracks the INPUT, not that two hardcoded
    /// strings are equal (the latter is exactly the shape the original bug
    /// had: it would still pass if `run_font` silently ignored `m` and
    /// hardcoded a family, as long as the hardcoded string matched
    /// `measure`'s literal too). Two different `CellMetrics` must produce
    /// two different font strings, tracking BOTH family and size.
    #[test]
    fn run_font_is_derived_from_the_metrics_it_is_given_not_a_literal() {
        let a = CellMetrics {
            width: 8.0,
            height: 17.0,
            font_px: 14.0,
            font_family: "Family A".to_string(),
        };
        let b = CellMetrics {
            width: 8.0,
            height: 17.0,
            font_px: 20.0,
            font_family: "Family B".to_string(),
        };
        let fa = run_font(&a, PtyAttrs::default());
        let fb = run_font(&b, PtyAttrs::default());
        assert!(fa.contains("Family A") && fa.contains("14"), "got {fa:?}");
        assert!(fb.contains("Family B") && fb.contains("20"), "got {fb:?}");
        assert_ne!(
            fa, fb,
            "two different CellMetrics must produce two different font strings"
        );
    }

    /// Source-level guard against the double-write this file already
    /// shipped once: the font family stated a second time, as a literal
    /// inside `paint`, drifting from `measure`'s the moment the two are no
    /// longer forced to match by coincidence. `paint` now has no family
    /// literal to state at all (see `run_font`) -- this is the regression
    /// sentinel that keeps it that way. Manually broken once while writing
    /// it: reintroducing `ctx.set_font(&format!("...{}px 'JetBrains Mono',
    /// monospace", m.font_px))` inside `paint`'s body turns this red with
    /// the offending body printed, exactly as intended.
    #[test]
    fn paint_never_states_a_font_family_literal() {
        let src = include_str!("render.rs").replace('\r', "");
        let start = src.find("pub fn paint(").expect("paint is gone");
        let body_start = src[start..]
            .find('{')
            .map(|i| start + i)
            .expect("paint has a body");
        // The next brace at column 0 (immediately preceded by a newline) is
        // `paint`'s own closing brace: every block INSIDE it is indented by
        // rustfmt, so only the enclosing top-level item's brace can appear
        // unindented.
        let body_end = src[body_start..]
            .find("\n}\n")
            .map(|i| body_start + i)
            .expect("paint's closing brace");
        let body = &src[body_start..body_end];
        assert!(
            !body.contains('\'') && !body.contains("monospace"),
            "paint's body states a font family literal -- the family must \
             come from `CellMetrics::font_family` via `run_font`, never a \
             literal, or `measure` and `paint` can silently draw two \
             different fonts. Offending body:\n{body}"
        );
    }
}
