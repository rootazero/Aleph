//! Box-drawing glyphs for separators and tables.
//!
//! Falls back to ASCII (`-`, `+`) when [`super::icon::use_unicode`] returns
//! false.

use super::icon::use_unicode;

/// Horizontal rule used as table separators and section dividers.
pub fn horizontal() -> char {
    if use_unicode() {
        '─'
    } else {
        '-'
    }
}

/// Cross intersection (used at column boundaries when gutter=0).
fn cross() -> char {
    if use_unicode() {
        '┼'
    } else {
        '+'
    }
}

/// Build a horizontal separator string of width `width`, optionally with
/// `column_widths` injected so the separator aligns with table columns.
///
/// When `column_widths` is non-empty, the result has Unicode `┼` (or ASCII
/// `+`) at each column boundary. With no column widths, returns a flat run
/// of horizontal rules.
pub fn separator(column_widths: &[usize], gutter: usize) -> String {
    let h = horizontal();
    let x = cross();
    if column_widths.is_empty() {
        return String::new();
    }
    // Each column gets its own run; gutters are filled with `gutter` rules
    // and joined by `x` only when gutter > 0.
    let mut out = String::new();
    for (i, &w) in column_widths.iter().enumerate() {
        if i > 0 {
            if gutter == 0 {
                out.push(x);
            } else {
                // Render gutter as: `<gutter rules>` (no cross — keeps it
                // visually quieter than full grid lines).
                for _ in 0..gutter {
                    out.push(h);
                }
            }
        }
        for _ in 0..w {
            out.push(h);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separator_empty_widths_is_empty() {
        assert_eq!(separator(&[], 2), "");
    }

    #[test]
    fn separator_runs_total_to_widths_plus_gutters() {
        // 3 + 2 + 4 widths with gutter 2 ⇒ 3 + 2 + 2 + 2 + 4 = 13 chars
        let s = separator(&[3, 2, 4], 2);
        assert_eq!(s.chars().count(), 13);
    }

    #[test]
    fn separator_with_zero_gutter_inserts_cross() {
        let s = separator(&[2, 2], 0);
        // Should be `┼` (or `+`) joining: 2 + 1 + 2 = 5 chars
        assert_eq!(s.chars().count(), 5);
    }
}
