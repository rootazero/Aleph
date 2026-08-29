//! Canvas2d grid renderer.
//!
//! Only dirty-free full repaints are attempted here: the client screen is
//! already the diff's result, and a 200x50 grid of style runs paints in well
//! under a frame. Run-level `fill_text` (not per-cell) is what keeps it cheap.

use aleph_protocol::pty::{PtyAttrs, PtyColor};
use unicode_width::UnicodeWidthStr;
use web_sys::CanvasRenderingContext2d;

use super::session::ClientScreen;

/// One cell's pixel size, measured once from the loaded monospace font.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    pub width: f64,
    pub height: f64,
}

impl CellMetrics {
    /// A zero or non-finite metric means the font has not loaded. Painting
    /// with it divides by zero; callers check this first.
    #[must_use]
    pub fn is_usable(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

/// How many cells fit. Floors, and never returns zero: a pane measured
/// mid-layout is 0x0, and a zero-column PTY is not a thing.
#[must_use]
pub fn viewport_cells(px_w: f64, px_h: f64, m: CellMetrics) -> (u16, u16) {
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

/// Measure the monospace cell. `measure_text` on a wide sample divided by its
/// length is steadier than measuring one glyph, which sub-pixel rounding can
/// skew enough to drift a column over 200 cells.
#[must_use]
pub fn measure(ctx: &CanvasRenderingContext2d, font: &str) -> CellMetrics {
    ctx.set_font(font);
    // ASCII only, and it must stay ASCII: the divisor below is `len()`, a BYTE
    // count. A non-ASCII sample would silently make every cell too narrow.
    const SAMPLE: &str = "MMMMMMMMMMMMMMMMMMMM";
    let width = ctx
        .measure_text(SAMPLE)
        .map(|m| m.width() / SAMPLE.len() as f64)
        .unwrap_or(0.0);
    // Line height is not measurable portably; the canonical ratio for a
    // terminal is ~1.2x the em box, and the font size is parsed from `font`.
    let px = font
        .split_whitespace()
        .find_map(|t| t.strip_suffix("px").and_then(|n| n.parse::<f64>().ok()))
        .unwrap_or(14.0);
    CellMetrics { width, height: (px * 1.2).round() }
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
            let weight = if run.attrs.has(PtyAttrs::BOLD) { "bold " } else { "" };
            let style = if run.attrs.has(PtyAttrs::ITALIC) { "italic " } else { "" };
            ctx.set_font(&format!("{style}{weight}14px 'JetBrains Mono', monospace"));
            let _ = ctx.fill_text(&run.text, x, y + m.height * 0.8);
            x += run_w;
        }
    }

    // Cursor as a block overlay.
    let (cr, cc) = screen.cursor();
    ctx.set_fill_style_str(theme.fg);
    ctx.set_global_alpha(0.6);
    ctx.fill_rect(f64::from(cc) * m.width, f64::from(cr) * m.height, m.width, m.height);
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
        let m = CellMetrics { width: 8.0, height: 17.0 };
        assert_eq!(viewport_cells(800.0, 340.0, m), (20, 100));
        assert_eq!(viewport_cells(7.0, 3.0, m), (1, 1), "a tiny pane still needs one cell");
        assert_eq!(viewport_cells(0.0, 0.0, m), (1, 1), "a pane mid-layout must not divide by zero");
    }

    /// A zero or NaN metric means the font has not loaded yet. Rendering with
    /// it produces a division by zero, so the caller must be able to tell.
    #[test]
    fn metrics_report_whether_they_are_usable() {
        assert!(CellMetrics { width: 8.0, height: 17.0 }.is_usable());
        assert!(!CellMetrics { width: 0.0, height: 17.0 }.is_usable());
        assert!(!CellMetrics { width: f64::NAN, height: 17.0 }.is_usable());
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
        let m = CellMetrics { width: 8.0, height: 17.0 };
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

        assert_eq!(second_run_x, 4.0 * m.width, "a run after two wide glyphs must be offset by 4 columns, not 2");
        assert_eq!(x, second_run_x + m.width, "an ASCII run after it must still advance by exactly 1 column");
    }
}
