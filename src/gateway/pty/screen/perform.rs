//! `vte::Perform` implementation — turns the PTY byte stream into grid writes.

use super::grid::{Attrs, Color, Grid};

/// The parser plus the state it mutates. `Parser` is retained across `feed`
/// calls because escape sequences straddle read boundaries: an OSC title can
/// arrive in two chunks, and a parser rebuilt per read would lose the tail.
pub struct Screen {
    pub grid: Grid,
    parser: vte::Parser,
    state: ScreenState,
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
        Self { grid: Grid::new(rows, cols), parser: vte::Parser::new(), state: ScreenState::default() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut parser = std::mem::take(&mut self.parser);
        let mut performer = Performer { grid: &mut self.grid, state: &mut self.state };
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
}

struct Performer<'a> {
    grid: &'a mut Grid,
    state: &'a mut ScreenState,
}

impl Performer<'_> {
    fn style(&self) -> (Color, Color, Attrs) {
        (self.state.fg, self.state.bg, self.state.attrs)
    }

    /// SGR. Consumes the parameter list because 38/48 take trailing
    /// arguments, so this cannot be a per-parameter loop.
    fn sgr(&mut self, params: &[u16]) {
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.state.fg = Color::Default;
                    self.state.bg = Color::Default;
                    self.state.attrs = Attrs::NONE;
                }
                1 => self.state.attrs.insert(Attrs::BOLD),
                3 => self.state.attrs.insert(Attrs::ITALIC),
                4 => self.state.attrs.insert(Attrs::UNDERLINE),
                7 => self.state.attrs.insert(Attrs::REVERSE),
                22 => self.state.attrs.remove(Attrs::BOLD),
                23 => self.state.attrs.remove(Attrs::ITALIC),
                24 => self.state.attrs.remove(Attrs::UNDERLINE),
                27 => self.state.attrs.remove(Attrs::REVERSE),
                30..=37 => self.state.fg = Color::Indexed((params[i] - 30) as u8),
                39 => self.state.fg = Color::Default,
                40..=47 => self.state.bg = Color::Indexed((params[i] - 40) as u8),
                49 => self.state.bg = Color::Default,
                90..=97 => self.state.fg = Color::Indexed((params[i] - 90 + 8) as u8),
                100..=107 => self.state.bg = Color::Indexed((params[i] - 100 + 8) as u8),
                38 | 48 => {
                    let is_fg = params[i] == 38;
                    // 38;5;N (indexed) or 38;2;R;G;B (truecolour). A malformed
                    // run is skipped rather than mis-parsed into the next
                    // parameter, which would recolour unrelated text.
                    match params.get(i + 1) {
                        Some(5) => {
                            if let Some(&n) = params.get(i + 2) {
                                let c = Color::Indexed(n as u8);
                                if is_fg { self.state.fg = c } else { self.state.bg = c }
                            }
                            i += 2;
                        }
                        Some(2) => {
                            if let (Some(&r), Some(&g), Some(&b)) =
                                (params.get(i + 2), params.get(i + 3), params.get(i + 4))
                            {
                                let c = Color::Rgb(r as u8, g as u8, b as u8);
                                if is_fg { self.state.fg = c } else { self.state.bg = c }
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
}

impl vte::Perform for Performer<'_> {
    fn print(&mut self, c: char) {
        let style = self.style();
        self.grid.put(c, style);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.grid.newline(),
            b'\r' => self.grid.carriage_return(),
            0x07 => self.state.bell = true,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, _intermediates: &[u8], _ignore: bool, action: char) {
        // Flatten sub-parameters: only SGR's 38/48 use them, and it reads the
        // colon form (38:2:r:g:b) identically to the semicolon form.
        let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
        if action == 'm' {
            let effective: &[u16] = if flat.is_empty() { &[0] } else { &flat };
            self.sgr(effective);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 = icon + title, OSC 2 = title.
        let Some(kind) = params.first() else { return };
        if matches!(*kind, b"0" | b"2") {
            if let Some(raw) = params.get(1) {
                self.state.title = Some(String::from_utf8_lossy(raw).into_owned());
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
}
