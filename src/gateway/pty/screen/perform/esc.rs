//! Non-CSI escapes (`ESC <byte>`) and the C0 control table. Two faces of
//! "one byte decides", kept together because they share nothing with CSI's
//! parameter machinery.

use super::{Performer, SavedCursor};

impl Performer<'_> {
    /// The C0 table. `vte` routes control bytes here rather than to
    /// `print`, so an unclaimed byte is dropped by an arm that was never
    /// written -- which is what the census in `tests.rs` reads this
    /// function's source to prevent.
    pub(super) fn c0(&mut self, byte: u8) {
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
        match byte {
            // DECSC. Position and style travel together because that is
            // what the sequence means; saving only the position passes a
            // position test and then drops colour on every prompt that
            // brackets its output with 7/8.
            b'7' => {
                self.screen.saved_cursor = Some(SavedCursor {
                    pos: self.screen.grid.cursor(),
                    style: self.style(),
                });
            }
            // DECRC. With nothing saved this does nothing. DEC's spec homes
            // the cursor instead; nothing here needs that, and a stray
            // `ESC 8` that homed the cursor would move a screen the user is
            // watching, where doing nothing cannot.
            b'8' => {
                if let Some(saved) = self.screen.saved_cursor {
                    let (row, col) = saved.pos;
                    self.screen.grid.goto(row, col);
                    let (fg, bg, attrs) = saved.style;
                    self.screen.state.fg = fg;
                    self.screen.state.bg = bg;
                    self.screen.state.attrs = attrs;
                }
            }
            _ => {}
        }
    }
}
