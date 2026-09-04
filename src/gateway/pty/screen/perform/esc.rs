//! Non-CSI escapes (`ESC <byte>`) and the C0 control table. Two faces of
//! "one byte decides", kept together because they share nothing with CSI's
//! parameter machinery.

use super::Performer;

impl Performer<'_> {
    /// The C0 table. `vte` routes control bytes here rather than to
    /// `print`, so an unclaimed byte is dropped by an arm that was never
    /// written -- which is what the census in `tests.rs` reads this
    /// function's source to prevent.
    pub(super) fn c0(&mut self, byte: u8) {
        // A control byte is not a print, so it invalidates REP's candidate --
        // see `Grid::take_last_printed`. Unconditional, including on the
        // bytes no arm below claims: "the dispatcher ignored it" is not the
        // same as "it never arrived", and REP's contract is about what was
        // printed, not about what this table happens to handle.
        self.screen.grid.take_last_printed();
        match byte {
            // VT and FF move down a line like LF: what xterm does, and a
            // program that emits either means "next line", never "nothing".
            b'\n' | 0x0b | 0x0c => self.screen.grid.newline(),
            b'\r' => self.screen.grid.carriage_return(),
            0x08 => self.screen.grid.backspace(),
            0x09 => self.screen.grid.tab(),
            0x07 => self.screen.state.bell = true,
            _ => {}
        }
    }

    /// Non-CSI escapes. `vte`'s default for this method is a silent no-op,
    /// so before it existed every one of them was dropped by construction
    /// rather than by a decision — which is why `ESC 7`/`ESC 8` went
    /// missing even though prompt drawing leans on them.
    pub(super) fn esc(&mut self, intermediates: &[u8], byte: u8) {
        // Intermediates are load-bearing, not decoration: `ESC # 8` is
        // DECALN (fill the screen with `E`), and matching on the final byte
        // alone would run a screen-alignment test as a cursor restore.
        if !intermediates.is_empty() {
            return;
        }
        // Same reasoning as the C0 table: an escape between the character
        // and the REP means there is no candidate any more.
        self.screen.grid.take_last_printed();
        match byte {
            // DECSC. Position and style travel together because that is
            // what the sequence means; saving only the position passes a
            // position test and then drops colour on every prompt that
            // brackets its output with 7/8.
            b'7' => self.save_cursor(),
            // DECRC. With nothing saved this does nothing. DEC's spec homes
            // the cursor instead; nothing here needs that, and a stray
            // `ESC 8` that homed the cursor would move a screen the user is
            // watching, where doing nothing cannot.
            b'8' => self.restore_cursor(),
            // RIS. The full reset, including the title; scrollback survives
            // on purpose (`Grid::reset` says why).
            b'c' => self.full_reset(),
            // IND: down one row, same column. NOT a newline -- a version
            // that returned to column zero overlays the next run of text
            // onto the start of the line, producing a plausible-looking
            // mixture of two real lines that a manifest regex can match.
            b'D' => self.screen.grid.newline(),
            // NEL: down one row AND back to column zero.
            b'E' => {
                self.screen.grid.carriage_return();
                self.screen.grid.newline();
            }
            // RI: up one row, scrolling the region DOWN at its top. The
            // direction IND does not cover, and the one a pager uses to
            // reveal the line above.
            b'M' => self.screen.grid.reverse_index(),
            _ => {}
        }
    }
}
