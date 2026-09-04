//! CSI dispatch -- `\e[ ... <final byte>`, plus the two helpers only CSI
//! reaches: SGR parameter folding and the alternate-screen swap.

use super::super::grid::{Attrs, Color, Grid};
use super::{AltBuffer, Performer};

impl Performer<'_> {
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
    pub(super) fn toggle_alt_screen(&mut self, enter: bool, buffer: AltBuffer) {
        let swapped = if enter {
            // A nested `?1049h` while already on the alt screen is a
            // no-op: clobbering `saved` here would replace the real
            // primary with the alt screen's own content, losing the
            // user's shell scrollback for good the next time they exit.
            // No grid becomes current here, so nothing needs marking dirty.
            if self.screen.saved.is_none() {
                let (rows, cols) = self.screen.grid.dims();
                // One alternate buffer, two entry policies. 47/1047 take back
                // whatever was parked on the last exit; 1049 always starts
                // blank, which is the whole of the difference between them.
                let alt = match buffer {
                    AltBuffer::Legacy => self
                        .screen
                        .retained_alt
                        .take()
                        .unwrap_or_else(|| Grid::new(rows, cols)),
                    AltBuffer::Cleared => Grid::new(rows, cols),
                };
                let primary = std::mem::replace(&mut self.screen.grid, alt);
                self.screen.saved = Some(primary);
                true
            } else {
                false
            }
        } else if let Some(primary) = self.screen.saved.take() {
            // Park the alternate buffer rather than drop it: whether it is
            // ever read again is the NEXT entry's decision, not this exit's.
            // Dropping here would make 47/1047's promise depend on which
            // spelling happened to leave.
            let alt = std::mem::replace(&mut self.screen.grid, primary);
            self.screen.retained_alt = Some(alt);
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

    /// DEC private modes (`CSI ? Pm h` / `CSI ? Pm l`).
    ///
    /// A table of its own rather than arms inside [`Self::csi`], because the
    /// census that guards `csi` reads FINAL BYTES: every mode here shares
    /// `h`/`l`, so folding them in would put mode numbers where that census
    /// expects verbs. Unknown modes fall through -- a mode nothing tracks is
    /// not an error, it is a mode nothing tracks.
    fn private_mode(&mut self, mode: u16, enable: bool) {
        match mode {
            // DECAWM.
            7 => self.screen.grid.set_autowrap(enable),
            // Legacy alternate screen. Distinct from 1049 only in that the
            // alternate buffer survives the round trip.
            47 | 1047 => self.toggle_alt_screen(enable, AltBuffer::Legacy),
            // Save/restore cursor -- DECSC/DECRC's private-mode spelling,
            // sharing the one slot.
            1048 => {
                if enable {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => self.toggle_alt_screen(enable, AltBuffer::Cleared),
            _ => {}
        }
    }

    /// The CSI table. Lives here rather than in the `vte::Perform` impl so
    /// there is exactly one body naming these final bytes -- the census in
    /// `tests.rs` reads this function's source and compares the verbs it
    /// names against the verbs it probes.
    pub(super) fn csi(&mut self, params: &vte::Params, inter: &[u8], action: char) {
        // Flatten sub-parameters: only SGR's 38/48 use them, and it reads the
        // colon form (38:2:r:g:b) identically to the semicolon form.
        let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
        if action == 'm' {
            let effective: &[u16] = if flat.is_empty() { &[0] } else { &flat };
            self.sgr(effective);
        }

        // REP repeats the last PRINTED character, so every other CSI has to
        // invalidate the candidate. Taken once here rather than cleared in
        // each arm: an arm added later cannot forget to do it, and REP's own
        // arm still sees the value because it reads this binding. `put`
        // records the character again, so consecutive REPs keep working.
        let repeatable = self.screen.grid.take_last_printed();

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
            // REP: repeat the preceding printed character. A missing
            // candidate writes nothing -- the only honest answer, since the
            // alternative is repeating whatever byte happened to come last.
            'b' => {
                if let Some(c) = repeatable {
                    let style = self.style();
                    for _ in 0..p(0, 1) {
                        self.screen.grid.put(c, style);
                    }
                }
            }
            // DECSTR (soft terminal reset). The `!` arrives as an
            // INTERMEDIATE, never as a parameter, and it is the whole of
            // what separates this from an unrelated `CSI p`.
            'p' if inter == b"!" => self.soft_reset(),
            // DEC private modes. `?` only ever arrives via `intermediates`,
            // never `params` — a guard on `action` alone would also swallow
            // the private-mode-less `h`/`l` sequences. Every parameter is
            // applied, not just the first: `\e[?1049;25h` sets both, and
            // reading only `flat.first()` would drop the rest silently.
            // Applied inline, not deferred -- see `toggle_alt_screen`.
            'h' | 'l' if inter == b"?" => {
                for mode in &flat {
                    self.private_mode(*mode, action == 'h');
                }
            }
            _ => {}
        }
    }
}
