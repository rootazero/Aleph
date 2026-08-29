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
        (self.screen.state.fg, self.screen.state.bg, self.screen.state.attrs)
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
            b'\n' => self.screen.grid.newline(),
            b'\r' => self.screen.grid.carriage_return(),
            0x07 => self.screen.state.bell = true,
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
            'J' => self.screen.grid.erase_in_display(flat.first().copied().unwrap_or(0)),
            'K' => self.screen.grid.erase_in_line(flat.first().copied().unwrap_or(0)),
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
    use crate::gateway::pty::screen::grid::{Attrs, Color};

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
        assert_eq!(s.grid.cursor(), (2, 4), "movement clamps at the last row and column");
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
        assert_eq!(row[1].ch, ' ', "the wide glyph's owner must not survive without its spacer");
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
        assert_eq!(s.grid.row_text(0), "primary", "the primary screen must survive");
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

        let p = s.take_patch().expect("entering the alt screen must produce a patch");
        let rows: Vec<u16> = p.rows.iter().map(|r| r.row).collect();
        assert_eq!(rows, vec![0, 1, 2], "every row of the (blank) alt screen must ship");
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

        let p = s.take_patch().expect("exiting the alt screen must produce a patch");
        let rows: Vec<u16> = p.rows.iter().map(|r| r.row).collect();
        assert_eq!(rows, vec![0, 1, 2], "every row of the restored primary must ship");
        assert_eq!(p.alt_screen, Some(false));
    }
}
