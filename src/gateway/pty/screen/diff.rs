//! Dirty-row tracking and the wire patch.
//!
//! Rows are re-sent whole rather than cell-by-cell. Cell-level diffs save
//! bandwidth but buy a whole class of "one cell never updated" bugs, and a row
//! is only ~200 cells. Whole-row re-send has the property that every frame is
//! self-healing.

use super::grid::{Attrs, Cell, Color, Grid};

/// A run of consecutive cells sharing one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleRun {
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPatch {
    pub row: u16,
    pub runs: Vec<StyleRun>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenPatch {
    pub rows: Vec<RowPatch>,
    pub cursor: Option<(u16, u16)>,
    pub alt_screen: Option<bool>,
    pub title: Option<String>,
    pub bell: bool,
}

impl ScreenPatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
            && self.cursor.is_none()
            && self.alt_screen.is_none()
            && self.title.is_none()
            && !self.bell
    }
}

/// Fold one row's cells into style runs. Spacer cells (the right half of a
/// wide glyph) carry no character and are dropped: the client re-derives the
/// width from the glyph itself.
pub(crate) fn row_runs(cells: &[Cell]) -> Vec<StyleRun> {
    let mut runs: Vec<StyleRun> = Vec::new();
    for cell in cells {
        if cell.is_spacer() {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.fg == cell.fg && last.bg == cell.bg && last.attrs == cell.attrs => {
                last.text.push(cell.ch);
            }
            _ => runs.push(StyleRun {
                text: cell.ch.to_string(),
                fg: cell.fg,
                bg: cell.bg,
                attrs: cell.attrs,
            }),
        }
    }
    runs
}

pub(crate) fn patch_rows(grid: &Grid, rows: impl IntoIterator<Item = u16>) -> Vec<RowPatch> {
    rows.into_iter()
        .map(|row| RowPatch { row, runs: row_runs(grid.row_cells(row)) })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::gateway::pty::screen::grid::{Attrs, Color};
    use crate::gateway::pty::screen::Screen;

    #[test]
    fn only_dirty_rows_are_emitted() {
        let mut s = Screen::new(4, 20);
        s.feed(b"a\r\nb\r\n");
        let p = s.take_patch().expect("first write is dirty");
        let rows: Vec<u16> = p.rows.iter().map(|r| r.row).collect();
        assert_eq!(rows, vec![0, 1], "untouched rows 2 and 3 must not ship");
    }

    /// The whole point of the 16 ms cadence: a quiet terminal costs nothing.
    #[test]
    fn a_second_take_with_no_writes_is_none() {
        let mut s = Screen::new(4, 20);
        s.feed(b"a");
        let _ = s.take_patch();
        assert!(s.take_patch().is_none(), "a quiet screen must produce no frame");
    }

    #[test]
    fn same_style_cells_collapse_into_one_run() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[31mRED\x1b[0mplain");
        let p = s.take_patch().expect("dirty");
        let runs = &p.rows[0].runs;
        assert_eq!(runs.len(), 2, "two styles, two runs");
        assert_eq!(runs[0].text, "RED");
        assert_eq!(runs[0].fg, Color::Indexed(1));
        assert_eq!(runs[1].text.trim_end(), "plain");
        assert_eq!(runs[1].fg, Color::Default);
        assert_eq!(runs[1].attrs, Attrs::NONE);
    }

    #[test]
    fn a_title_change_rides_along_and_is_reported_once() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]0;t1\x07x");
        assert_eq!(s.take_patch().and_then(|p| p.title), Some("t1".to_string()));
        s.feed(b"y");
        assert_eq!(s.take_patch().and_then(|p| p.title), None, "an unchanged title must not reship");
    }

    /// A full snapshot is what `pty.attach` hands a fresh client, so it must
    /// carry every row — including the ones no write has touched.
    #[test]
    fn a_full_patch_carries_every_row() {
        let mut s = Screen::new(4, 20);
        s.feed(b"only-row-0");
        let full = s.full_patch();
        assert_eq!(full.rows.len(), 4);
        assert_eq!(full.cursor, Some(s.grid.cursor()));
    }
}
