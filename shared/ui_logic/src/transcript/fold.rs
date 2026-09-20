//! Fold a tool result body to a few PHYSICAL rows.
//!
//! The rule that matters (pi-claude-code-tui `MAX_RESULT_ROWS`, the #1
//! "most likely to be got wrong" item): wrap FIRST, then count. One minified
//! JSON line is sixty terminal rows; counting logical lines would show all
//! sixty. The logical line count participates only as a safety net — if the
//! body has more logical lines than the limit it is truncated regardless of
//! how the wrap measured.

use unicode_width::UnicodeWidthChar;

/// Rows shown when collapsed (excluding the `… +N lines` hint row).
pub const DEFAULT_COLLAPSED_ROWS: u16 = 2;
/// Width assumed when the surface cannot measure yet (first frame, hidden
/// container). Unknown must still fold — never "skip the fold".
pub const FALLBACK_WIDTH: u16 = 80;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldAnchor {
    /// Keep the first rows (default).
    Head,
    /// Keep the last rows — shell output, where the error is at the end.
    Tail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldBody {
    /// Show `collapsed_rows` of body when collapsed.
    Show,
    /// Show no body when collapsed (a `file_read` shows only `Read 120 lines`).
    Hide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldPolicy {
    pub collapsed_rows: u16,
    pub anchor: FoldAnchor,
    pub body: FoldBody,
}

impl Default for FoldPolicy {
    fn default() -> Self {
        Self {
            collapsed_rows: DEFAULT_COLLAPSED_ROWS,
            anchor: FoldAnchor::Head,
            body: FoldBody::Show,
        }
    }
}

impl FoldPolicy {
    /// Per-tool overrides (spec §5 `fold` row). Keyed by the DISPLAY name
    /// produced by `summarize::display_name`, so MCP/unknown tools get the
    /// default and no name list has to be kept in two places.
    #[must_use]
    pub fn for_display_name(display_name: &str) -> Self {
        match display_name {
            "Read" => Self {
                body: FoldBody::Hide,
                ..Self::default()
            },
            "Bash" => Self {
                anchor: FoldAnchor::Tail,
                ..Self::default()
            },
            _ => Self::default(),
        }
    }
}

/// Result of a fold. `rows` are physical (already wrapped to `width`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Folded {
    pub rows: Vec<String>,
    /// Physical rows not shown. What the `… +N lines` hint reports.
    pub hidden_rows: usize,
    /// Logical lines not fully shown (for the "N lines" wording when a
    /// surface prefers logical units, e.g. `Read 120 lines`).
    pub hidden_lines: usize,
    pub truncated: bool,
}

/// Wrap one logical line into physical rows of at most `width` columns,
/// measuring with `unicode-width` (CJK = 2 columns, combining = 0).
/// Never returns an empty Vec: an empty line is one empty row.
#[must_use]
pub fn wrap_physical(line: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut rows = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in line.chars() {
        let w = ch.width().unwrap_or(0);
        if cur_w + w > width && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += w;
    }
    rows.push(cur);
    rows
}

/// Fold `lines` (logical) to the policy's collapsed height at `width`.
/// `width == 0` is treated as unknown → [`FALLBACK_WIDTH`].
#[must_use]
pub fn fold(lines: &[&str], width: u16, policy: FoldPolicy) -> Folded {
    let width = if width == 0 { FALLBACK_WIDTH } else { width };
    let limit = usize::from(policy.collapsed_rows);
    let total_lines = lines.len();

    if policy.body == FoldBody::Hide {
        let hidden_rows: usize = lines.iter().map(|l| wrap_physical(l, width).len()).sum();
        return Folded {
            rows: Vec::new(),
            hidden_rows,
            hidden_lines: total_lines,
            truncated: total_lines > 0,
        };
    }

    // Logical safety net: more logical lines than rows → truncates for sure,
    // even if a wrap measurement disagreed.
    let logical_truncates = total_lines > limit;

    // Wrap everything. Bodies are capped upstream (TUI keeps ≤ 64 KB per row
    // in memory; larger output is fetched on expand), so a full wrap is a
    // few hundred microseconds — no pre-scan shortcut is worth its edge cases.
    let mut physical: Vec<(usize, String)> = Vec::new(); // (logical index, row)
    for (i, line) in lines.iter().enumerate() {
        for row in wrap_physical(line, width) {
            physical.push((i, row));
        }
    }

    let total_rows = physical.len();
    let truncated = logical_truncates || total_rows > limit;
    if !truncated {
        return Folded {
            rows: physical.into_iter().map(|(_, r)| r).collect(),
            hidden_rows: 0,
            hidden_lines: 0,
            truncated: false,
        };
    }

    let (shown, first_shown_line, last_shown_line) = match policy.anchor {
        FoldAnchor::Head => {
            let s: Vec<&(usize, String)> = physical.iter().take(limit).collect();
            let first = s.first().map_or(0, |(i, _)| *i);
            let last = s.last().map_or(0, |(i, _)| *i);
            (s, first, last)
        }
        FoldAnchor::Tail => {
            let skip = total_rows.saturating_sub(limit);
            let s: Vec<&(usize, String)> = physical.iter().skip(skip).collect();
            let first = s.first().map_or(0, |(i, _)| *i);
            let last = s.last().map_or(0, |(i, _)| *i);
            (s, first, last)
        }
    };
    let shown_rows: Vec<String> = shown.iter().map(|(_, r)| (*r).clone()).collect();
    let hidden_rows = total_rows - shown_rows.len();
    // A logical line counts as hidden unless EVERY one of its rows is shown.
    let rows_of = |li: usize| physical.iter().filter(|(i, _)| *i == li).count();
    let shown_rows_of = |li: usize| shown.iter().filter(|(i, _)| *i == li).count();
    let fully_shown = (first_shown_line..=last_shown_line)
        .filter(|li| shown_rows_of(*li) == rows_of(*li))
        .count();
    let hidden_lines = total_lines.saturating_sub(fully_shown);

    Folded {
        rows: shown_rows,
        hidden_rows,
        hidden_lines,
        truncated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_body_is_not_folded() {
        let f = fold(&["a", "b"], 80, FoldPolicy::default());
        assert!(!f.truncated);
        assert_eq!(f.rows, vec!["a", "b"]);
        assert_eq!(f.hidden_rows, 0);
    }

    #[test]
    fn head_keeps_the_first_two_rows_and_counts_the_rest() {
        let f = fold(&["l1", "l2", "l3", "l4", "l5"], 80, FoldPolicy::default());
        assert!(f.truncated);
        assert_eq!(f.rows, vec!["l1", "l2"]);
        assert_eq!(f.hidden_rows, 3);
        assert_eq!(f.hidden_lines, 3);
    }

    #[test]
    fn tail_keeps_the_last_two_rows() {
        let p = FoldPolicy::for_display_name("Bash");
        let f = fold(&["l1", "l2", "l3", "l4", "l5"], 80, p);
        assert_eq!(f.rows, vec!["l4", "l5"]);
        assert_eq!(f.hidden_rows, 3);
    }

    #[test]
    fn one_minified_json_line_folds_by_physical_rows_not_logical_lines() {
        // The regression this module exists for: raw newline count is 0, yet
        // at width 80 this is dozens of rows and must fold.
        let blob = "x".repeat(6 * 1024);
        let f = fold(&[blob.as_str()], 80, FoldPolicy::default());
        assert!(f.truncated, "a 6 KB single line must fold");
        assert_eq!(f.rows.len(), 2);
        assert!(f.rows.iter().all(|r| r.chars().count() == 80));
        assert!(
            f.hidden_rows > 20,
            "hidden rows must follow wrap width, got {}",
            f.hidden_rows
        );
    }

    #[test]
    fn cjk_is_two_columns_wide() {
        // 41 CJK chars = 82 columns → 2 rows at width 80.
        let s = "中".repeat(41);
        let rows = wrap_physical(&s, 80);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].chars().count(), 40);
        assert_eq!(rows[1].chars().count(), 1);
    }

    #[test]
    fn hide_body_shows_nothing_but_still_counts() {
        let p = FoldPolicy::for_display_name("Read");
        let f = fold(&["a", "b", "c"], 80, p);
        assert!(f.rows.is_empty());
        assert_eq!(f.hidden_lines, 3);
        assert!(f.truncated);
    }

    #[test]
    fn unknown_width_falls_back_to_eighty_columns_rather_than_skipping() {
        let blob = "y".repeat(500);
        let f = fold(&[blob.as_str()], 0, FoldPolicy::default());
        assert!(f.truncated);
        assert_eq!(f.rows[0].chars().count(), usize::from(FALLBACK_WIDTH));
    }

    #[test]
    fn exactly_the_limit_is_not_truncated() {
        let f = fold(&["a", "b"], 80, FoldPolicy::default());
        assert!(!f.truncated);
    }

    #[test]
    fn a_thousand_lines_report_exact_hidden_counts() {
        let lines: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let f = fold(&refs, 80, FoldPolicy::default());
        assert_eq!(f.rows, vec!["line 0", "line 1"]);
        assert_eq!(f.hidden_rows, 998);
        assert_eq!(f.hidden_lines, 998);
    }

    #[test]
    fn a_partially_shown_line_counts_as_hidden() {
        // Line 0 wraps to 3 rows at width 4; only 2 rows fit, so line 0 is
        // partially hidden and line 1 entirely: hidden_lines must be 2, not 1.
        let f = fold(&["abcdefghij", "k"], 4, FoldPolicy::default());
        assert_eq!(f.rows, vec!["abcd", "efgh"]);
        assert_eq!(f.hidden_rows, 2);
        assert_eq!(f.hidden_lines, 2);
    }
}
