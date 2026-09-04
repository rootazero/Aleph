//! The erase and insert/delete verbs -- `ECH`, `DCH`, `ICH`, `IL`, `DL`,
//! `ED`, `EL` -- and the two helpers that keep a wide glyph from being left
//! half-present when a range or a row-wide shift cuts through it.

use unicode_width::UnicodeWidthChar;

use super::{Cell, Grid};

impl Grid {
    /// CSI X (ECH): blank `n` cells from the cursor. Unlike DCH this moves
    /// nothing — neither the cursor nor the cells after the erased run.
    pub fn erase_chars(&mut self, n: u16) {
        let from = self.idx(self.cursor_row, self.cursor_col);
        let to = from + n.min(self.cols - self.cursor_col) as usize;
        self.clear_range(from, to);
    }

    /// CSI P (DCH): delete `n` cells at the cursor, pulling the rest of the
    /// row left and blanking what it vacates at the right edge.
    pub fn delete_chars(&mut self, n: u16) {
        let cols = self.cols as usize;
        let at = self.cursor_col as usize;
        let n = (n as usize).min(cols - at);
        let start = self.idx(self.cursor_row, 0);
        let row = &mut self.cells[start..start + cols];
        row.copy_within(at + n.., at);
        for cell in &mut row[cols - n..] {
            *cell = Cell::default();
        }
        self.dirty.insert(self.cursor_row);
        self.repair_row_pairs(self.cursor_row);
    }

    /// CSI @ (ICH): insert `n` blanks at the cursor, pushing the rest of the
    /// row right; whatever that pushes past the right edge is gone.
    pub fn insert_chars(&mut self, n: u16) {
        let cols = self.cols as usize;
        let at = self.cursor_col as usize;
        let n = (n as usize).min(cols - at);
        let start = self.idx(self.cursor_row, 0);
        let row = &mut self.cells[start..start + cols];
        row.copy_within(at..cols - n, at + n);
        for cell in &mut row[at..at + n] {
            *cell = Cell::default();
        }
        self.dirty.insert(self.cursor_row);
        self.repair_row_pairs(self.cursor_row);
    }

    /// A row-wide shift splits any wide glyph the shift boundary fell
    /// inside: the displaced half leaves the survivor claiming a partner
    /// that is no longer beside it. [`Self::clear_range`] handles this for
    /// erasures by widening the range, but a shift has no range to widen,
    /// so the pairs are re-checked afterwards and any half left alone is
    /// blanked.
    ///
    /// What each kind of damage looks like if this does not run:
    /// [`super::diff::row_runs`] drops spacers on the way to the wire, so an
    /// orphaned SPACER reaches the client as a row one column short, and an
    /// orphaned OWNER as a glyph the client draws two columns wide while the
    /// server counts one — a row one column long, with everything after it
    /// displaced. Both are client-visible; only the second leaves nothing
    /// behind for a test to find, which is why the tests assert characters
    /// rather than "no spacer remains". [`Self::row_text`] filters spacers
    /// too, so a test written against it cannot see either.
    ///
    /// One left-to-right pass suffices, and the reason is that the loop
    /// never creates a new orphan: blanking writes a width-1 blank, and
    /// neither predicate at an already-visited `c - 1` can be flipped by
    /// that. `orphan_owner(c-1)` reads `cells[c].is_spacer()` — in the
    /// orphan-spacer case `cells[c-1]` was already established not to be an
    /// owner, and in the orphan-owner case `cells[c]` was not a spacer
    /// before or after. `orphan_spacer(c-1)` reads only `cells[c-2]` and
    /// `cells[c-1]`, which this write does not touch.
    fn repair_row_pairs(&mut self, row: u16) {
        let start = self.idx(row, 0);
        let cols = self.cols as usize;
        let owns_next = |cell: Cell| UnicodeWidthChar::width(cell.ch) == Some(2);
        for c in 0..cols {
            let orphan_spacer = self.cells[start + c].is_spacer()
                && (c == 0 || !owns_next(self.cells[start + c - 1]));
            let orphan_owner = owns_next(self.cells[start + c])
                && (c + 1 == cols || !self.cells[start + c + 1].is_spacer());
            if orphan_spacer || orphan_owner {
                self.cells[start + c] = Cell::default();
            }
        }
    }

    /// CSI L (IL): insert `n` blank rows at the cursor row, pushing the rows
    /// below it down. Rows pushed past the bottom are discarded rather than
    /// filed as scrollback: scrollback is what scrolled off the TOP of the
    /// screen, and an in-screen insert never reached the top — filing them
    /// would let a client scrolling back read rows the user never saw leave.
    pub fn insert_lines(&mut self, n: u16) {
        let Some((first, last)) = self.editable_rows_below_cursor() else {
            return;
        };
        let cols = self.cols as usize;
        let height = last - first + 1;
        let n = (n as usize).min(height);
        let region = &mut self.cells[first * cols..(last + 1) * cols];
        region.copy_within(..(height - n) * cols, n * cols);
        for cell in &mut region[..n * cols] {
            *cell = Cell::default();
        }
        self.dirty.extend(self.cursor_row..=self.scroll_region.1);
    }

    /// CSI M (DL): delete `n` rows at the cursor row, pulling the rows below
    /// it up and blanking the bottom. Same discard reasoning as
    /// [`Self::insert_lines`] — a deleted row is not history either.
    pub fn delete_lines(&mut self, n: u16) {
        let Some((first, last)) = self.editable_rows_below_cursor() else {
            return;
        };
        let cols = self.cols as usize;
        let height = last - first + 1;
        let n = (n as usize).min(height);
        let region = &mut self.cells[first * cols..(last + 1) * cols];
        region.copy_within(n * cols.., 0);
        let vacated = region.len() - n * cols;
        for cell in &mut region[vacated..] {
            *cell = Cell::default();
        }
        self.dirty.extend(self.cursor_row..=self.scroll_region.1);
    }

    /// The half-open row span IL/DL may shift: the cursor's row down to the
    /// scrolling region's bottom, INCLUSIVE, as 0-based indices.
    ///
    /// `None` when the cursor sits outside the region, and then both verbs do
    /// nothing — DEC's rule, and the one that keeps a pinned footer pinned.
    /// Without it a program could move below its own region and push the
    /// footer down through IL, having just been stopped from doing the same
    /// thing through a newline.
    fn editable_rows_below_cursor(&self) -> Option<(usize, usize)> {
        let (top, bottom) = self.scroll_region;
        if self.cursor_row < top || self.cursor_row > bottom {
            return None;
        }
        Some((self.cursor_row as usize, bottom as usize))
    }

    /// CSI J. 0 = cursor to end, 1 = start to cursor, anything else = all.
    ///
    /// **Screen-absolute, deliberately, even with a scrolling region set** --
    /// the one row-spanning verb here that does NOT read the region. `CSI J`
    /// erases the DISPLAY, and DEC defines its three modes against the
    /// screen: a region constrains scrolling, not erasure. Clipping this to
    /// the region would leave a header standing that the program believes it
    /// erased, and stale text a manifest can still match is worse than none
    /// at all — a wrong label costs more than a missing one.
    pub fn erase_in_display(&mut self, mode: u16) {
        let cur = self.idx(self.cursor_row, self.cursor_col);
        let len = self.cells.len();
        let (from, to) = match mode {
            0 => (cur, len),
            1 => (0, (cur + 1).min(len)),
            _ => (0, len),
        };
        self.clear_range(from, to);
    }

    /// CSI K. 0 = cursor to end of line, 1 = start of line to cursor,
    /// anything else = the whole line.
    pub fn erase_in_line(&mut self, mode: u16) {
        let start = self.idx(self.cursor_row, 0);
        let end = start + self.cols as usize;
        let cur = self.idx(self.cursor_row, self.cursor_col);
        let (from, to) = match mode {
            0 => (cur, end),
            1 => (start, (cur + 1).min(end)),
            _ => (start, end),
        };
        self.clear_range(from, to);
    }

    /// Blanks `[from, to)`, extending either edge by one cell when it falls
    /// inside a wide glyph's owner/spacer pair. Left unextended, clearing
    /// only a spacer strands its owner claiming a width-2 glyph with a
    /// blank neighbour instead of a spacer, and clearing only an owner
    /// orphans its spacer — the same corruption class `put`'s
    /// `repair_straddled_glyph` guards against, here for a range instead of
    /// a single cursor write. A spacer can never sit at column 0 (`put`
    /// always wraps before it would split a wide glyph across rows), so
    /// this extension never reaches across a row boundary.
    fn clear_range(&mut self, mut from: usize, mut to: usize) {
        let len = self.cells.len();
        if from >= to || from >= len {
            return;
        }
        to = to.min(len);
        if from > 0 && self.cells[from].is_spacer() {
            from -= 1;
        }
        if to < len && self.cells[to].is_spacer() {
            to += 1;
        }
        // A single choke point for both erase_in_display and erase_in_line:
        // the range can span multiple rows (e.g. "clear to end of screen"),
        // so every row it touches — not just the cursor's — is dirty.
        let cols = self.cols as usize;
        let row_start = (from / cols) as u16;
        let row_end = ((to - 1) / cols) as u16;
        self.dirty.extend(row_start..=row_end);
        for cell in &mut self.cells[from..to] {
            *cell = Cell::default();
        }
    }
}
