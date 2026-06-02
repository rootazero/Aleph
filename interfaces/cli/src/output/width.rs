//! Display-width helpers shared by table / detail / markdown rendering.
//!
//! Previously each renderer carried its own `display_width` that counted
//! Unicode scalars (treating every codepoint as width 1). That mis-aligns any
//! output containing wide characters — CJK text, full-width punctuation, most
//! emoji — which is common for Aleph's Chinese-speaking users. This module
//! centralises the calculation on `unicode-width` (already a workspace
//! dependency via the TUI crate) so columns line up correctly.

use unicode_width::UnicodeWidthStr;

/// Visible terminal width of `s` in cells.
///
/// Wide characters (CJK, full-width forms) count as 2; combining marks and
/// control characters count as 0. Input is expected to be plain text (no ANSI
/// escapes) — callers compute widths before colourising.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Right-pad `s` with spaces to reach `width` display cells. If `s` is already
/// at least `width` wide, it is returned unchanged.
pub fn pad_to(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (width - w));
    out.push_str(s);
    for _ in 0..(width - w) {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_width_is_byte_count() {
        assert_eq!(display_width("hi"), 2);
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn cjk_characters_are_double_width() {
        // Each Han character occupies two terminal cells.
        assert_eq!(display_width("中"), 2);
        assert_eq!(display_width("中文"), 4);
        // Mixed ASCII + CJK.
        assert_eq!(display_width("a中b"), 4);
    }

    #[test]
    fn pad_to_accounts_for_wide_chars() {
        // "中" is width 2, so padding to 4 adds exactly 2 spaces.
        assert_eq!(pad_to("中", 4), "中  ");
        // Already wide enough → unchanged.
        assert_eq!(pad_to("中文", 3), "中文");
    }

    #[test]
    fn pad_to_pads_ascii_with_spaces() {
        assert_eq!(pad_to("hi", 5), "hi   ");
    }

    #[test]
    fn cjk_table_columns_align() {
        // Two cells of equal display width must pad to the same total width.
        let a = pad_to("中文", 6); // width 4 → +2 spaces = 6 cells
        let b = pad_to("abcd", 6); // width 4 → +2 spaces = 6 cells
        assert_eq!(display_width(&a), display_width(&b));
    }
}
