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
        let i = self.idx(self.cursor_row, self.cursor_col);
        self.cells[i] = Cell { ch: c, fg, bg, attrs };
        if w == 2 {
            let j = self.idx(self.cursor_row, self.cursor_col + 1);
            self.cells[j] = Cell { ch: Cell::SPACER, fg, bg, attrs };
        }
        self.cursor_col += w;
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

    fn scroll_up(&mut self) {
        let first: Vec<Cell> = self.row_cells(0).to_vec();
        if self.scrollback.len() == self.scrollback_limit {
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
}
