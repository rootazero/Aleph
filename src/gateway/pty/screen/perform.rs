//! `vte::Perform` implementation — turns the PTY byte stream into grid writes.

use super::grid::{Attrs, Color, Grid};

/// The parser plus the state it mutates. `Parser` is retained across `feed`
/// calls because escape sequences straddle read boundaries: an OSC title can
/// arrive in two chunks, and a parser rebuilt per read would lose the tail.
pub struct Screen {
    pub grid: Grid,
    /// The saved primary screen while the alternate screen is active.
    saved: Option<Grid>,
    parser: vte::Parser,
    state: ScreenState,
    /// Title as of the last `take_patch`, so an unchanged title is not
    /// reshipped on every frame.
    last_sent_title: Option<String>,
    /// Alt-screen flag as of the last `take_patch`, same reasoning.
    last_sent_alt: Option<bool>,
    /// What `ESC 7` (DECSC) stashed, for `ESC 8` to put back. One slot, not
    /// one per screen buffer: DECSC/DECRC in a shell prompt are always
    /// paired inside one buffer, and a second slot would have no producer.
    saved_cursor: Option<SavedCursor>,
}

/// DECSC's slot. Position and style are one value because `ESC 7` saves
/// both — splitting them invites a restore that returns the column and
/// silently drops the colour.
#[derive(Clone, Copy)]
struct SavedCursor {
    pos: (u16, u16),
    style: (Color, Color, Attrs),
}

#[derive(Default)]
struct ScreenState {
    fg: Color,
    bg: Color,
    attrs: Attrs,
    title: Option<String>,
    bell: bool,
}

impl Screen {
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            grid: Grid::new(rows, cols),
            saved: None,
            parser: vte::Parser::new(),
            state: ScreenState::default(),
            last_sent_title: None,
            last_sent_alt: None,
            saved_cursor: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut parser = std::mem::take(&mut self.parser);
        // `&mut *self` reborrows rather than moves, so `self` is usable
        // again below once `performer`'s borrow ends (its last use is the
        // `advance` call). Performer holding the whole `Screen`, not a
        // split grid/state borrow, is what lets `csi_dispatch` swap
        // `screen.grid` inline the instant `?1049h`/`?1049l` is parsed --
        // see `Performer::toggle_alt_screen` for why that has to happen
        // there and not after this function returns.
        let mut performer = Performer { screen: &mut *self };
        parser.advance(&mut performer, bytes);
        self.parser = parser;
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.state.title.as_deref()
    }

    /// Reads and clears the bell flag — a bell is an edge, not a level.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.state.bell)
    }

    /// True while the alternate screen buffer (`\e[?1049h`) is active —
    /// e.g. while `vim`, `htop`, or a pager is running in the shell.
    #[must_use]
    pub const fn alt_screen(&self) -> bool {
        self.saved.is_some()
    }

    /// Resize the visible grid and, if the alternate screen is active, the
    /// saved primary underneath it too — so a resize made while inside e.g.
    /// `vim` is not silently lost, and the primary comes back at the right
    /// dimensions once the program exits and restores it.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.grid.resize(rows, cols);
        if let Some(saved) = &mut self.saved {
            saved.resize(rows, cols);
        }
        // Belt and suspenders: `Grid::resize` already marks everything dirty
        // when the dimensions actually change, but a resize call is also the
        // moment a client's viewport genuinely changed, so force a full
        // repaint even on the (same rows, same cols) no-op path.
        self.grid.mark_all_dirty();
    }

    /// Override the scrollback ceiling on the visible grid and, if the
    /// alternate screen is active, the saved primary underneath it too —
    /// same reasoning as `resize`: a program running when the config is
    /// patched should not come back to a primary grid with a stale ceiling
    /// once it exits and restores.
    pub fn set_scrollback_limit(&mut self, lines: usize) {
        self.grid.set_scrollback_limit(lines);
        if let Some(saved) = &mut self.saved {
            saved.set_scrollback_limit(lines);
        }
    }

    /// The scrollback ceiling currently in effect on the visible grid.
    #[must_use]
    pub fn scrollback_limit(&self) -> usize {
        self.grid.scrollback_limit()
    }

    /// The diff since the last call, or `None` when nothing changed. `None`
    /// is what makes a quiet terminal free: the flush task publishes
    /// nothing.
    pub fn take_patch(&mut self) -> Option<super::diff::ScreenPatch> {
        let dirty = self.grid.take_dirty();
        let title_changed = self.state.title != self.last_sent_title;
        let alt = self.alt_screen();
        let alt_changed = Some(alt) != self.last_sent_alt;
        let bell = self.take_bell();

        let patch = super::diff::ScreenPatch {
            rows: super::diff::patch_rows(&self.grid, dirty),
            cursor: Some(self.grid.cursor()),
            alt_screen: alt_changed.then_some(alt),
            title: title_changed.then(|| self.state.title.clone()).flatten(),
            bell,
        };
        // Cursor is always present above, so emptiness is decided on the
        // fields that actually carry news.
        if patch.rows.is_empty() && !title_changed && !alt_changed && !bell {
            return None;
        }
        self.last_sent_title.clone_from(&self.state.title);
        self.last_sent_alt = Some(alt);
        Some(patch)
    }

    /// Every row, for `pty.attach`. Does not consume the dirty set — an
    /// attach must not swallow a diff a live client is still waiting for.
    #[must_use]
    pub fn full_patch(&self) -> super::diff::ScreenPatch {
        let (rows, _) = self.grid.dims();
        super::diff::ScreenPatch {
            rows: super::diff::patch_rows(&self.grid, 0..rows),
            cursor: Some(self.grid.cursor()),
            alt_screen: Some(self.alt_screen()),
            title: self.state.title.clone(),
            bell: false,
        }
    }
}

/// Holds the whole `Screen`, not split `grid`/`state` borrows: `csi_dispatch`
/// needs to swap `screen.grid` itself (entering/exiting the alternate
/// screen) at the exact point `?1049h`/`?1049l` is parsed, and a split
/// borrow of just `grid` can't be replaced wholesale -- see
/// `Performer::toggle_alt_screen`.
struct Performer<'a> {
    screen: &'a mut Screen,
}

impl Performer<'_> {
    fn style(&self) -> (Color, Color, Attrs) {
        (
            self.screen.state.fg,
            self.screen.state.bg,
            self.screen.state.attrs,
        )
    }

    /// SGR. Consumes the parameter list because 38/48 take trailing
    /// arguments, so this cannot be a per-parameter loop.
    fn sgr(&mut self, params: &[u16]) {
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.screen.state.fg = Color::Default;
                    self.screen.state.bg = Color::Default;
                    self.screen.state.attrs = Attrs::NONE;
                }
                1 => self.screen.state.attrs.insert(Attrs::BOLD),
                3 => self.screen.state.attrs.insert(Attrs::ITALIC),
                4 => self.screen.state.attrs.insert(Attrs::UNDERLINE),
                7 => self.screen.state.attrs.insert(Attrs::REVERSE),
                22 => self.screen.state.attrs.remove(Attrs::BOLD),
                23 => self.screen.state.attrs.remove(Attrs::ITALIC),
                24 => self.screen.state.attrs.remove(Attrs::UNDERLINE),
                27 => self.screen.state.attrs.remove(Attrs::REVERSE),
                30..=37 => self.screen.state.fg = Color::Indexed((params[i] - 30) as u8),
                39 => self.screen.state.fg = Color::Default,
                40..=47 => self.screen.state.bg = Color::Indexed((params[i] - 40) as u8),
                49 => self.screen.state.bg = Color::Default,
                90..=97 => self.screen.state.fg = Color::Indexed((params[i] - 90 + 8) as u8),
                100..=107 => self.screen.state.bg = Color::Indexed((params[i] - 100 + 8) as u8),
                38 | 48 => {
                    let is_fg = params[i] == 38;
                    // 38;5;N (indexed) or 38;2;R;G;B (truecolour). A malformed
                    // run is skipped rather than mis-parsed into the next
                    // parameter, which would recolour unrelated text.
                    match params.get(i + 1) {
                        Some(5) => {
                            if let Some(&n) = params.get(i + 2) {
                                let c = Color::Indexed(n as u8);
                                if is_fg {
                                    self.screen.state.fg = c
                                } else {
                                    self.screen.state.bg = c
                                }
                            }
                            i += 2;
                        }
                        Some(2) => {
                            if let (Some(&r), Some(&g), Some(&b)) =
                                (params.get(i + 2), params.get(i + 3), params.get(i + 4))
                            {
                                let c = Color::Rgb(r as u8, g as u8, b as u8);
                                if is_fg {
                                    self.screen.state.fg = c
                                } else {
                                    self.screen.state.bg = c
                                }
                            }
                            i += 4;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Enter (`enter == true`) or exit the alternate screen. Called from
    /// `csi_dispatch` at the exact point `?1049h`/`?1049l` is parsed -- not
    /// deferred until `feed` finishes the chunk. Deferring it was the
    /// original design; it passed the round-trip test because that test
    /// feeds the escape and the following bytes in separate `feed()` calls,
    /// but broke the common case of a program (vim, less) writing the enter
    /// escape and its first frame in ONE chunk: those bytes would print
    /// onto the PRIMARY grid before the deferred swap ever ran, and the
    /// swap would then stash that now-polluted primary into `saved`.
    fn toggle_alt_screen(&mut self, enter: bool) {
        let swapped = if enter {
            // A nested `?1049h` while already on the alt screen is a
            // no-op: clobbering `saved` here would replace the real
            // primary with the alt screen's own content, losing the
            // user's shell scrollback for good the next time they exit.
            // No grid becomes current here, so nothing needs marking dirty.
            if self.screen.saved.is_none() {
                let (rows, cols) = self.screen.grid.dims();
                let primary = std::mem::replace(&mut self.screen.grid, Grid::new(rows, cols));
                self.screen.saved = Some(primary);
                true
            } else {
                false
            }
        } else if let Some(primary) = self.screen.saved.take() {
            self.screen.grid = primary;
            true
        } else {
            false
        };
        if swapped {
            // Whichever grid just became current is entirely new to an
            // already-attached client. The fresh alt grid starts with an
            // EMPTY dirty set by design (`Grid::new`'s doc comment: a
            // client that just attached gets a full sync from
            // `full_patch`, which reads every row directly and ignores
            // the dirty set) -- but a mid-session swap is the opposite
            // case, a client that is already attached and only ever
            // reads `take_patch`. And the restored primary's dirty set is
            // whatever was stashed on it when it was last current, which
            // is stale. Without this, `take_patch` reports nothing
            // changed in both directions while the entire visible screen
            // was just replaced: entering leaves the Panel rendering the
            // shell screen while vim runs, exiting leaves it rendering
            // vim's last frame after the user quits.
            self.screen.grid.mark_all_dirty();
        }
    }
}

impl vte::Perform for Performer<'_> {
    fn print(&mut self, c: char) {
        let style = self.style();
        self.screen.grid.put(c, style);
    }

    fn execute(&mut self, byte: u8) {
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
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
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

    fn csi_dispatch(&mut self, params: &vte::Params, inter: &[u8], _ignore: bool, action: char) {
        // Flatten sub-parameters: only SGR's 38/48 use them, and it reads the
        // colon form (38:2:r:g:b) identically to the semicolon form.
        let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
        if action == 'm' {
            let effective: &[u16] = if flat.is_empty() { &[0] } else { &flat };
            self.sgr(effective);
        }

        // A parameter of `0` means "use the default" for every arm handled
        // here (never a literal zero), same as an omitted parameter.
        let p = |n: usize, default: u16| -> u16 {
            flat.get(n).copied().filter(|v| *v != 0).unwrap_or(default)
        };
        match action {
            // CUP / HVP: 1-based on the wire, 0-based in the grid.
            'H' | 'f' => self.screen.grid.goto(p(0, 1) - 1, p(1, 1) - 1),
            'A' => self.screen.grid.move_cursor(-i32::from(p(0, 1)), 0),
            'B' => self.screen.grid.move_cursor(i32::from(p(0, 1)), 0),
            'C' => self.screen.grid.move_cursor(0, i32::from(p(0, 1))),
            'D' => self.screen.grid.move_cursor(0, -i32::from(p(0, 1))),
            // CHA / VPA: absolute column and row, 1-based on the wire like
            // CUP above. A shell's line editor uses these and the five
            // below on every redraw; `p`'s "0 means default" is right for
            // all of them, unlike `J`/`K` whose 0 is a real mode value.
            'G' => self.screen.grid.goto_col(p(0, 1) - 1),
            'd' => self.screen.grid.goto_row(p(0, 1) - 1),
            'X' => self.screen.grid.erase_chars(p(0, 1)),
            'P' => self.screen.grid.delete_chars(p(0, 1)),
            '@' => self.screen.grid.insert_chars(p(0, 1)),
            'L' => self.screen.grid.insert_lines(p(0, 1)),
            'M' => self.screen.grid.delete_lines(p(0, 1)),
            'J' => self
                .screen
                .grid
                .erase_in_display(flat.first().copied().unwrap_or(0)),
            'K' => self
                .screen
                .grid
                .erase_in_line(flat.first().copied().unwrap_or(0)),
            // `\e[?1049h` / `\e[?1049l`: enter/exit the alternate screen.
            // `?` only ever arrives via `intermediates`, never `params` — a
            // guard on `action` alone would also swallow the private-mode-less
            // `h`/`l` sequences (unused here, but real DEC private modes like
            // `?25` cursor-visibility share these final bytes). Applied
            // inline, not deferred -- see `Performer::toggle_alt_screen`.
            'h' | 'l' if inter == b"?" && flat.first() == Some(&1049) => {
                self.toggle_alt_screen(action == 'h');
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 = icon + title, OSC 2 = title.
        let Some(kind) = params.first() else { return };
        if matches!(*kind, b"0" | b"2") {
            if let Some(raw) = params.get(1) {
                self.screen.state.title = Some(String::from_utf8_lossy(raw).into_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::pty::screen::grid::{Attrs, Cell, Color};

    #[test]
    fn plain_text_and_newlines_land_on_the_grid() {
        let mut s = Screen::new(4, 20);
        s.feed(b"one\r\ntwo\r\n");
        assert_eq!(s.grid.row_text(0), "one");
        assert_eq!(s.grid.row_text(1), "two");
    }

    #[test]
    fn sgr_sets_and_resets_colour_and_bold() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[1;31mR\x1b[0mp");
        let row = s.grid.row_cells(0);
        assert_eq!(row[0].fg, Color::Indexed(1), "SGR 31 is indexed red");
        assert!(row[0].attrs.contains(Attrs::BOLD), "SGR 1 is bold");
        assert_eq!(row[1].fg, Color::Default, "SGR 0 resets");
        assert!(!row[1].attrs.contains(Attrs::BOLD));
    }

    #[test]
    fn sgr_38_2_sets_truecolour() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[38;2;10;20;30mX");
        assert_eq!(s.grid.row_cells(0)[0].fg, Color::Rgb(10, 20, 30));
    }

    /// OSC 0/2 is how a shell renames its tab. It reaches the client as the
    /// tab label, so it is protocol, not decoration.
    #[test]
    fn osc_zero_sets_the_title() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]0;my-title\x07");
        assert_eq!(s.title(), Some("my-title"));
    }

    /// A title arriving in pieces across two reads must not be truncated.
    #[test]
    fn a_split_osc_sequence_still_yields_the_whole_title() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]0;my-");
        s.feed(b"title\x07");
        assert_eq!(s.title(), Some("my-title"));
    }

    /// SGR params can arrive as separate iterator items ("38;2;r;g;b") or as
    /// subparameters within one item ("38:2:r:g:b"). Both must resolve to
    /// the same colour — the colon form is common from tmux and some
    /// terminfo-driven output.
    #[test]
    fn sgr_38_2_colon_form_sets_truecolour() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[38:2:10:20:30mX");
        assert_eq!(s.grid.row_cells(0)[0].fg, Color::Rgb(10, 20, 30));
    }

    /// A bell (BEL, 0x07 outside an OSC) is an edge the client polls for,
    /// not a persistent flag — reading it must clear it.
    #[test]
    fn bell_is_taken_once() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x07");
        assert!(s.take_bell());
        assert!(!s.take_bell(), "take_bell must clear the flag");
    }

    /// A split escape sequence is the real-world case: a PTY read boundary
    /// can land anywhere, including mid-CSI. The retained parser must not
    /// lose or misfire on the fragment.
    #[test]
    fn a_split_csi_sequence_still_applies() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[1;3");
        s.feed(b"1mR");
        let row = s.grid.row_cells(0);
        assert_eq!(row[0].fg, Color::Indexed(1));
        assert!(row[0].attrs.contains(Attrs::BOLD));
    }

    #[test]
    fn cup_moves_the_cursor_one_based() {
        let mut s = Screen::new(5, 20);
        s.feed(b"\x1b[3;7HX");
        // CSI row;col H is 1-based; row 3 col 7 is grid (2, 6).
        assert_eq!(s.grid.row_cells(2)[6].ch, 'X');
    }

    #[test]
    fn cup_without_params_homes_the_cursor() {
        let mut s = Screen::new(3, 10);
        s.feed(b"abc\r\ndef\x1b[HZ");
        assert_eq!(s.grid.row_text(0), "Zbc");
    }

    #[test]
    fn erase_in_line_to_end_clears_the_tail_only() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abcdef\x1b[1;4H\x1b[0K");
        assert_eq!(s.grid.row_text(0), "abc");
    }

    #[test]
    fn erase_in_display_two_clears_everything() {
        let mut s = Screen::new(3, 10);
        s.feed(b"aaa\r\nbbb\x1b[2J");
        assert_eq!(s.grid.row_text(0), "");
        assert_eq!(s.grid.row_text(1), "");
    }

    /// Cursor-up at the top row must clamp, not underflow. This is the
    /// arithmetic that panics in debug and wraps in release if written with
    /// unsigned subtraction.
    #[test]
    fn cursor_up_at_the_top_row_clamps() {
        let mut s = Screen::new(3, 10);
        s.feed(b"\x1b[10A\x1b[10DX");
        assert_eq!(s.grid.cursor().0, 0);
        assert_eq!(s.grid.row_cells(0)[0].ch, 'X');
    }

    /// `f` is CUP's alias (HVP) and must behave identically to `H`.
    #[test]
    fn cursor_position_alias_f_also_moves_the_cursor() {
        let mut s = Screen::new(5, 20);
        s.feed(b"\x1b[2;3fY");
        assert_eq!(s.grid.row_cells(1)[2].ch, 'Y');
    }

    /// `CSI B` / `CSI C` move down/right and must clamp at the bottom-right
    /// edge rather than walking off the grid. Asserted right after the
    /// movement, before printing — `put` itself advances the cursor past
    /// the last column once it writes there (existing Task 2 wrap-before-
    /// write behaviour), so checking cursor position after the print would
    /// conflate that with clamping.
    #[test]
    fn cursor_down_and_forward_clamp_at_the_bottom_right() {
        let mut s = Screen::new(3, 5);
        s.feed(b"\x1b[99B\x1b[99C");
        assert_eq!(
            s.grid.cursor(),
            (2, 4),
            "movement clamps at the last row and column"
        );
        s.feed(b"X");
        assert_eq!(s.grid.row_cells(2)[4].ch, 'X');
    }

    /// Erasing a range whose edge cuts through a wide glyph must clear both
    /// the owner and its spacer — never strand one without the other. This
    /// is asserted through `row_cells`, not `row_text`, because `row_text`
    /// filters spacers and would hide the corruption (the same reason the
    /// `put` spacer-repair bug survived its original tests).
    #[test]
    fn erase_in_line_to_end_does_not_strand_a_spacer_at_the_left_edge() {
        let mut s = Screen::new(2, 10);
        // "a" then a wide glyph at columns 1-2, so erasing from column 2
        // onward starts exactly on the glyph's spacer.
        s.feed("a中".as_bytes());
        s.feed(b"\x1b[1;3H\x1b[0K");
        let row = s.grid.row_cells(0);
        assert_eq!(row[0].ch, 'a', "column before the erase is untouched");
        assert_eq!(
            row[1].ch, ' ',
            "the wide glyph's owner must not survive without its spacer"
        );
        assert!(!row[2].is_spacer(), "no orphaned spacer may remain");
    }

    #[test]
    fn erase_in_line_from_start_does_not_strand_a_spacer_at_the_right_edge() {
        let mut s = Screen::new(2, 10);
        // A wide glyph at columns 0-1, then "a" at column 2. Erasing
        // start-to-cursor with the cursor on the glyph's owner (column 0)
        // must also clear its spacer at column 1.
        s.feed("中a".as_bytes());
        s.feed(b"\x1b[1;1H\x1b[1K");
        let row = s.grid.row_cells(0);
        assert_eq!(row[0].ch, ' ');
        assert!(!row[1].is_spacer(), "no orphaned spacer may remain");
        assert_eq!(row[2].ch, 'a', "column after the erase is untouched");
    }

    /// `goto` can land the cursor directly on a spacer (previously only
    /// reachable via internal wrapping). A subsequent `put` there must still
    /// repair the orphaned owner rather than writing into a corrupted cell.
    #[test]
    fn goto_onto_a_spacer_then_printing_repairs_the_owner() {
        let mut s = Screen::new(2, 10);
        s.feed("中中".as_bytes()); // columns 0-1 and 2-3
        s.feed(b"\x1b[1;4Hx"); // 1-based col 4 = 0-based col 3, the second glyph's spacer
        let row = s.grid.row_cells(0);
        assert_eq!(row[0].ch, '中', "the first glyph is untouched");
        assert_eq!(row[2].ch, ' ', "the orphaned owner must be blanked");
        assert_eq!(row[3].ch, 'x');
    }

    #[test]
    fn alt_screen_is_separate_and_the_primary_survives_the_round_trip() {
        let mut s = Screen::new(3, 20);
        s.feed(b"primary");
        s.feed(b"\x1b[?1049h");
        assert!(s.alt_screen());
        assert_eq!(s.grid.row_text(0), "", "the alt screen starts blank");
        s.feed(b"alt");
        assert_eq!(s.grid.row_text(0), "alt");
        s.feed(b"\x1b[?1049l");
        assert!(!s.alt_screen());
        assert_eq!(
            s.grid.row_text(0),
            "primary",
            "the primary screen must survive"
        );
    }

    #[test]
    fn resize_preserves_content_that_still_fits() {
        let mut s = Screen::new(3, 20);
        s.feed(b"hello");
        s.resize(5, 40);
        assert_eq!(s.grid.dims(), (5, 40));
        assert_eq!(s.grid.row_text(0), "hello");
    }

    /// The alt-screen swap must apply at the exact point `?1049h`/`?1049l`
    /// is parsed, not deferred until `feed` finishes the whole chunk. The
    /// round-trip test above feeds the escape and the following bytes in
    /// SEPARATE `feed()` calls, which cannot see this: real programs (vim,
    /// less) routinely emit the enter escape and their first frame in ONE
    /// write. If the swap is deferred, that first frame prints onto the
    /// PRIMARY grid — which a deferred swap then stashes into `saved`,
    /// permanently burying it under the (still-blank) alt screen and
    /// leaving alt-screen content sitting in the user's shell scrollback.
    #[test]
    fn alt_screen_swap_applies_before_the_rest_of_the_same_chunk_is_parsed() {
        let mut s = Screen::new(3, 20);
        s.feed(b"primary");
        // Escape AND its first frame in one feed() call — the case a
        // deferred swap cannot handle.
        s.feed(b"\x1b[?1049hHELLO");
        assert!(s.alt_screen());
        assert_eq!(
            s.grid.row_text(0),
            "HELLO",
            "HELLO must land on the alt grid, not the primary"
        );

        s.feed(b"\x1b[?1049l");
        assert!(!s.alt_screen());
        assert_eq!(
            s.grid.row_text(0),
            "primary",
            "the primary must not have been polluted by alt-screen content written in the same chunk"
        );
    }

    /// Entering the alt screen must mark it dirty even though `Grid::new`
    /// starts with an EMPTY dirty set (by design, for the `full_patch` /
    /// `pty.attach` path, which ignores the dirty set entirely). A swap
    /// mid-session is the opposite case: an already-attached client only
    /// ever reads `take_patch`, so without marking the fresh alt grid
    /// dirty it reports nothing changed while the whole visible screen was
    /// just replaced. Asserted through `take_patch`, not `full_patch` --
    /// `full_patch` would pass with the bug fully present.
    #[test]
    fn entering_alt_screen_marks_it_dirty_through_take_patch() {
        let mut s = Screen::new(3, 20);
        s.feed(b"primary");
        let _ = s.take_patch(); // drain the initial write's own patch

        s.feed(b"\x1b[?1049h");

        let p = s
            .take_patch()
            .expect("entering the alt screen must produce a patch");
        let rows: Vec<u16> = p.rows.iter().map(|r| r.row).collect();
        assert_eq!(
            rows,
            vec![0, 1, 2],
            "every row of the (blank) alt screen must ship"
        );
        assert_eq!(p.alt_screen, Some(true));
    }

    /// The mirror direction: leaving restores `saved`, whose dirty set is
    /// whatever was stashed on it when it was last current -- stale.
    /// Without re-marking it, `take_patch` reports nothing changed while
    /// vim's last frame is replaced by the shell screen underneath.
    #[test]
    fn exiting_alt_screen_marks_the_restored_primary_dirty_through_take_patch() {
        let mut s = Screen::new(3, 20);
        s.feed(b"primary");
        s.feed(b"\x1b[?1049h");
        let _ = s.take_patch(); // drain the enter's own patch

        s.feed(b"\x1b[?1049l");

        let p = s
            .take_patch()
            .expect("exiting the alt screen must produce a patch");
        let rows: Vec<u16> = p.rows.iter().map(|r| r.row).collect();
        assert_eq!(
            rows,
            vec![0, 1, 2],
            "every row of the restored primary must ship"
        );
        assert_eq!(p.alt_screen, Some(false));
    }

    /// CSI G (CHA). zsh redraws its input line by jumping to a column and
    /// reprinting; dropping this makes every redraw append instead of
    /// overwrite, so typing one `x` puts `xxx` on the screen. Measured on a
    /// real server before this arm existed.
    #[test]
    fn cha_moves_the_cursor_to_an_absolute_column() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abcdef\x1b[1GZ");
        assert_eq!(s.grid.row_text(0), "Zbcdef");
    }

    /// The 1-based wire convention, and a column past the edge clamps
    /// rather than panicking.
    #[test]
    fn cha_is_one_based_and_clamps() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abcdef\x1b[3GZ");
        assert_eq!(s.grid.row_text(0), "abZdef");

        let mut s = Screen::new(2, 10);
        s.feed(b"abc\x1b[99GZ");
        assert_eq!(
            s.grid.row_text(0).len(),
            10,
            "a column past the right edge clamps to the last column"
        );
    }

    /// CSI d (VPA) is CHA's row twin; p10k uses it to park the cursor.
    #[test]
    fn vpa_moves_the_cursor_to_an_absolute_row() {
        let mut s = Screen::new(4, 6);
        s.feed(b"top\x1b[3dZ");
        assert_eq!(s.grid.row_text(0), "top");
        assert_eq!(
            s.grid.row_text(2),
            "   Z",
            "row 3 (1-based), column unchanged"
        );
    }

    /// CSI X (ECH) blanks in place: it moves neither the cursor nor the
    /// cells after the erased run.
    #[test]
    fn ech_blanks_cells_without_moving_the_cursor_or_the_tail() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abcdef\x1b[1G\x1b[3XQ");
        assert_eq!(s.grid.row_text(0), "Q  def");
    }

    /// CSI P (DCH) pulls the tail left; CSI @ (ICH) pushes it right. Both
    /// are how a line editor edits mid-line.
    #[test]
    fn dch_pulls_the_tail_left_and_ich_pushes_it_right() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abcdef\x1b[1G\x1b[2P");
        assert_eq!(s.grid.row_text(0), "cdef");

        let mut s = Screen::new(2, 10);
        s.feed(b"abcdef\x1b[1G\x1b[2@");
        assert_eq!(s.grid.row_text(0), "  abcdef");
    }

    /// A delete or insert wider than the row must blank it, not panic and
    /// not wrap into the next row.
    #[test]
    fn char_edits_wider_than_the_row_blank_it() {
        let mut s = Screen::new(2, 6);
        s.feed(b"abcdef\x1b[1G\x1b[99P");
        assert_eq!(s.grid.row_text(0), "");

        let mut s = Screen::new(2, 6);
        s.feed(b"abcdef\x1b[1G\x1b[99@");
        assert_eq!(s.grid.row_text(0), "");
    }

    /// CSI L / M insert and delete whole rows from the cursor row down.
    #[test]
    fn il_and_dl_move_the_rows_below_the_cursor() {
        let mut s = Screen::new(4, 4);
        s.feed(b"aaa\r\nbbb\x1b[1A\x1b[1L");
        assert_eq!(s.grid.row_text(0), "");
        assert_eq!(s.grid.row_text(1), "aaa");
        assert_eq!(s.grid.row_text(2), "bbb");

        let mut s = Screen::new(4, 4);
        s.feed(b"aaa\r\nbbb\x1b[1;1H\x1b[1M");
        assert_eq!(s.grid.row_text(0), "bbb");
        assert_eq!(s.grid.row_text(1), "");
    }

    /// Rows pushed off the bottom by an insert are discarded, not filed as
    /// history: scrollback is what scrolled off the TOP of the screen, and
    /// a mid-screen insert never reached the top. Filing them would let a
    /// client scrolling back read rows the user never saw leave the screen.
    #[test]
    fn rows_pushed_off_the_bottom_do_not_become_scrollback() {
        let mut s = Screen::new(3, 4);
        s.feed(b"aaa\r\nbbb\r\nccc");
        let before = s.grid.scrollback_len();
        s.feed(b"\x1b[1;1H\x1b[2L");
        assert_eq!(
            s.grid.scrollback_len(),
            before,
            "an in-screen line insert is not history"
        );
        assert_eq!(s.grid.row_text(2), "aaa", "the rows below moved down");
    }

    /// A line editor mid-line does not only shift ASCII: DCH/ICH can cut
    /// through a wide glyph's owner/spacer pair the same way an erase can.
    /// `clear_range` handles that for erases by widening the range; a shift
    /// cannot widen, so the halves are re-checked after the move. Asserted
    /// through `row_cells` -- `row_text` filters spacers and would hide a
    /// half-glyph, which is exactly how the original spacer bug survived.
    #[test]
    fn char_edits_do_not_strand_half_of_a_wide_glyph() {
        // Delete the column the wide glyph owns: its spacer must not
        // survive alone once the tail slides left.
        let mut s = Screen::new(2, 10);
        s.feed("a\u{4e2d}b".as_bytes());
        s.feed(b"\x1b[1;2H\x1b[1P");
        assert!(
            !s.grid.row_cells(0).iter().any(|c| c.is_spacer()),
            "deleting a wide glyph's owner must not leave its spacer behind"
        );

        // Insert in the middle of the pair: the owner stays put and the
        // spacer is pushed one cell right, so neither may survive.
        let mut s = Screen::new(2, 10);
        s.feed("a\u{4e2d}b".as_bytes());
        s.feed(b"\x1b[1;3H\x1b[1@");
        assert!(
            !s.grid.row_cells(0).iter().any(|c| c.is_spacer()),
            "splitting a wide glyph with an insert must leave neither half"
        );
    }

    /// Every arm that changes cells must mark its row dirty, or a client
    /// that missed no frames never learns the row changed -- the defect
    /// would come back as "it is right after a refresh and wrong while you
    /// watch". The print that sets the row up is drained first: without
    /// that drain this loop passes against an unimplemented verb, because
    /// printing `abc` already dirtied row 0.
    ///
    /// CHA and VPA are absent on purpose -- they move the cursor and touch
    /// no cell, so they have no row to dirty; their effect is asserted by
    /// the text tests above.
    #[test]
    fn the_new_cell_editing_arms_all_mark_their_rows_dirty() {
        for seq in [
            &b"\x1b[1X"[..],
            &b"\x1b[1P"[..],
            &b"\x1b[1@"[..],
            &b"\x1b[1L"[..],
            &b"\x1b[1M"[..],
        ] {
            let mut s = Screen::new(3, 6);
            s.feed(b"abc\x1b[1G");
            let _ = s.grid.take_dirty();
            s.feed(seq);
            assert!(
                !s.grid.take_dirty().is_empty(),
                "{seq:?} changed cells but marked nothing dirty"
            );
        }
    }

    /// The verb list in `csi_dispatch` was written by hand once, and the
    /// one it omitted -- CHA (`CSI G`) -- is the one ordinary typing
    /// depends on: every zsh prompt redraw jumps back to a column and
    /// reprints, so dropping it made each redraw append instead of
    /// overwrite and one keystroke drew three characters. Nothing went red,
    /// because nothing tests a verb that was never listed.
    ///
    /// So this guard does not restate the list. It reads the final bytes
    /// `csi_dispatch` claims out of that function's own source and requires
    /// the probe table to name exactly the same set: an arm added without a
    /// probe fails the set comparison, and a claimed verb that no longer
    /// reaches the grid fails its own probe. A scraper that stopped finding
    /// literals would yield an empty set and fail the same comparison, so
    /// there is no arrangement in which this passes by seeing nothing.
    ///
    /// It follows that a character literal added to `csi_dispatch` for some
    /// purpose other than naming a final byte also turns this red. That is
    /// the intended trade: a loud question is cheaper than a scanner that
    /// quietly decides which literals it believes in.
    #[test]
    fn every_csi_verb_the_dispatcher_claims_actually_reaches_the_grid() {
        // (final byte, bytes whose effect on the screen proves the arm ran)
        let probes: &[(u8, &[u8])] = &[
            (b'H', b"abc\x1b[1;1HZ"),
            (b'f', b"abc\x1b[1;1fZ"),
            (b'A', b"a\r\nb\x1b[1AZ"),
            (b'B', b"a\x1b[1BZ"),
            (b'C', b"a\x1b[2CZ"),
            (b'D', b"abc\x1b[2DZ"),
            (b'G', b"abc\x1b[1GZ"),
            (b'd', b"abc\x1b[2dZ"),
            (b'J', b"abc\x1b[2J"),
            (b'K', b"abc\x1b[1G\x1b[0K"),
            (b'X', b"abc\x1b[1G\x1b[2X"),
            (b'P', b"abc\x1b[1G\x1b[1P"),
            (b'@', b"abc\x1b[1G\x1b[1@"),
            (b'L', b"abc\x1b[1L"),
            (b'M', b"abc\x1b[1M"),
            (b'm', b"\x1b[31ma"),
            (b'h', b"abc\x1b[?1049h"),
            (b'l', b"abc\x1b[?1049h\x1b[?1049l"),
        ];
        // `~` is a legal CSI final byte no arm claims.
        each_probe_reaches_the_grid("CSI", probes, b'~', 3, 8);
        assert_claims_match_probes(
            "CSI",
            &claimed_char_literals(&dispatch_body("csi_dispatch")),
            probes,
        );
    }

    /// The C0 table is the same shape as the CSI one and had the same
    /// disease: it claimed LF, CR and BEL and nothing else, so BS and HT --
    /// which is how a shell moves back over what it just drew, and how
    /// every column of every tabular output is placed -- were dropped by an
    /// arm that was never written. Same treatment, not a second copy of the
    /// list: the claimed set is read out of that function's own source.
    #[test]
    fn every_c0_control_the_dispatcher_claims_actually_reaches_the_grid() {
        let probes: &[(u8, &[u8])] = &[
            (0x07, b"a\x07"),
            (0x08, b"ab\x08"),
            (0x09, b"a\x09"),
            (0x0a, b"a\x0a"),
            (0x0b, b"a\x0b"),
            (0x0c, b"a\x0c"),
            (0x0d, b"ab\x0d"),
        ];
        // 0x06 (ACK) is a C0 control no arm claims, so it still reaches the
        // dispatcher and falls through -- the role `~` plays for CSI.
        each_probe_reaches_the_grid("C0", probes, 0x06, 3, 20);
        assert_claims_match_probes(
            "C0",
            &claimed_byte_literals(&dispatch_body("execute")),
            probes,
        );
    }

    /// The third table, and the one that was not there at all: `vte`'s
    /// default for an unimplemented `Perform` method is a silent no-op, so
    /// every non-CSI escape was dropped by construction rather than by a
    /// decision. Only DECSC/DECRC are claimed; the guard's job is to keep
    /// "claimed" and "reaches the grid" the same set as that grows.
    #[test]
    fn every_escape_the_dispatcher_claims_actually_reaches_the_grid() {
        // One sequence proves both halves: with `7` inert nothing is saved,
        // with `8` inert nothing is restored, and `Z` lands elsewhere either
        // way -- which is also why neither probe proves the other's arm.
        let probes: &[(u8, &[u8])] = &[(b'7', b"ab\x1b7cd\x1b8Z"), (b'8', b"ab\x1b7cd\x1b8Z")];
        // `ESC 9` is a final byte no arm claims.
        each_probe_reaches_the_grid("ESC", probes, b'9', 3, 20);
        assert_claims_match_probes(
            "ESC",
            &claimed_byte_literals(&dispatch_body("esc_dispatch")),
            probes,
        );
    }

    /// The body of one `Perform` method, comment lines removed -- comments
    /// are prose, not dispatch, and the guards' own paragraphs name verbs
    /// their function does not handle. `\r` goes first so the boundary
    /// still matches on a CRLF checkout.
    fn dispatch_body(method: &str) -> String {
        let src = include_str!("perform.rs").replace('\r', "");
        let body = src
            .split_once(&format!("fn {method}"))
            .unwrap_or_else(|| panic!("no `fn {method}` in this file"))
            .1
            .split("\n    fn ")
            .next()
            .expect("split always yields a first piece")
            .to_string();
        body.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every `'X'` character literal. In `csi_dispatch` those are the final
    /// bytes it claims; a `b'?'` byte literal there compares an
    /// intermediate, not a final byte, so it is excluded.
    fn claimed_char_literals(code: &str) -> std::collections::BTreeSet<u8> {
        let c: Vec<char> = code.chars().collect();
        let mut out = std::collections::BTreeSet::new();
        for i in 1..c.len() {
            if c[i] == '\'' && c.get(i + 2) == Some(&'\'') && c[i - 1] != 'b' && c[i + 1].is_ascii()
            {
                out.insert(c[i + 1] as u8);
            }
        }
        out
    }

    /// Every byte literal, in the three spellings the dispatch tables use.
    /// Each is exercised by a different table, which is worth writing down
    /// because it decides which guard a blind spot shows up in:
    ///
    /// | spelling | example  | exercised by   |
    /// |----------|----------|----------------|
    /// | plain    | `b'7'`   | `esc_dispatch` |
    /// | escape   | `b'\n'`  | `execute`      |
    /// | hex      | `0x0a`   | `execute`      |
    ///
    /// So every path has a table whose guard drops bytes if that path stops
    /// reading -- verified by blinding each one in turn. A scraper blind to
    /// a spelling nobody used yet would go unnoticed, which is the same
    /// reason the tables themselves get scraped rather than restated.
    fn claimed_byte_literals(code: &str) -> std::collections::BTreeSet<u8> {
        let c: Vec<char> = code.chars().collect();
        let mut out = std::collections::BTreeSet::new();
        let mut i = 0;
        while i < c.len() {
            if c[i] == 'b' && c.get(i + 1) == Some(&'\'') {
                if c.get(i + 3) == Some(&'\'') && c[i + 2].is_ascii() {
                    out.insert(c[i + 2] as u8);
                    i += 4;
                    continue;
                }
                if c.get(i + 2) == Some(&'\\') && c.get(i + 4) == Some(&'\'') {
                    // An escape this arm does not know is simply not
                    // inserted, which fails the set comparison rather than
                    // passing quietly.
                    let byte = match c[i + 3] {
                        'n' => Some(b'\n'),
                        'r' => Some(b'\r'),
                        't' => Some(b'\t'),
                        '0' => Some(0),
                        _ => None,
                    };
                    if let Some(byte) = byte {
                        out.insert(byte);
                    }
                    i += 5;
                    continue;
                }
            }
            if c[i] == '0' && c.get(i + 1) == Some(&'x') {
                let hex: String = c.iter().skip(i + 2).take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    out.insert(byte);
                    i += 4;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Everything a `Perform` method can change: the cells, the cursor and
    /// the bell. Cells alone would call a cursor-only arm (BS, HT) or a
    /// flag-only arm (BEL) "no change" and pass against an arm that was
    /// never wired.
    fn observable(bytes: &[u8], rows: u16, cols: u16) -> (Vec<Vec<Cell>>, (u16, u16), bool) {
        let mut s = Screen::new(rows, cols);
        s.feed(bytes);
        let cells: Vec<Vec<Cell>> = (0..rows).map(|r| s.grid.row_cells(r).to_vec()).collect();
        (cells, s.grid.cursor(), s.take_bell())
    }

    /// Feed each probe, then feed it again with the byte it claims swapped
    /// for one no arm claims; whatever differs is what that arm did.
    fn each_probe_reaches_the_grid(
        kind: &str,
        probes: &[(u8, &[u8])],
        inert: u8,
        rows: u16,
        cols: u16,
    ) {
        for (claimed, seq) in probes {
            assert_eq!(
                seq.iter().filter(|b| *b == claimed).count(),
                1,
                "the {kind} probe for {:?} must contain that byte exactly once, or the \
                 mutation below changes more than the one arm",
                char::from(*claimed)
            );
            let mutated: Vec<u8> = seq
                .iter()
                .map(|b| if b == claimed { inert } else { *b })
                .collect();
            assert_ne!(
                observable(seq, rows, cols),
                observable(&mutated, rows, cols),
                "{kind} {:?} changed nothing -- it is not wired to the grid",
                char::from(*claimed)
            );
        }
    }

    /// The set the dispatcher names and the set probed above must be equal.
    /// An arm added without a probe fails here, and so does a scraper that
    /// stopped finding literals -- there is no arrangement in which these
    /// guards pass by seeing nothing.
    fn assert_claims_match_probes(
        kind: &str,
        claimed: &std::collections::BTreeSet<u8>,
        probes: &[(u8, &[u8])],
    ) {
        let probed: std::collections::BTreeSet<u8> = probes.iter().map(|(b, _)| *b).collect();
        let show = |s: &std::collections::BTreeSet<u8>| {
            s.iter()
                .map(|b| format!("{:?}", char::from(*b)))
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            *claimed,
            probed,
            "the {kind} bytes the dispatcher names and the ones probed here must be the \
             same set -- add a probe for the arm you added, or drop the probe for the arm \
             you removed\n  named:  {}\n  probed: {}",
            show(claimed),
            show(&probed)
        );
    }

    /// HT is a MOVE, not a write of spaces. The two are identical on a
    /// fresh row and differ the moment a tab crosses text that is already
    /// there -- which is exactly what a shell does when it redraws a line,
    /// so the probe puts the tab across existing letters.
    #[test]
    fn ht_moves_to_the_next_tab_stop_without_writing_over_what_it_crosses() {
        let mut s = Screen::new(2, 20);
        s.feed(b"abcdefghij\rX\tY");
        assert_eq!(
            s.grid.row_text(0),
            "XbcdefghYj",
            "the letters the tab crossed must survive; a version that wrote spaces \
             would read 'X       Yj'"
        );
    }

    /// Stops are every 8 columns, and the last one clamps rather than
    /// running off the row.
    #[test]
    fn ht_stops_every_eight_columns_and_clamps_at_the_edge() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abc\t");
        assert_eq!(
            s.grid.cursor(),
            (0, 8),
            "column 3 advances to the stop at 8"
        );

        let mut s = Screen::new(2, 10);
        s.feed(b"abcdefghi\t");
        assert_eq!(
            s.grid.cursor(),
            (0, 9),
            "past the last stop, HT clamps to the last column"
        );
    }

    /// A tab stop can land on the spacer half of a double-width glyph.
    /// Ruling: HT moves to the column and stops there, exactly like `goto`
    /// -- landing on a spacer is a question `put`'s owner/spacer repair
    /// already answers, and answering it a second way here would be a
    /// second source of truth for it.
    #[test]
    fn ht_onto_a_wide_glyphs_spacer_is_a_plain_cursor_move() {
        let mut s = Screen::new(2, 20);
        s.feed("abcdefg\u{4e2d}".as_bytes()); // a-g in 0..=6, the glyph in 7..=8
        s.feed(b"\rX\tY");

        let row = s.grid.row_cells(0);
        assert_eq!(row[7].ch, ' ', "the orphaned owner is blanked by `put`");
        assert_eq!(row[8].ch, 'Y', "the write lands on the tab stop itself");
        assert!(
            !row.iter().any(|c| c.is_spacer()),
            "no half of the glyph may survive"
        );
    }

    /// BS moves left and does nothing else. Erasing is the application's
    /// job: a shell sends `\b \b` when it wants the character gone, and a
    /// BS that erased would make that eat two characters.
    #[test]
    fn bs_moves_left_without_erasing() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abc\x08");
        assert_eq!(s.grid.cursor(), (0, 2));
        assert_eq!(
            s.grid.row_cells(0)[2].ch,
            'c',
            "BS must not erase the cell it moved off"
        );

        let mut s = Screen::new(2, 10);
        s.feed(b"abc\x08 \x08");
        assert_eq!(
            s.grid.row_text(0),
            "ab",
            "the shell's rub-out removes exactly one character"
        );
        assert_eq!(s.grid.cursor(), (0, 2));
    }

    #[test]
    fn bs_at_column_zero_stays_put() {
        let mut s = Screen::new(3, 6);
        s.feed(b"ab\r\ncd\r\x08");
        assert_eq!(
            s.grid.cursor(),
            (1, 0),
            "BS does not wrap back onto the previous row"
        );
    }

    /// VT and FF move down a line like LF -- what xterm does, and a program
    /// that emits either means "next line", never "nothing".
    #[test]
    fn vt_and_ff_move_down_a_line_like_lf() {
        let mut s = Screen::new(4, 6);
        s.feed(b"a\x0bb\x0cc");
        assert_eq!(s.grid.row_text(0), "a");
        assert_eq!(s.grid.row_text(1), " b", "VT moved down, not to column 0");
        assert_eq!(s.grid.row_text(2), "  c", "FF likewise");
    }

    /// DECSC/DECRC save and restore the cursor AND the style together,
    /// because that is what the sequence means. A version that saved only
    /// the position passes a position assertion and then loses colour on
    /// every prompt that brackets its output with 7/8, which is why the
    /// colour is asserted here beside the column.
    #[test]
    fn decsc_and_decrc_restore_the_cursor_and_the_style_together() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[31m\x1b7\x1b[0mabc\x1b8Z");
        assert_eq!(
            s.grid.row_text(0),
            "Zbc",
            "the cursor came back to column 0"
        );
        assert_eq!(
            s.grid.row_cells(0)[0].fg,
            Color::Indexed(1),
            "the saved style came back with it"
        );
        assert_eq!(
            s.grid.row_cells(0)[1].fg,
            Color::Default,
            "b was printed after the reset and keeps that style"
        );
    }

    /// `ESC # 8` is DECALN (a screen alignment test), not DECRC. It shares
    /// its final byte with the restore, so matching on that byte alone
    /// would run one as the other -- the intermediates are load-bearing
    /// here, the same way `?` is on the alt-screen arm.
    #[test]
    fn an_escape_with_intermediates_is_not_read_as_a_cursor_restore() {
        let mut s = Screen::new(2, 10);
        s.feed(b"\x1b7abc\x1b#8Z");
        assert_eq!(
            s.grid.row_text(0),
            "abcZ",
            "DECALN is not implemented and must not be dispatched as DECRC"
        );
    }

    /// A restore with nothing saved leaves the cursor where it is. DEC's
    /// spec homes it instead; nothing here needs that, and a stray `ESC 8`
    /// that homed the cursor would move a screen the user is watching,
    /// where doing nothing cannot.
    #[test]
    fn decrc_without_a_save_leaves_the_cursor_alone() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abc\x1b8Z");
        assert_eq!(s.grid.row_text(0), "abcZ");
    }
}
