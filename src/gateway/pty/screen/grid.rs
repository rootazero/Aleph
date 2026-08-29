//! The character grid: cells, rows, cursor, and the scrollback ring.

use unicode_width::UnicodeWidthChar;

/// A single cell's colour. `Default` means "whatever the client's theme
/// says", which is why it is a variant rather than a concrete RGB — the
/// server does not know the client's palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Bitflags for the SGR attributes we render. Kept to one byte so a `Cell`
/// stays small — a 1000-line scrollback at 200 columns is 200k cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attrs(pub u8);

impl Attrs {
    pub const NONE: Self = Self(0);
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const REVERSE: Self = Self(1 << 3);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// One cell. `ch == '\0'` marks the right half of a double-width glyph: it
/// holds no character of its own but must not be overwritten independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', fg: Color::Default, bg: Color::Default, attrs: Attrs::NONE }
    }
}

impl Cell {
    /// The spacer that follows a double-width glyph.
    pub(crate) const SPACER: char = '\0';

    pub(crate) fn is_spacer(self) -> bool {
        self.ch == Self::SPACER
    }
}

/// The visible screen plus its cursor. Scrollback lands in [`Grid::scrollback`]
/// as rows fall off the top.
#[derive(Debug)]
pub struct Grid {
    rows: u16,
    cols: u16,
    cells: Vec<Cell>,
    cursor_row: u16,
    cursor_col: u16,
    scrollback: std::collections::VecDeque<Vec<Cell>>,
    scrollback_limit: usize,
}

impl Grid {
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        let (rows, cols) = (rows.max(1), cols.max(1));
        Self {
            rows,
            cols,
            cells: vec![Cell::default(); rows as usize * cols as usize],
            cursor_row: 0,
            cursor_col: 0,
            scrollback: std::collections::VecDeque::new(),
            scrollback_limit: 1000,
        }
    }

    #[must_use]
    pub const fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    #[must_use]
    pub const fn cursor(&self) -> (u16, u16) {
        (self.cursor_row, self.cursor_col)
    }

    fn idx(&self, row: u16, col: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    #[must_use]
    pub fn row_cells(&self, row: u16) -> &[Cell] {
        let start = self.idx(row.min(self.rows - 1), 0);
        &self.cells[start..start + self.cols as usize]
    }

    /// Row rendered as text, spacers dropped and trailing blanks trimmed.
    /// Test-facing; the wire uses [`Self::row_cells`].
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        let s: String = self
            .row_cells(row)
            .iter()
            .filter(|c| !c.is_spacer())
            .map(|c| c.ch)
            .collect();
        s.trim_end().to_string()
    }

    /// Write `c` at the cursor with `style`, advancing the cursor. Wraps to
    /// the next row at the right margin and scrolls at the bottom.
    pub fn put(&mut self, c: char, style: (Color, Color, Attrs)) {
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            return;
        }
        let w = width as u16;
        if self.cursor_col + w > self.cols {
            self.newline();
            self.cursor_col = 0;
        }
        let (fg, bg, attrs) = style;
        self.repair_straddled_glyph(w, fg, bg, attrs);
        let i = self.idx(self.cursor_row, self.cursor_col);
        self.cells[i] = Cell { ch: c, fg, bg, attrs };
        if w == 2 {
            let j = self.idx(self.cursor_row, self.cursor_col + 1);
            self.cells[j] = Cell { ch: Cell::SPACER, fg, bg, attrs };
        }
        self.cursor_col += w;
    }

    /// A write at the cursor can straddle an existing wide glyph on either
    /// side: land on a spacer whose owner sits one cell to the left, or
    /// overwrite an owner whose spacer sits just past the write. Left
    /// unrepaired, either case orphans a spacer cell — one with no owning
    /// wide glyph immediately to its left. [`Self::row_text`] filters
    /// spacers and would hide the corruption; [`Self::row_cells`], which is
    /// what the wire sends, would not.
    fn repair_straddled_glyph(&mut self, w: u16, fg: Color, bg: Color, attrs: Attrs) {
        let blank = Cell { ch: ' ', fg, bg, attrs };

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

    /// Absolute cursor move, clamped to the grid. Callers pass 0-based
    /// coordinates; the 1-based CSI convention is converted by the caller.
    pub fn goto(&mut self, row: u16, col: u16) {
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
    }

    /// Relative cursor move, clamped at every edge. Signed deltas because
    /// unsigned subtraction here panics in debug and wraps in release —
    /// the same byte behaving two ways in two profiles.
    pub fn move_cursor(&mut self, d_row: i32, d_col: i32) {
        let r = i64::from(self.cursor_row) + i64::from(d_row);
        let c = i64::from(self.cursor_col) + i64::from(d_col);
        self.cursor_row = r.clamp(0, i64::from(self.rows - 1)) as u16;
        self.cursor_col = c.clamp(0, i64::from(self.cols - 1)) as u16;
    }

    /// CSI J. 0 = cursor to end, 1 = start to cursor, anything else = all.
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
        for cell in &mut self.cells[from..to] {
            *cell = Cell::default();
        }
    }

    /// Move to the next row, scrolling the top row into scrollback when the
    /// cursor is already on the last row.
    pub fn newline(&mut self) {
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        } else {
            self.scroll_up();
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

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
    }

    /// Narrowing can cut a wide glyph's spacer off at the new right edge,
    /// leaving its owner alone in `next` with no spacer to follow it — the
    /// same corruption class [`Self::repair_straddled_glyph`] guards
    /// against for writes, here for the edge a resize can create instead of
    /// a cursor write. If the first column this narrowing drops was a
    /// spacer, the column it keeps right before it must be the spacer's
    /// owner; blank that owner rather than let a truncated half-glyph
    /// survive into the resized grid. `next` is still `self.cols` wide at
    /// this point (the field is not reassigned until after the copy loop),
    /// so `self.idx` remains valid for indexing into it via `new_cols`.
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

    fn scroll_up(&mut self) {
        let first: Vec<Cell> = self.row_cells(0).to_vec();
        if self.scrollback.len() >= self.scrollback_limit {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(first);
        self.cells.rotate_left(self.cols as usize);
        let start = self.idx(self.rows - 1, 0);
        for cell in &mut self.cells[start..] {
            *cell = Cell::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: (Color, Color, Attrs) = (Color::Default, Color::Default, Attrs::NONE);

    #[test]
    fn printing_advances_the_cursor_and_lands_in_the_row() {
        let mut g = Grid::new(3, 10);
        for c in "hello".chars() {
            g.put(c, PLAIN);
        }
        assert_eq!(g.row_text(0), "hello");
        assert_eq!(g.cursor(), (0, 5));
    }

    /// A CJK glyph occupies two columns. Getting this wrong is invisible in
    /// ASCII tests and then misaligns every table a user ever prints.
    #[test]
    fn wide_chars_take_two_columns_and_leave_a_spacer() {
        let mut g = Grid::new(2, 10);
        g.put('中', PLAIN);
        assert_eq!(g.cursor(), (0, 2), "a wide glyph advances the cursor by two");
        assert_eq!(g.row_text(0), "中", "the spacer cell must not surface as a char");
    }

    /// Writing past the last column wraps to the next row rather than
    /// silently dropping the character.
    #[test]
    fn printing_past_the_last_column_wraps() {
        let mut g = Grid::new(2, 3);
        for c in "abcd".chars() {
            g.put(c, PLAIN);
        }
        assert_eq!(g.row_text(0), "abc");
        assert_eq!(g.row_text(1), "d");
        assert_eq!(g.cursor(), (1, 1));
    }

    /// A wide glyph's owner and spacer are a pair; overwriting one without
    /// the other orphans the survivor. This is the concrete repro from
    /// review: print a wide glyph, return to column 0 without a newline (a
    /// bare CR — what a progress bar or spinner does), then print a narrow
    /// char there. That narrow write overwrites only the owner, so the old
    /// spacer at column 1 must be repaired rather than left dangling.
    /// Asserted through `row_cells`, not `row_text` — `row_text` filters
    /// spacers and would pass against the unrepaired bug, which is why the
    /// bug survived the original three tests.
    #[test]
    fn put_repairs_a_dangling_spacer_left_by_overwriting_its_owner() {
        let mut g = Grid::new(2, 10);
        g.put('中', PLAIN);
        g.carriage_return();
        g.put('a', PLAIN);

        let cells = g.row_cells(0);
        assert_eq!(cells[0].ch, 'a');
        assert!(!cells[1].is_spacer(), "column 1 must not be left as a dangling spacer");
    }

    /// The mirror direction: the write lands directly on an existing
    /// spacer, whose owner sits one cell to the left and must not survive
    /// without it. Only reachable today by placing the cursor directly
    /// (a future cursor-repositioning method, e.g. Task 4's `goto`, would
    /// do this through the public API) — simulated here via the private
    /// field, the same precondition that method's tests will exercise.
    #[test]
    fn put_repairs_a_dangling_owner_when_the_cursor_lands_on_its_spacer() {
        let mut g = Grid::new(2, 10);
        g.put('中', PLAIN); // columns 0-1
        g.put('中', PLAIN); // columns 2-3
        g.cursor_col = 3; // the second glyph's spacer; its owner is column 2
        g.put('x', PLAIN);

        let cells = g.row_cells(0);
        assert_eq!(cells[0].ch, '中', "the first glyph is untouched");
        assert!(cells[1].is_spacer(), "the first glyph keeps its own spacer");
        assert_eq!(cells[2].ch, ' ', "the orphaned owner must be blanked, not left wide with no spacer");
        assert_eq!(cells[3].ch, 'x');
    }

    /// The same scenario as above, this time reached through the public API
    /// the test just above could only simulate: Task 4's `goto` can land
    /// the cursor directly on a spacer, exactly the state a real program
    /// reaches by repositioning mid-glyph (e.g. `CSI 1;4H` over the second
    /// half of a CJK character). The private-field test documents the
    /// invariant directly; this one proves the repair is reachable the way
    /// real terminal output reaches it.
    #[test]
    fn goto_onto_a_spacer_then_put_repairs_the_owner() {
        let mut g = Grid::new(2, 10);
        g.put('中', PLAIN); // columns 0-1
        g.put('中', PLAIN); // columns 2-3
        g.goto(0, 3); // the second glyph's spacer; its owner is column 2
        g.put('x', PLAIN);

        let cells = g.row_cells(0);
        assert_eq!(cells[0].ch, '中', "the first glyph is untouched");
        assert!(cells[1].is_spacer(), "the first glyph keeps its own spacer");
        assert_eq!(cells[2].ch, ' ', "the orphaned owner must be blanked, not left wide with no spacer");
        assert_eq!(cells[3].ch, 'x');
    }

    /// `scroll_up` — ring eviction, `rotate_left`, and clearing the new
    /// last row — is the only nontrivial method in this file, and none of
    /// the tests above ever fill the last row, so it never runs. Fill past
    /// the bottom and check both what's still visible and what landed in
    /// scrollback.
    #[test]
    fn scrolling_past_the_last_row_evicts_the_top_row_into_scrollback() {
        let mut g = Grid::new(2, 5);
        for c in "row0!".chars() {
            g.put(c, PLAIN);
        }
        g.newline();
        g.carriage_return();
        for c in "row1!".chars() {
            g.put(c, PLAIN);
        }
        g.newline(); // cursor is already on the last row: this scrolls
        g.carriage_return();
        for c in "row2!".chars() {
            g.put(c, PLAIN);
        }

        assert_eq!(g.row_text(0), "row1!", "row0 scrolled off the top, row1 moved up");
        assert_eq!(g.row_text(1), "row2!");
        assert_eq!(g.scrollback.len(), 1, "exactly one row was evicted");
        let evicted: String = g.scrollback[0].iter().map(|c| c.ch).collect();
        assert_eq!(evicted, "row0!", "the evicted row is what fell off the top");
    }

    /// Narrowing must not strand a wide glyph's owner without its spacer at
    /// the new right edge -- the same corruption class
    /// `repair_straddled_glyph` guards against for writes. Asserted via
    /// `row_cells`, not `row_text`: `row_text` filters spacers and would
    /// hide a half-glyph surviving where its spacer used to be, which is
    /// exactly how the original spacer bug survived its first tests.
    #[test]
    fn narrowing_resize_does_not_strand_a_wide_glyph_at_the_new_edge() {
        let mut g = Grid::new(2, 10);
        g.put('a', PLAIN);
        g.put('中', PLAIN); // columns 1-2

        g.resize(2, 2);

        let cells = g.row_cells(0);
        assert_eq!(cells[0].ch, 'a', "the surviving column is untouched");
        assert_eq!(
            cells[1].ch, ' ',
            "the truncated wide glyph's owner must be blanked, not left as a half-rendered glyph"
        );
    }

    /// The mirror case: narrowing to a boundary that falls cleanly between
    /// glyphs (not through one) must leave the surviving glyph intact.
    #[test]
    fn narrowing_resize_leaves_a_glyph_that_fits_entirely_untouched() {
        let mut g = Grid::new(2, 10);
        g.put('中', PLAIN); // columns 0-1

        g.resize(2, 2);

        let cells = g.row_cells(0);
        assert_eq!(cells[0].ch, '中', "the glyph owner survives");
        assert!(cells[1].is_spacer(), "its spacer survives alongside it");
    }

    #[test]
    fn resize_clamps_a_cursor_that_would_now_be_out_of_bounds() {
        let mut g = Grid::new(5, 20);
        g.goto(4, 15);
        g.resize(2, 5);
        assert_eq!(g.cursor(), (1, 4), "cursor is clamped into the smaller grid");
    }

    #[test]
    fn resize_to_the_same_dimensions_is_a_cheap_no_op() {
        let mut g = Grid::new(3, 10);
        g.put('x', PLAIN);
        g.resize(3, 10);
        assert_eq!(g.row_text(0), "x", "content is untouched by a same-size resize");
    }
}
