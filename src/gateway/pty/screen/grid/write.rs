//! Printing a glyph at the cursor -- the one path that turns a character
//! into cells, and the wide-glyph repair only it needs.

use unicode_width::UnicodeWidthChar;

use super::{Attrs, Cell, Color, Grid};

impl Grid {
    /// Write `c` at the cursor with `style`, advancing the cursor. Wraps to
    /// the next row at the right margin and scrolls at the bottom.
    pub fn put(&mut self, c: char, style: (Color, Color, Attrs)) {
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            return;
        }
        let w = width as u16;
        // A glyph wider than the whole screen has nowhere to go. Returning
        // beats the alternatives: wrapping would loop, and writing it would
        // index the spacer past the end of the row. Reachable only on a
        // one-column grid, which `Grid::new`'s `max(1)` permits.
        if w > self.cols {
            return;
        }
        // Widened to `u32` before the compare: `cursor_col` may legitimately
        // sit AT `cols` (the invariant is `cursor_col <= cols` — parking there
        // means "a wrap is owed"), so on a maximally wide grid `cursor_col + w`
        // overflows `u16` — panic in debug, wrap in release, and a wrapped sum
        // is `< cols`, so the wrap check says "no wrap needed" and `idx` then
        // indexes past the row. Today `handlers::pty::MAX_TERMINAL_DIMENSION`
        // keeps `cols` three orders of magnitude below that, but the arithmetic
        // must not depend on a bound enforced in another module — still less on
        // the older accident that the allocation for such a grid aborted the
        // process first.
        if u32::from(self.cursor_col) + u32::from(w) > u32::from(self.cols) {
            if self.autowrap {
                self.newline();
                self.cursor_col = 0;
            } else {
                // DECAWM off: the glyph lands at the right margin, on top of
                // whatever was there. No newline, so no phantom scroll on the
                // bottom row -- the failure that offsets every later frame by
                // one row against what the program believes it painted.
                self.cursor_col = self.cols - w;
            }
        }
        let (fg, bg, attrs) = style;
        self.repair_straddled_glyph(w, fg, bg, attrs);
        let i = self.idx(self.cursor_row, self.cursor_col);
        self.cells[i] = Cell {
            ch: c,
            fg,
            bg,
            attrs,
        };
        if w == 2 {
            let j = self.idx(self.cursor_row, self.cursor_col + 1);
            self.cells[j] = Cell {
                ch: Cell::SPACER,
                fg,
                bg,
                attrs,
            };
        }
        // repair_straddled_glyph above only ever touches cursor_row too, so
        // one insert covers the whole write.
        self.dirty.insert(self.cursor_row);
        self.last_printed = Some(c);
        self.cursor_col = if self.autowrap {
            self.cursor_col + w
        } else {
            // `cursor_col == cols` is this model's ONLY representation of "a
            // wrap is owed" (see the field's invariant). With DECAWM off no
            // wrap is ever owed, so that value must never be entered --
            // parking there would make the next `put` take the branch above
            // and wrap after all, which is the bug DECAWM was turned off to
            // avoid.
            (self.cursor_col + w).min(self.cols - 1)
        };
    }

    /// A write at the cursor can straddle an existing wide glyph on either
    /// side: land on a spacer whose owner sits one cell to the left, or
    /// overwrite an owner whose spacer sits just past the write. Left
    /// unrepaired, either case orphans a spacer cell — one with no owning
    /// wide glyph immediately to its left. [`Self::row_text`] filters
    /// spacers, so a test written against it cannot see this at all;
    /// [`Self::row_cells`] can. (The wire drops spacers too — see
    /// [`Self::repair_row_pairs`] for what each kind of orphan looks like on
    /// the client — so `row_cells` is a test's window, not the wire's.)
    fn repair_straddled_glyph(&mut self, w: u16, fg: Color, bg: Color, attrs: Attrs) {
        let blank = Cell {
            ch: ' ',
            fg,
            bg,
            attrs,
        };

        if self.cursor_col > 0 {
            let here = self.idx(self.cursor_row, self.cursor_col);
            if self.cells[here].is_spacer() {
                let owner = self.idx(self.cursor_row, self.cursor_col - 1);
                self.cells[owner] = blank;
            }
        }

        let after = self.cursor_col + w;
        if after < self.cols {
            let after_idx = self.idx(self.cursor_row, after);
            if self.cells[after_idx].is_spacer() {
                self.cells[after_idx] = blank;
            }
        }
    }
}
