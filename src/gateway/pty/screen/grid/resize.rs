//! Resizing the grid, and the edge repair only a narrowing resize can need.

use super::{Cell, Grid};

impl Grid {
    /// Resize, keeping the top-left content that still fits. Reflow is
    /// deliberately not attempted: a wrong reflow scrambles a screen the user
    /// is looking at, whereas clipping is legible and self-corrects on the
    /// application's next repaint (which every full-screen app does on SIGWINCH).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        let mut next = vec![Cell::default(); rows as usize * cols as usize];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                next[r as usize * cols as usize + c as usize] = self.cells[self.idx(r, c)];
            }
            self.repair_edge_truncated_glyph(&mut next, r, cols);
        }
        self.cells = next;
        self.rows = rows;
        self.cols = cols;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        // A region is stated in rows, so it cannot outlive a change to how
        // many rows there are. Clamping instead would keep a region the
        // program sized for the old screen, which is a region it never asked
        // for; the program repaints on SIGWINCH and states a new one.
        self.scroll_region = (0, rows - 1);
        // Every coordinate on the wire changed meaning (new width, possibly
        // new height), so mark every surviving row dirty. A shrinking
        // resize can otherwise leave stale row indices >= the new
        // `self.rows` in the dirty set (rows dirtied by a write before the
        // shrink) -- `patch_rows` does not filter those, so an unfiltered
        // stale index would ship a `RowPatch` labelled with a row number
        // the client's now-smaller grid does not have.
        self.dirty.retain(|&r| r < self.rows);
        self.dirty.extend(0..self.rows);
    }

    /// Narrowing can cut a wide glyph's spacer off at the new right edge,
    /// leaving its owner alone in `next` with no spacer to follow it — the
    /// same corruption class [`Self::repair_straddled_glyph`] guards
    /// against for writes, here for the edge a resize can create instead of
    /// a cursor write. If the first column this narrowing drops was a
    /// spacer, the column it keeps right before it must be the spacer's
    /// owner; blank that owner rather than let a truncated half-glyph
    /// survive into the resized grid.
    ///
    /// Two different widths are in play, and indexing must not mix them up:
    /// `self.idx` always computes an offset into the OLD grid (`self.cells`,
    /// still `self.cols` wide — the field is not reassigned until after
    /// `resize`'s copy loop returns), which is why `self.idx(row, new_cols)`
    /// correctly reads the dropped column's original content below. `next`
    /// is a different array, already `new_cols` wide from the moment
    /// `resize` allocated it — so the owner's index into `next` is computed
    /// by hand with `new_cols`, not via `self.idx`, which would compute the
    /// wrong offset for it.
    fn repair_edge_truncated_glyph(&self, next: &mut [Cell], row: u16, new_cols: u16) {
        if new_cols >= self.cols {
            return; // widening or unchanged: nothing was cut off.
        }
        let dropped = self.idx(row, new_cols);
        if self.cells[dropped].is_spacer() {
            let owner = row as usize * new_cols as usize + (new_cols - 1) as usize;
            next[owner] = Cell::default();
        }
    }
}
