//! The character grid: cells, rows, cursor, and the scrollback ring.
//!
//! Split by responsibility, one file per kind of change to the cells:
//! [`write`] (printing a glyph), [`edit`] (the erase and insert/delete
//! verbs), [`scroll`] (the scrolling region and everything that shifts whole
//! rows), and [`resize`]. This file keeps the types, the state, and the
//! moves that touch only the cursor.
//!
//! Every helper stays private to the file that calls it. That works because
//! a child module can see its parent's private items but not the reverse:
//! the shared primitives (`idx`, the fields) live here, and each sub-module's
//! own repair helpers stay where their only callers are.


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
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::NONE,
        }
    }
}

impl Cell {
    /// The spacer that follows a double-width glyph.
    pub(crate) const SPACER: char = '\0';

    pub(crate) fn is_spacer(self) -> bool {
        self.ch == Self::SPACER
    }
}

/// The interval between tab stops. Fixed: HTS/TBC (programmable stops) are
/// not modelled, so every stop is a multiple of this.
const TAB_WIDTH: u16 = 8;

/// The visible screen plus its cursor. Scrollback lands in [`Grid::scrollback`]
/// as rows fall off the top.
#[derive(Debug)]
pub struct Grid {
    rows: u16,
    cols: u16,
    cells: Vec<Cell>,
    cursor_row: u16,
    /// Invariant: `cursor_col <= cols`, note the `<=`. `put` leaves it
    /// exactly at `cols` after filling a row, and that is the model's only
    /// representation of "a wrap is owed" — there is no separate flag. Every
    /// method that does arithmetic on it (`tab`, `backspace`, `erase_chars`,
    /// `delete_chars`, `insert_chars`) has to stay correct at that one
    /// out-of-range-looking value, so subtract from `cols` rather than
    /// assuming `cursor_col < cols`.
    cursor_col: u16,
    scrollback: std::collections::VecDeque<Vec<Cell>>,
    scrollback_limit: usize,
    /// Rows changed since the last [`Self::take_dirty`]. A row lands here on
    /// every write that touches it, so a client that missed no frames can be
    /// sent only what changed instead of the whole screen.
    dirty: std::collections::BTreeSet<u16>,
    /// The scrolling region (DECSTBM), 0-based and INCLUSIVE at both ends,
    /// defaulting to the whole screen. Every verb that shifts rows reads it;
    /// `erase_in_display` deliberately does not (see its doc comment).
    scroll_region: (u16, u16),
    /// DECOM (`CSI ?6 h/l`). On, a row addressed by CUP or VPA is measured
    /// from the region's top and clamped at its bottom. Off before a region
    /// exists this means nothing, which is why it ships with DECSTBM rather
    /// than after it.
    origin_mode: bool,
    /// DECAWM (`CSI ?7 h/l`), on by default as the standard requires. Off,
    /// the right margin absorbs writes instead of wrapping -- the standard
    /// trick for painting a full-width status line without scrolling.
    autowrap: bool,
    /// The last character [`Self::put`] actually placed, for REP (`CSI Ps b`)
    /// to repeat. `None` means "no candidate", which is what a control byte
    /// or an intervening escape leaves behind: REP repeats the last PRINTED
    /// character, so anything that is not a print invalidates it. The
    /// invalidation is the dispatcher's job, not this field's -- `put` is
    /// also how a wrap and a repeat write, and clearing it from inside any
    /// grid method would erase the candidate REP is about to read.
    last_printed: Option<char>,
}

mod edit;
mod resize;
mod scroll;
mod write;

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
            // Starts empty, not full: a client that just attached gets a
            // full sync from `Screen::full_patch`, which reads every row
            // directly and does not consult this set at all. Seeding it
            // with every row here would only cause `take_patch`'s very
            // first call to resend rows nothing ever wrote to.
            dirty: std::collections::BTreeSet::new(),
            scroll_region: (0, rows - 1),
            origin_mode: false,
            autowrap: true,
            last_printed: None,
        }
    }

    /// DECSTBM. The arguments arrive 1-based and inclusive; `0` (or an
    /// omitted parameter) means the screen's own edge.
    ///
    /// An impossible region -- a top not strictly above the bottom -- resets
    /// to the full screen rather than being nudged into one the program did
    /// not ask for. DEC specifies the reset, and a silently-repaired region
    /// would pin rows the program expects to scroll, which is the failure
    /// this whole feature exists to remove, arriving through the fix.
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let last = self.rows - 1;
        let top = top.max(1) - 1;
        let bottom = if bottom == 0 {
            last
        } else {
            (bottom - 1).min(last)
        };
        self.scroll_region = if top < bottom { (top, bottom) } else { (0, last) };
    }

    /// DECOM. Setting or resetting it homes the cursor, as DEC requires:
    /// without that the cursor can be left outside the region it has just
    /// started addressing relative to.
    pub fn set_origin_mode(&mut self, on: bool) {
        self.origin_mode = on;
        self.cursor_row = if on { self.scroll_region.0 } else { 0 };
        self.cursor_col = 0;
    }

    /// A row from CUP/HVP/VPA, resolved through DECOM.
    ///
    /// Separate from the absolute [`Self::set_cursor`] on purpose: DECRC
    /// restores a position that was ALREADY absolute when it was saved, and
    /// putting it through this offset would add the region's top a second
    /// time -- a cursor that drifts one region deeper on every save/restore
    /// pair, which prompt drawing performs constantly.
    fn resolve_row(&self, row: u16) -> u16 {
        let (top, bottom) = self.scroll_region;
        if self.origin_mode {
            top.saturating_add(row).min(bottom)
        } else {
            row.min(self.rows - 1)
        }
    }

    /// DECAWM. See [`Self::put`] for what "off" costs the cursor invariant.
    pub fn set_autowrap(&mut self, on: bool) {
        self.autowrap = on;
    }

    /// Takes REP's repeat candidate, leaving none behind.
    ///
    /// Taking rather than reading is what makes "the last PRINTED character"
    /// true without every CSI arm remembering to invalidate: the dispatcher
    /// takes it once per sequence, and only REP's arm does anything with the
    /// value. [`Self::put`] sets it again, so a run of REPs keeps working.
    pub fn take_last_printed(&mut self) -> Option<char> {
        self.last_printed.take()
    }

    /// The mode half of a reset: what DECSTR (`CSI ! p`) shares with RIS.
    /// Cells, cursor, scrollback and title are NOT touched here -- a soft
    /// reset leaves all four alone, and [`Self::reset`] does the rest.
    pub fn reset_modes(&mut self) {
        self.autowrap = true;
        self.origin_mode = false;
        self.scroll_region = (0, self.rows - 1);
        self.last_printed = None;
    }

    /// RIS (`ESC c`) on the grid: blank every cell, home the cursor, put the
    /// modes back.
    ///
    /// **Scrollback deliberately survives.** xterm's RIS clears it; Aleph's
    /// does not, because the one reader that decides anything --
    /// `Screen::visible_text`, which feeds agent detection -- never looks at
    /// scrollback, so clearing it would take away what the user scrolled
    /// back to and give no consumer anything. Recorded here rather than only
    /// in the test, because this is the file someone changes.
    pub fn reset(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::default();
        }
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.reset_modes();
        self.mark_all_dirty();
    }

    /// Takes and clears the dirty set. The next call sees only rows changed
    /// since this one.
    pub(crate) fn take_dirty(&mut self) -> std::collections::BTreeSet<u16> {
        std::mem::take(&mut self.dirty)
    }

    /// Marks every row dirty — used when the grid itself changed shape
    /// (resize) or shifted wholesale (scroll), where a per-row diff would
    /// be as expensive to compute as just resending everything.
    pub(crate) fn mark_all_dirty(&mut self) {
        self.dirty.extend(0..self.rows);
    }

    #[must_use]
    pub const fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Current visible row count. Tracks `resize` — never a cached magic
    /// number — so callers that iterate `0..rows()` stay in bounds across a
    /// live terminal resize.
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    #[must_use]
    pub const fn cursor(&self) -> (u16, u16) {
        (self.cursor_row, self.cursor_col)
    }

    /// Rows CURRENTLY retained in scrollback -- not a running total of rows
    /// ever evicted. The ring is capped at `scrollback_limit`, and once it
    /// saturates each new eviction drops the oldest row, so this value stops
    /// growing while the cumulative count keeps climbing. Reported in
    /// `pty.attach` so a client knows how far back it can scroll before the
    /// server has nothing left to give it, which is exactly the retained
    /// count and never the cumulative one: a client that treated this as
    /// "rows evicted so far" would compute scrollback offsets against rows
    /// the server discarded long ago.
    #[must_use]
    pub fn scrollback_len(&self) -> u32 {
        self.scrollback.len() as u32
    }

    /// Override the scrollback ceiling. Called at spawn from
    /// `[policies.terminal] scrollback_lines`; without this the field would
    /// be settable and inert. Shrinking the ceiling below the current
    /// retained count evicts the oldest rows immediately, same as
    /// `scroll_up`'s own eviction when the ring is full.
    pub fn set_scrollback_limit(&mut self, lines: usize) {
        self.scrollback_limit = lines.max(1);
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }
    }

    /// The scrollback ceiling currently in effect.
    #[must_use]
    pub const fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
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

    /// CUP / HVP. Callers pass 0-based coordinates; the 1-based CSI
    /// convention is converted by the caller. The row goes through DECOM
    /// (see [`Self::resolve_row`]); the column does not, because DECOM has
    /// no left/right margins to be relative to.
    pub fn goto(&mut self, row: u16, col: u16) {
        self.cursor_row = self.resolve_row(row);
        self.cursor_col = col.min(self.cols - 1);
    }

    /// Absolute cursor move, ignoring DECOM. For restores of a position that
    /// was absolute when it was captured -- DECRC and private mode 1048.
    pub fn set_cursor(&mut self, row: u16, col: u16) {
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

    /// CSI G (CHA): absolute column, row unchanged. Every zsh prompt redraw
    /// starts here, which is why its absence made one keystroke draw three
    /// characters — the redraw could not get back to the column it began at.
    pub fn goto_col(&mut self, col: u16) {
        self.cursor_col = col.min(self.cols - 1);
    }

    /// HT (0x09): move to the next tab stop.
    ///
    /// A MOVE, not a write of spaces. The two are indistinguishable on a
    /// fresh row and differ the moment a tab crosses text that is already
    /// there — which is what a shell does every time it redraws a line, so
    /// writing spaces here would quietly erase the row it moved across.
    ///
    /// A stop can land on a wide glyph's spacer. That is left alone
    /// deliberately: "the cursor is sitting on a spacer" is a question
    /// [`Self::put`]'s `repair_straddled_glyph` already answers, and
    /// answering it a second way here would be a second source of truth for
    /// it. HT therefore ends at [`Self::goto_col`], exactly like CHA.
    pub fn tab(&mut self) {
        // Saturating because `cursor_col` can be `cols - 1` on a grid as
        // wide as `u16::MAX`, where the next stop does not fit — the same
        // "panics in debug, wraps in release" arithmetic `move_cursor`
        // avoids. `goto_col` clamps to the row either way.
        let next = (self.cursor_col / TAB_WIDTH)
            .saturating_add(1)
            .saturating_mul(TAB_WIDTH);
        self.goto_col(next);
    }

    /// BS (0x08): one column left, and nothing else.
    ///
    /// It does not erase. Erasing is the application's call — a shell rubs
    /// a character out by sending `\b \b`, and a BS that erased on its own
    /// would make that sequence eat two. It does not wrap onto the previous
    /// row either; that is `reverse-wrap` mode, which is not modelled.
    pub fn backspace(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    /// CSI d (VPA): row only, column unchanged — CHA's twin. DECOM-relative
    /// for the same reason CUP's row is.
    pub fn goto_row(&mut self, row: u16) {
        self.cursor_row = self.resolve_row(row);
    }

    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }
}

#[cfg(test)]
mod tests;
