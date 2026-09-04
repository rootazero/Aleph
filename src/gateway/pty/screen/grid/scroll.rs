//! Everything that shifts whole rows: the newline/reverse-index pair, `SU`
//! and `SD`, and the two scrolls they share. All of it reads the scrolling
//! region, which is why it is one file rather than scattered among the verbs
//! that call it.

use super::{Cell, Grid};

impl Grid {
    /// Move to the next row, scrolling the region when the cursor is already
    /// on its LAST row -- not the screen's. A cursor parked below the region
    /// (legal: nothing confines it there) just moves down.
    pub fn newline(&mut self) {
        let (_, bottom) = self.scroll_region;
        if self.cursor_row == bottom {
            self.scroll_up();
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    /// RI (`ESC M`): up one row, opening a blank row at the region's top when
    /// the cursor is already there. The mirror of [`Self::newline`], and the
    /// reason a program that scrolls BACKWARDS gets new space instead of
    /// stale text it believes it replaced.
    pub fn reverse_index(&mut self) {
        let (top, _) = self.scroll_region;
        if self.cursor_row == top {
            self.scroll_down();
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    /// SU (`CSI S`): scroll the region up `n` rows, cursor unmoved.
    ///
    /// `n` is capped at the region's height: more than that cannot do more
    /// than blank it, and an uncapped loop would run `u16::MAX` times on a
    /// parameter a remote program chose.
    pub fn scroll_up_n(&mut self, n: u16) {
        let (top, bottom) = self.scroll_region;
        for _ in 0..n.min(bottom - top + 1) {
            self.scroll_up();
        }
    }

    /// SD (`CSI T`): scroll the region down `n` rows, cursor unmoved.
    pub fn scroll_down_n(&mut self, n: u16) {
        let (top, bottom) = self.scroll_region;
        for _ in 0..n.min(bottom - top + 1) {
            self.scroll_down();
        }
    }

    /// Shift the scrolling region up one row, blanking the row it vacates at
    /// the region's bottom.
    ///
    /// **Only a FULL-HEIGHT scroll files the evicted row as scrollback.**
    /// Scrollback means "what fell off the top of the screen"; a row leaving
    /// the top of a region never reached the top of the screen, so filing it
    /// would let a client scrolling back read rows the user never saw leave
    /// — the same reasoning [`Self::insert_lines`] already carries for rows
    /// pushed off the bottom.
    fn scroll_up(&mut self) {
        let (top, bottom) = self.scroll_region;
        if top == 0 && bottom == self.rows - 1 {
            let first: Vec<Cell> = self.row_cells(top).to_vec();
            if self.scrollback.len() >= self.scrollback_limit {
                self.scrollback.pop_front();
            }
            self.scrollback.push_back(first);
        }
        self.rotate_region(top, bottom, true);
    }

    /// Shift the scrolling region down one row, blanking the row it vacates
    /// at the region's top. Never touches scrollback: the row it discards
    /// left through the BOTTOM, and scrollback only ever means the top.
    fn scroll_down(&mut self) {
        let (top, bottom) = self.scroll_region;
        self.rotate_region(top, bottom, false);
    }

    /// The cell moving both scrolls share. Rotating wraps the row leaving one
    /// end around to the other, where it is immediately blanked — so the
    /// wrap is how the vacated row gets cleared, not a leak.
    fn rotate_region(&mut self, top: u16, bottom: u16, up: bool) {
        let cols = self.cols as usize;
        let start = top as usize * cols;
        let end = (bottom as usize + 1) * cols;
        let region = &mut self.cells[start..end];
        if up {
            region.rotate_left(cols);
        } else {
            region.rotate_right(cols);
        }
        let vacated = if up {
            region.len() - cols..region.len()
        } else {
            0..cols
        };
        for cell in &mut region[vacated] {
            *cell = Cell::default();
        }
        self.dirty.extend(top..=bottom);
    }
}
