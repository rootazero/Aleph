//! Emulator tests. Moved out of the production file so each dispatch face
//! stays readable on its own; the dispatch-table censuses below name the
//! file and function they scrape, because after the split a table's source
//! is no longer "this file".

    use super::*;
    use super::osc::OSC_PAYLOAD_MAX_CHARS;
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

    /// The payload's shape is not this module's choice -- it is what the
    /// detection manifests' `osc_progress` regexes match. `grok.toml` keys on
    /// `^4;1;-1$` and `qwen.toml` on `^4;3(?:;|$)`, so a payload that arrived
    /// here as anything but "everything after `9;`" would read as no evidence.
    #[test]
    fn osc_nine_four_is_retained_in_the_shape_the_manifests_match() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]9;4;3;50\x07");
        assert_eq!(s.osc_progress(), Some("4;3;50"));
    }

    /// `\e]9;4;3;\a` -- the form Claude actually paints, with the percentage
    /// field present but empty. The trailing `;` is load-bearing: it is the
    /// literal `"4;3;"` the manifest tests are written against, and dropping
    /// an empty trailing field would change which rules match.
    #[test]
    fn osc_nine_four_keeps_an_empty_trailing_field() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]9;4;3;\x07");
        assert_eq!(s.osc_progress(), Some("4;3;"));
    }

    /// OSC 9 is a shared namespace. A cwd report (`9;9;<path>`) or an iTerm2
    /// notification (`9;<text>`) must not overwrite a live progress level with
    /// a string no manifest rule can match -- that would silently downgrade
    /// "working" to "no evidence" (判据 §8).
    #[test]
    fn a_non_progress_osc_nine_does_not_clobber_the_progress_level() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]9;4;1;-1\x07");
        s.feed(b"\x1b]9;9;/tmp/somewhere\x07");
        s.feed(b"\x1b]9;a desktop notification\x07");
        assert_eq!(s.osc_progress(), Some("4;1;-1"));
    }

    /// A session holds its progress payload for its whole life, and the child
    /// process chooses the bytes. The cap has to be able to bite: vte's own
    /// OSC accumulator is 1024 bytes, four times this limit, so a hostile or
    /// buggy child can reach it.
    #[test]
    fn an_osc_nine_progress_payload_is_bounded() {
        let mut s = Screen::new(2, 20);
        let mut bytes = b"\x1b]9;4;3;".to_vec();
        bytes.extend(std::iter::repeat_n(b'9', 600));
        bytes.push(0x07);
        s.feed(&bytes);

        let kept = s.osc_progress().expect("progress payload never arrived");
        assert!(
            kept.starts_with("4;3;"),
            "the retained payload is not the progress one: {kept:?}"
        );
        assert_eq!(
            kept.chars().count(),
            OSC_PAYLOAD_MAX_CHARS,
            "payload was not capped at OSC_PAYLOAD_MAX_CHARS"
        );
    }

    /// The control-char filter in `retain_osc_progress` is a LIVE guard, not
    /// decoration -- which is a claim that has to name the bytes that reach it
    /// (判据 §2).
    ///
    /// Measured against vte 0.14.1 on 2026-09-03 by dumping the raw
    /// `osc_dispatch` params: a C0 byte (`\x01`) is swallowed by vte's own OSC
    /// state machine and never arrives, but **DEL (`\x7f`) and C1 controls
    /// (U+0080..U+009F, e.g. `\u{9b}` = CSI) are passed straight through** --
    /// raw params came back as `[b"9", b"4", b"3", b"5\x7f5\xc2\x9b0"]`. So
    /// the filter's only job is these, and this test is the case where it
    /// bites: delete the filter and the retained payload keeps the DEL and the
    /// C1 CSI, both of which are escape-sequence injection into a string an
    /// untrusted child process chose.
    #[test]
    fn del_and_c1_controls_are_stripped_from_a_progress_payload() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]9;4;3;5\x7f5\xc2\x9b0\x07");
        assert_eq!(s.osc_progress(), Some("4;3;550"));
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

        // Asserted as a position, not a length: `row_text` trims trailing
        // blanks, so a length of 10 passes for anything that leaves the row
        // 10 characters long, including a clamp to the wrong column.
        let mut s = Screen::new(2, 10);
        s.feed(b"abc\x1b[99GZ");
        assert_eq!(
            s.grid.row_text(0),
            "abc      Z",
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
    /// cannot widen, so the halves are re-checked after the move.
    ///
    /// Asserted on the CHARACTERS, not on "does a spacer remain". The two
    /// kinds of damage have to be named separately: an orphaned OWNER
    /// leaves no spacer behind, so a `!any(is_spacer)` assertion passes
    /// against it. That is how `repair_row_pairs`'s `orphan_owner` branch
    /// went untested -- measured, not supposed: replacing that whole clause
    /// with `false` left all 82 tests in this module green.
    ///
    /// `row_text` is not used because it filters spacers out entirely, so a
    /// test written against it cannot see this class at all.
    #[test]
    fn char_edits_do_not_strand_half_of_a_wide_glyph() {
        let chars = |s: &Screen, n: usize| -> Vec<char> {
            s.grid.row_cells(0)[..n].iter().map(|c| c.ch).collect()
        };

        // Delete the column the glyph owns: the tail slides left and its
        // spacer must not survive alone. (Pure `orphan_spacer`.)
        let mut s = Screen::new(2, 10);
        s.feed("a\u{4e2d}b".as_bytes());
        s.feed(b"\x1b[1;2H\x1b[1P");
        assert_eq!(
            chars(&s, 3),
            ['a', ' ', 'b'],
            "the owner was deleted, so its spacer must be blanked, not left behind"
        );

        // Insert between owner and spacer: the owner stays put, the spacer
        // is pushed one right, so BOTH halves must go -- `orphan_owner` at
        // the owner, `orphan_spacer` at the displaced spacer.
        let mut s = Screen::new(2, 10);
        s.feed("a\u{4e2d}b".as_bytes());
        s.feed(b"\x1b[1;3H\x1b[1@");
        assert_eq!(
            chars(&s, 5),
            ['a', ' ', ' ', ' ', 'b'],
            "the insert orphans the owner, which must be blanked too -- asserting \
             only that no spacer remains passes without that"
        );

        // The `c + 1 == cols` sub-case: an insert to the left of a wide
        // glyph pushes its spacer off the end of the row, stranding the
        // owner in the last column where there is no neighbour to inspect.
        //
        // On the LAST row deliberately. On any other row `cells[start +
        // cols]` is simply the next row's first cell -- blank, hence not a
        // spacer -- so dropping the `c + 1 == cols` guard gives the same
        // answer and the mutation reads as a no-op. On the last row that
        // index is off the end of the grid, which is what makes the guard
        // load-bearing rather than decorative.
        let mut s = Screen::new(2, 4);
        s.feed(b"\x1b[2;1H");
        s.feed("ab\u{4e2d}".as_bytes()); // owner at 2, spacer at 3
        s.feed(b"\x1b[2;1H\x1b[1@");
        let last: Vec<char> = s.grid.row_cells(1).iter().map(|c| c.ch).collect();
        assert_eq!(
            last,
            [' ', 'a', 'b', ' '],
            "a glyph pushed into the last column lost its spacer off the row edge"
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
            // REP: the `b` is the only one in the probe, as the mutation
            // below requires.
            (b'b', b"-\x1b[4b"),
            // DECSTR. `!` is an intermediate, so the census -- which reads
            // final bytes only -- names `p`, not `!`.
            (b'p', b"ab\x1b[!p"),
            // DECSTBM homes the cursor, which `observable` reads.
            (b'r', b"ab\x1b[2;3r"),
            // SU and SD.
            (b'S', b"a\r\nb\x1b[1S"),
            (b'T', b"a\r\nb\x1b[1T"),
        ];
        // `~` is a legal CSI final byte no arm claims.
        each_probe_reaches_the_grid("CSI", probes, b'~', 3, 8);
        assert_claims_match_probes(
            "CSI",
            &claimed_char_literals(&dispatch_body(include_str!("csi.rs"), "csi")),
            probes,
        );
        assert_forwarding_body_claims_nothing("csi_dispatch");
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
            &claimed_byte_literals(&dispatch_body(include_str!("esc.rs"), "c0")),
            probes,
        );
        assert_forwarding_body_claims_nothing("execute");
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
        let probes: &[(u8, &[u8])] = &[
            (b'7', b"ab\x1b7cd\x1b8Z"),
            (b'8', b"ab\x1b7cd\x1b8Z"),
            // RIS, IND, NEL. Each probe contains its own claimed byte
            // exactly once, which `each_probe_reaches_the_grid` enforces --
            // `abc` would have put a second `c` in the RIS probe and the
            // mutation would then have changed two things at once.
            (b'c', b"ab\x1bcZ"),
            (b'D', b"ab\x1bDZ"),
            (b'E', b"ab\x1bEZ"),
            // RI.
            (b'M', b"ab\r\ncd\x1bMZ"),
        ];
        // `ESC 9` is a final byte no arm claims.
        each_probe_reaches_the_grid("ESC", probes, b'9', 3, 20);
        assert_claims_match_probes(
            "ESC",
            &claimed_byte_literals(&dispatch_body(include_str!("esc.rs"), "esc")),
            probes,
        );
        assert_forwarding_body_claims_nothing("esc_dispatch");
    }

    /// The scraper is itself a dispatch table -- one arm per spelling -- so
    /// it gets the treatment it exists to impose: every spelling it claims
    /// to read is named here with the byte it must yield.
    ///
    /// This exists because I wrote "`b'\\'` and `b'\''` are read outright"
    /// after verifying only the panic, and one of those two was false:
    /// `b'\''` yielded `0x5C` because the plain shape was tested first and
    /// matched on the escape's closing quote. **"Verified by mutation" is a
    /// per-claim property, not a per-finding one** -- a supported claim and
    /// an unsupported one travelled in the same sentence.
    #[test]
    fn the_byte_literal_scraper_reads_every_spelling_it_claims() {
        for (src, want) in [
            ("b'7'", b'7'),
            ("b'\\n'", b'\n'),
            ("b'\\r'", b'\r'),
            ("b'\\t'", b'\t'),
            ("b'\\0'", 0),
            ("b'\\\\'", b'\\'),
            ("b'\\''", b'\''),
            ("0x1b", 0x1b),
        ] {
            let got = claimed_byte_literals(src);
            assert_eq!(
                got.iter().copied().collect::<Vec<u8>>(),
                vec![want],
                "{src} must scrape to {want:#04x}"
            );
        }
    }

    /// The other half of that contract: a spelling it does NOT read must
    /// stop the test rather than silently contribute nothing, because
    /// contributing nothing leaves both sets unchanged and the comparison
    /// passes.
    #[test]
    #[should_panic(expected = "is a spelling this scraper does not read")]
    fn the_byte_literal_scraper_panics_on_a_spelling_it_cannot_read() {
        let _ = claimed_byte_literals("b'\\x1b'");
    }

    /// Splitting the dispatch by face gave every table TWO places it could
    /// be written: the real one in `csi.rs`/`esc.rs`, and the `vte::Perform`
    /// forwarding body in `mod.rs`. The censuses above read only the first,
    /// so an arm added to the second would run at parse time and be invisible
    /// to them -- a second face of one verb with only one of them guarded
    /// (判据 §9). This pins the forwarding bodies as pure delegation: they
    /// must name no byte and no character of their own.
    fn assert_forwarding_body_claims_nothing(method: &str) {
        let body = dispatch_body(include_str!("mod.rs"), method);
        let bytes = claimed_byte_literals(&body);
        let chars = claimed_char_literals(&body);
        assert!(
            bytes.is_empty() && chars.is_empty(),
            "`{method}` in mod.rs must forward and nothing else, but it names \
             bytes {bytes:?} and chars {chars:?}. Put the arm in the file that \
             owns that table (csi.rs / esc.rs), or the census guarding it never \
             sees it."
        );
    }

    /// The body of one dispatch function, comment lines removed -- comments
    /// are prose, not dispatch, and the guards' own paragraphs name verbs
    /// their function does not handle. `\r` goes first so the boundary
    /// still matches on a CRLF checkout.
    ///
    /// Takes the source explicitly because after the dispatch-face split a
    /// table's source is no longer "this file": the CSI table lives in
    /// `csi.rs`, the C0 and ESC tables in `esc.rs`, and `mod.rs` holds only
    /// the forwarding block. Pointing this at the wrong file yields an
    /// empty claimed set, which `assert_claims_match_probes` reports as a
    /// mismatch -- there is no arrangement where a misaimed scrape passes
    /// by seeing nothing.
    fn dispatch_body(src: &str, method: &str) -> String {
        let src = src.replace('\r', "");
        let tail = src
            .split_once(&format!("fn {method}"))
            .unwrap_or_else(|| panic!("no `fn {method}` in this source"))
            .1;
        // The end of the body is the next item in the impl block, whatever
        // its visibility. A separator that knew only the bare `fn` spelling
        // ran straight past `pub(super) fn esc` and folded the ESC table
        // into the C0 one -- silently WIDENING the claimed set, which the
        // set comparison reports but a one-sided "is it non-empty" check
        // never would.
        let opens_next_item = |l: &str| {
            let t = l.trim_start();
            l.starts_with("    ")
                && (t.starts_with("fn ")
                    || t.starts_with("pub fn ")
                    || t.starts_with("pub(super) fn ")
                    || t.starts_with("pub(crate) fn "))
        };
        tail.lines()
            .enumerate()
            .take_while(|(i, l)| *i == 0 || !opens_next_item(l))
            .map(|(_, l)| l)
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every `'X'` character literal. In `csi_dispatch` those are the final
    /// bytes it claims; a `b'?'` byte literal there compares an
    /// intermediate, not a final byte, so it is excluded.
    ///
    /// ⚠️ Lexical, and therefore not total. An arm keyed on a constant
    /// (`SU => …`) or spelled `'\u{53}'` is invisible to it, and no amount
    /// of scanning fixes that — reading a `const`'s value would mean
    /// evaluating the source. So this reads "every verb spelled as a
    /// character literal", not "every verb". Its sibling
    /// [`claimed_byte_literals`] can be made total for its own shape and is;
    /// this one cannot, and says so rather than implying a totality it does
    /// not have.
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
    /// reading -- verified by blinding each one in turn.
    ///
    /// A spelling NOT in that table (`b'\x1b'`, `b'\\'`, `b'\''`) used to
    /// fall through silently, and then both sets were unchanged and
    /// `assert_claims_match_probes` compared two unchanged sets and passed
    /// — a blind spot inside the very guard meant to catch blind spots. Any
    /// unread spelling now **panics** naming the literal, so this function
    /// is total for byte literals: it reads them or it stops the test.
    fn claimed_byte_literals(code: &str) -> std::collections::BTreeSet<u8> {
        let c: Vec<char> = code.chars().collect();
        let mut out = std::collections::BTreeSet::new();
        let mut i = 0;
        while i < c.len() {
            if c[i] == 'b' && c.get(i + 1) == Some(&'\'') {
                // Escape shape FIRST, and the order is load-bearing: in
                // `b'\''` the closing quote of the ESCAPE sits where the
                // plain shape expects its own closing quote, so a
                // plain-first test matches and yields `\` (0x5C) instead of
                // `'` (0x27). Silent, because the escaped character is
                // itself a quote and nothing else looks wrong.
                if c.get(i + 2) == Some(&'\\') && c.get(i + 4) == Some(&'\'') {
                    let byte = match c[i + 3] {
                        'n' => b'\n',
                        'r' => b'\r',
                        't' => b'\t',
                        '0' => 0,
                        '\\' => b'\\',
                        '\'' => b'\'',
                        // Loud, not skipped. Skipping leaves BOTH sets
                        // unchanged, so `assert_claims_match_probes` would
                        // compare two unchanged sets and pass -- the exact
                        // failure these guards exist to catch, hidden inside
                        // the guard itself.
                        other => panic!(
                            "byte-literal escape `b'\\{other}'` is a spelling this \
                             scraper does not read. Teach it here, or the guard goes \
                             blind to the arm that uses it."
                        ),
                    };
                    out.insert(byte);
                    i += 5;
                    continue;
                }
                if c.get(i + 3) == Some(&'\'') && c[i + 2].is_ascii() {
                    out.insert(c[i + 2] as u8);
                    i += 4;
                    continue;
                }
                // `b'` opened and neither shape closed it -- `b'\x1b'` is the
                // realistic one. Same reasoning as the escape arm above:
                // falling through to `i += 1` is silent blindness.
                let tail: String = c[i..(i + 8).min(c.len())].iter().collect();
                panic!(
                    "byte literal starting `{tail}` is a spelling this scraper does \
                     not read. Teach it here, or the guard goes blind to the arm \
                     that uses it."
                );
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

    // ----------------------------------------------------------------
    // 0-A Tier A: stateless sequences (RIS/DECSTR, DECAWM, IND/NEL, REP,
    // legacy alt screen, OSC title rejoin, explicit DCS no-ops).
    //
    // Every one of these is written as "same input, with and without the
    // sequence, `visible_text()` differs". An assertion on the with-side
    // alone cannot tell an implemented arm from a lucky coincidence of the
    // rest of the input; the without-side is what makes the arm the thing
    // being measured.
    // ----------------------------------------------------------------

    /// RIS (`ESC c`) is the full reset: blank grid, cursor home, SGR back to
    /// default, alt screen exited, DECAWM back on, title gone, DECSC slot
    /// dropped.
    ///
    /// **Design call, recorded here and at the code: RIS does NOT clear
    /// scrollback.** xterm does. Aleph's `visible_text()` — the only reader
    /// that decides an agent's state — never looks at scrollback, so
    /// clearing it would buy no consumer anything while throwing away what
    /// the user scrolled back to. The assertion below pins that choice, and
    /// the non-vacuity check above it stops the pin from measuring an empty
    /// ring.
    #[test]
    fn ris_clears_grid_title_and_saved_cursor() {
        let setup: &[u8] = b"one\r\ntwo\r\nthree\r\nfour\x1b]0;t\x07\x1b[31m\x1b7";
        let mut with = Screen::new(3, 10);
        with.feed(setup);
        let mut without = Screen::new(3, 10);
        without.feed(setup);

        let scrollback_before = with.grid.scrollback_len();
        assert!(
            scrollback_before > 0,
            "the setup must actually evict a row, or the scrollback assertion \
             below is measuring an empty ring and would pass either way"
        );
        assert_eq!(with.title(), Some("t"), "setup must set a title to clear");

        with.feed(b"\x1bcZ");
        without.feed(b"Z");
        assert_ne!(
            with.visible_text(),
            without.visible_text(),
            "with RIS the screen is blank and Z lands home; without it Z appends"
        );
        assert_eq!(with.grid.row_text(0), "Z");
        assert_eq!(with.grid.row_text(1), "", "every row is blanked");
        assert_eq!(with.title(), None, "RIS clears the title");
        assert_eq!(
            with.grid.row_cells(0)[0].fg,
            Color::Default,
            "RIS resets SGR, so Z is not still red"
        );
        assert_eq!(
            with.grid.scrollback_len(),
            scrollback_before,
            "RIS deliberately keeps scrollback -- see this test's doc comment"
        );

        with.feed(b"ab\x1b8Y");
        assert_eq!(
            with.grid.row_text(0),
            "ZabY",
            "RIS dropped the DECSC slot, so the DECRC that follows moves nothing"
        );
    }

    /// DECSTR (`CSI ! p`) is the soft variant: the modes go back to their
    /// defaults and the cursor homes, but the cells stay. The `!` is an
    /// intermediate, not a parameter — without reading it this is an
    /// unrelated `CSI p`.
    #[test]
    fn decstr_resets_modes_but_keeps_the_grid() {
        let setup: &[u8] = b"\x1b]0;t\x07\x1b[?7lkeep\x1b[31m\x1b7";
        let mut with = Screen::new(3, 10);
        with.feed(setup);
        let mut without = Screen::new(3, 10);
        without.feed(setup);

        with.feed(b"\x1b[!pZ");
        without.feed(b"Z");
        assert_ne!(with.visible_text(), without.visible_text());
        assert_eq!(
            with.grid.row_text(0),
            "Zeep",
            "DECSTR homed the cursor but left the cells alone"
        );
        assert_eq!(without.grid.row_text(0), "keepZ");
        assert_eq!(
            with.title(),
            Some("t"),
            "a title is not a mode: the soft reset leaves it"
        );
        assert_eq!(
            with.grid.row_cells(0)[0].fg,
            Color::Default,
            "SGR is a mode and does go back to default"
        );
    }

    /// DECAWM off (`CSI ?7 l`) makes the right margin absorb writes instead
    /// of wrapping. The invariant the 0-A survey singles out is asserted
    /// here too: `cursor_col == cols` is this model's ONLY representation of
    /// "a wrap is owed", so with wrapping disabled it must never be entered
    /// — parking there would make the very next `put` wrap after all.
    #[test]
    fn decawm_off_overwrites_the_last_column_instead_of_wrapping() {
        let mut off = Screen::new(3, 5);
        off.feed(b"\x1b[?7labcdefgh");
        let mut on = Screen::new(3, 5);
        on.feed(b"abcdefgh");

        assert_ne!(off.visible_text(), on.visible_text());
        assert_eq!(off.grid.row_text(0), "abcdh", "the last column absorbs f, g, h");
        assert_eq!(off.grid.row_text(1), "", "nothing wrapped onto the next row");
        assert_eq!(on.grid.row_text(0), "abcde", "with DECAWM on it wraps");
        assert_eq!(on.grid.row_text(1), "fgh");

        let (_, col) = off.grid.cursor();
        let (_, cols) = off.grid.dims();
        assert!(
            col < cols,
            "DECAWM off must never park the cursor at `cols` -- that value is \
             the model's 'a wrap is owed' state, and no wrap is ever owed here"
        );

        off.feed(b"\x1b[?7hij");
        assert_eq!(off.grid.row_text(1), "j", "re-enabling DECAWM wraps again");
    }

    /// IND (`ESC D`) moves down keeping the column; NEL (`ESC E`) moves down
    /// and returns to column zero. Asserting both against each other as well
    /// as against neither is what stops one arm from standing in for the
    /// other: a version that implemented IND as NEL passes an IND-only test.
    #[test]
    fn ind_moves_down_same_column_nel_moves_down_col_zero() {
        let mut ind = Screen::new(3, 10);
        ind.feed(b"abc\x1bDX");
        let mut nel = Screen::new(3, 10);
        nel.feed(b"abc\x1bEX");
        let mut without = Screen::new(3, 10);
        without.feed(b"abcX");

        assert_ne!(ind.visible_text(), without.visible_text());
        assert_ne!(nel.visible_text(), without.visible_text());
        assert_ne!(
            ind.visible_text(),
            nel.visible_text(),
            "IND keeps the column, NEL does not -- implementing one as the \
             other would pass a test that only checked 'it moved down'"
        );
        assert_eq!(ind.grid.row_text(1), "   X");
        assert_eq!(nel.grid.row_text(1), "X");
        assert_eq!(without.grid.row_text(0), "abcX");
    }

    /// REP (`CSI Ps b`) repeats the last PRINTED character. "Printed" is the
    /// whole difficulty: a control byte or another CSI in between means
    /// there is no candidate any more, and a version that repeated "the last
    /// character seen" would emit an escape's final byte instead.
    #[test]
    fn rep_repeats_the_last_printed_char_and_a_control_byte_invalidates_it() {
        let mut with = Screen::new(2, 10);
        with.feed(b"-\x1b[4b");
        let mut without = Screen::new(2, 10);
        without.feed(b"-");
        assert_ne!(with.visible_text(), without.visible_text());
        assert_eq!(with.grid.row_text(0), "-----", "one printed plus four repeats");

        let mut after_control = Screen::new(2, 10);
        after_control.feed(b"-\x08\x1b[4b");
        assert_eq!(
            after_control.grid.row_text(0),
            "-",
            "BS is a control byte: it clears the repeat candidate"
        );

        let mut after_csi = Screen::new(2, 10);
        after_csi.feed(b"-\x1b[31m\x1b[4b");
        assert_eq!(
            after_csi.grid.row_text(0),
            "-",
            "an intervening CSI clears it too"
        );

        let mut twice = Screen::new(2, 10);
        twice.feed(b"-\x1b[2b\x1b[2b");
        assert_eq!(
            twice.grid.row_text(0),
            "-----",
            "REP's own writes are prints, so a second REP still has a candidate"
        );
    }

    /// Modes 47 and 1047 are the legacy alternate screen. The semantic
    /// difference that matters is on the buffer, not the switch: 1049 clears
    /// the alternate buffer every time it enters, 47/1047 do not, so a
    /// program that leaves and re-enters through the legacy pair expects to
    /// find its screen still there.
    #[test]
    fn legacy_alt_screen_47_and_1047_keep_the_alt_grid_across_exit() {
        for (enter, exit) in [
            (&b"\x1b[?47h"[..], &b"\x1b[?47l"[..]),
            (&b"\x1b[?1047h"[..], &b"\x1b[?1047l"[..]),
        ] {
            let mut with = Screen::new(3, 10);
            with.feed(b"primary");
            with.feed(enter);
            assert!(
                with.alt_screen(),
                "the legacy modes must enter the alternate screen, not fall \
                 through to the catch-all"
            );
            with.feed(b"alt");
            with.feed(exit);
            assert_eq!(
                with.grid.row_text(0),
                "primary",
                "exiting restores the primary buffer"
            );
            with.feed(enter);
            assert_eq!(
                with.grid.row_text(0),
                "alt",
                "the legacy alternate buffer survives the round trip"
            );

            let mut without = Screen::new(3, 10);
            without.feed(b"primary");
            without.feed(b"alt");
            assert_ne!(with.visible_text(), without.visible_text());
        }

        let mut modern = Screen::new(3, 10);
        modern.feed(b"primary\x1b[?1049halt\x1b[?1049l");
        assert_eq!(modern.grid.row_text(0), "primary");
        modern.feed(b"\x1b[?1049h");
        assert_eq!(
            modern.grid.row_text(0),
            "",
            "1049 is the contrast: it clears the alternate buffer on entry"
        );
    }

    /// Mode 1048 is DECSC/DECRC under another spelling, and it shares the one
    /// saved-cursor slot with `ESC 7`/`ESC 8` deliberately: a second slot
    /// would have no producer, and two slots would let a save through one
    /// spelling be restored as nothing through the other.
    #[test]
    fn mode_1048_saves_and_restores_the_cursor() {
        let mut with = Screen::new(3, 10);
        with.feed(b"ab\x1b[?1048hcd\x1b[?1048lZ");
        let mut without = Screen::new(3, 10);
        without.feed(b"abcdZ");

        assert_ne!(with.visible_text(), without.visible_text());
        assert_eq!(with.grid.row_text(0), "abZd", "the restore put the cursor back at column 2");
        assert_eq!(without.grid.row_text(0), "abcdZ");
    }

    /// `vte` splits an OSC payload on every `;`, so reading `params[1]` alone
    /// truncates a title at its first semicolon. `retain_osc_progress` already
    /// rejoins `params[1..]` for exactly this reason; the title path had the
    /// other, wrong shape of the same code sitting beside it.
    #[test]
    fn osc_title_with_semicolons_is_rejoined() {
        let mut s = Screen::new(2, 30);
        s.feed(b"\x1b]0;a;b;c\x07");
        assert_eq!(
            s.title(),
            Some("a;b;c"),
            "a truncating reader would report just `a` -- and `a` is a \
             plausible-looking title, so nothing downstream would notice"
        );

        let mut two = Screen::new(2, 30);
        two.feed(b"\x1b]2;dir - cmd; arg\x07");
        assert_eq!(two.title(), Some("dir - cmd; arg"), "OSC 2 shares the path");
    }

    /// DCS is swallowed by `vte`'s own state machine: `advance_dcs_passthrough`
    /// routes payload bytes to `put()` and never to `print()`, so the payload
    /// cannot reach the grid. The three methods are implemented explicitly
    /// anyway, because "no branch" and "a branch that deliberately does
    /// nothing" read identically at the call site and only one of them is a
    /// decision.
    ///
    /// Falsifiable: implement `Perform::put` as `self.print(char::from(byte))`
    /// — the plausible wrong version — and the DECRQSS payload `1$r0m` paints
    /// onto row 0 and this test fails on the first assertion.
    #[test]
    fn dcs_hook_put_unhook_are_explicit_no_ops() {
        let mut with = Screen::new(3, 10);
        with.feed(b"ab\x1bP1$r0m\x1b\\cd");
        let mut without = Screen::new(3, 10);
        without.feed(b"abcd");

        assert_eq!(
            with.visible_text(),
            without.visible_text(),
            "a DCS payload must not paint; the bytes after ST must still land"
        );
        assert_eq!(with.grid.row_text(0), "abcd");
    }

    // ----------------------------------------------------------------
    // 0-A A2 + B5: scroll regions (DECSTBM / SU / SD / RI) and origin mode.
    //
    // A2 and B5 ship together on purpose. DECOM means nothing without a
    // region, and a region without DECOM is half-right: a program that sets
    // both and then addresses `CSI 1;1H` lands at the top of the SCREEN
    // instead of the top of its region, and every row it paints afterwards
    // is offset against what it believes it drew.
    // ----------------------------------------------------------------

    /// The shape this whole feature exists for: a program pins a header and
    /// a footer and scrolls only the middle (`vim`, `less`, `tmux`, anything
    /// that sends `CSI 2;23r`). Scrolling the pinned rows marches the header
    /// off the top while it is still plainly on the user's real terminal --
    /// and it is the status line the detection manifests match on.
    #[test]
    fn decstbm_scrolls_only_the_region_and_pins_the_header() {
        let setup: &[u8] = b"header\r\none\r\ntwo\r\nthree\r\nfooter";
        let mut with = Screen::new(5, 10);
        with.feed(setup);
        // Region = rows 2..=4 one-based, i.e. the three middle rows.
        with.feed(b"\x1b[2;4r");
        with.feed(b"\x1b[4;1H\nnew");

        let mut without = Screen::new(5, 10);
        without.feed(setup);
        without.feed(b"\x1b[4;1H\nnew");

        assert_ne!(with.visible_text(), without.visible_text());
        assert_eq!(with.grid.row_text(0), "header", "the pinned header does not move");
        assert_eq!(with.grid.row_text(4), "footer", "nor does the pinned footer");
        assert_eq!(with.grid.row_text(1), "two", "the region scrolled by one");
        assert_eq!(with.grid.row_text(2), "three");
        assert_eq!(with.grid.row_text(3), "new");
    }

    /// Scrollback is what fell off the TOP OF THE SCREEN. A row leaving the
    /// top of a region never reached the top of the screen, so filing it
    /// would let a client scrolling back read rows the user never saw leave
    /// -- the same reasoning `insert_lines` already carries for rows pushed
    /// off the bottom.
    #[test]
    fn rows_leaving_a_region_top_do_not_enter_scrollback() {
        let mut region = Screen::new(4, 10);
        region.feed(b"a\r\nb\r\nc\r\nd");
        assert_eq!(region.grid.scrollback_len(), 0, "the setup must not scroll on its own");
        region.feed(b"\x1b[2;4r\x1b[4;1H\n\n\n");
        assert_eq!(
            region.grid.scrollback_len(),
            0,
            "three scrolls inside a region file nothing"
        );

        let mut full = Screen::new(4, 10);
        full.feed(b"a\r\nb\r\nc\r\nd\n\n\n");
        assert_eq!(
            full.grid.scrollback_len(),
            3,
            "the contrast: a full-height scroll still files every row it evicts \
             -- without this the assertion above would pass on a screen that \
             simply never scrolls"
        );
    }

    /// RI (`ESC M`) at the top of the region opens a blank row there and
    /// pushes the region down. Without it, a program scrolling backwards
    /// gets nothing: the top of the screen keeps stale text forever while
    /// the program believes it revealed new content.
    #[test]
    fn reverse_index_at_region_top_scrolls_down() {
        let setup: &[u8] = b"header\r\none\r\ntwo\r\nthree";
        let mut with = Screen::new(4, 10);
        with.feed(setup);
        with.feed(b"\x1b[2;4r\x1b[2;1H\x1bMX");

        let mut without = Screen::new(4, 10);
        without.feed(setup);
        without.feed(b"\x1b[2;4r\x1b[2;1HX");

        assert_ne!(with.visible_text(), without.visible_text());
        assert_eq!(with.grid.row_text(0), "header", "the row above the region is untouched");
        assert_eq!(with.grid.row_text(1), "X", "a blank row opened at the region top");
        assert_eq!(with.grid.row_text(2), "one");
        assert_eq!(with.grid.row_text(3), "two");
        assert_eq!(
            with.grid.scrollback_len(),
            0,
            "the row pushed off the region bottom is discarded, not filed -- it \
             left downwards, and scrollback only ever means 'off the top'"
        );

        // Away from the region top, RI is a plain move up.
        let mut inside = Screen::new(4, 10);
        inside.feed(setup);
        inside.feed(b"\x1b[2;4r\x1b[3;1H\x1bMY");
        assert_eq!(inside.grid.row_text(1), "Yne", "no scroll: the cursor just moved up");
    }

    /// SU/SD scroll the region without moving the cursor, which is what
    /// separates them from IND/RI.
    #[test]
    fn su_and_sd_scroll_within_the_region() {
        let setup: &[u8] = b"header\r\none\r\ntwo\r\nthree";
        let mut without = Screen::new(4, 10);
        without.feed(setup);

        let mut su = Screen::new(4, 10);
        su.feed(setup);
        su.feed(b"\x1b[2;4r\x1b[2S");
        assert_ne!(su.visible_text(), without.visible_text());
        assert_eq!(su.grid.row_text(0), "header");
        assert_eq!(su.grid.row_text(1), "three");
        assert_eq!(su.grid.row_text(2), "");
        assert_eq!(su.grid.row_text(3), "");

        let mut sd = Screen::new(4, 10);
        sd.feed(setup);
        sd.feed(b"\x1b[2;4r\x1b[1T");
        assert_ne!(sd.visible_text(), without.visible_text());
        assert_eq!(sd.grid.row_text(0), "header");
        assert_eq!(sd.grid.row_text(1), "");
        assert_eq!(sd.grid.row_text(2), "one");
        assert_eq!(sd.grid.row_text(3), "two");
    }

    /// DECOM (`CSI ?6 h`) makes CUP's row 1 mean the region's top row, and
    /// clamps at the region's bottom rather than the screen's. Both halves
    /// are asserted: a version that added the offset but clamped to the
    /// screen still puts text outside the region the program reserved.
    #[test]
    fn origin_mode_makes_cup_relative_to_the_region() {
        let setup: &[u8] = b"\x1b[3;5r";
        let mut with = Screen::new(6, 10);
        with.feed(setup);
        with.feed(b"\x1b[?6h\x1b[1;1HX");

        let mut without = Screen::new(6, 10);
        without.feed(setup);
        without.feed(b"\x1b[1;1HX");

        assert_ne!(with.visible_text(), without.visible_text());
        assert_eq!(with.grid.row_text(2), "X", "row 1 is the region's top row");
        assert_eq!(with.grid.row_text(0), "", "and the screen's top is out of reach");
        assert_eq!(without.grid.row_text(0), "X", "without DECOM, row 1 is the screen's top");

        with.feed(b"\x1b[9;1HY");
        assert_eq!(with.grid.row_text(4), "Y", "DECOM clamps at the region's bottom");
        assert_eq!(with.grid.row_text(5), "", "not at the screen's");
    }

    /// A region is stated in rows, so it cannot outlive a change to how many
    /// rows there are. RIS resets it for the same reason it resets every
    /// other mode.
    #[test]
    fn resize_and_ris_reset_the_region() {
        let mut resized = Screen::new(4, 10);
        resized.feed(b"a\r\nb\r\nc\r\nd\x1b[2;3r");
        resized.resize(5, 10);
        resized.feed(b"\x1b[5;1H\nZ");
        assert_eq!(
            resized.grid.scrollback_len(),
            1,
            "after the resize the region is the whole screen again, so the \
             scroll evicts a row -- a stale (2,3) region would have scrolled \
             two rows in the middle and filed nothing"
        );

        let mut after_ris = Screen::new(4, 10);
        after_ris.feed(b"\x1b[2;3r\x1bc");
        after_ris.feed(b"a\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(after_ris.grid.scrollback_len(), 1, "RIS put the region back");
        assert_eq!(after_ris.grid.row_text(3), "e");

        let mut still_set = Screen::new(4, 10);
        still_set.feed(b"\x1b[2;3r");
        still_set.feed(b"a\r\nb\r\nc\r\nd\r\ne");
        assert_eq!(
            still_set.grid.scrollback_len(),
            0,
            "the contrast: a region still in force files nothing, which is \
             what makes the two assertions above measure the reset and not \
             the grid's height"
        );
    }

    /// IL/DL are region-bounded: rows below the region's bottom must not be
    /// pushed down or pulled up, or the footer a program pinned moves anyway
    /// through a different verb than the one A2 was written for.
    #[test]
    fn insert_and_delete_lines_respect_the_region() {
        let setup: &[u8] = b"header\r\none\r\ntwo\r\nthree\r\nfooter";

        let mut il = Screen::new(5, 10);
        il.feed(setup);
        il.feed(b"\x1b[2;4r\x1b[2;1H\x1b[1L");
        assert_eq!(il.grid.row_text(0), "header");
        assert_eq!(il.grid.row_text(1), "", "a blank row inserted at the region top");
        assert_eq!(il.grid.row_text(2), "one");
        assert_eq!(il.grid.row_text(3), "two");
        assert_eq!(
            il.grid.row_text(4),
            "footer",
            "`three` fell off the region's bottom; the footer below it did not move"
        );

        let mut dl = Screen::new(5, 10);
        dl.feed(setup);
        dl.feed(b"\x1b[2;4r\x1b[2;1H\x1b[1M");
        assert_eq!(dl.grid.row_text(0), "header");
        assert_eq!(dl.grid.row_text(1), "two");
        assert_eq!(dl.grid.row_text(2), "three");
        assert_eq!(
            dl.grid.row_text(3),
            "",
            "the region's last row is blanked, not filled from below the region"
        );
        assert_eq!(dl.grid.row_text(4), "footer");

        let mut without = Screen::new(5, 10);
        without.feed(setup);
        without.feed(b"\x1b[2;1H\x1b[1L");
        assert_ne!(il.visible_text(), without.visible_text());
    }

    /// The one verb in this task that must NOT read the region.
    ///
    /// `CSI J` erases the DISPLAY. DEC defines its three modes against the
    /// screen, and a scrolling region constrains scrolling, not erasure --
    /// so "respect the region" is the wrong answer here even though it is
    /// the right answer four lines up in the same dispatch table. A version
    /// that clipped ED to the region leaves a header standing that the
    /// program believes it erased, and stale text a manifest can still match
    /// is the exact failure this round exists to remove: a wrong label is
    /// dearer than a missing one.
    #[test]
    fn erase_in_display_within_a_region_is_still_screen_absolute() {
        let mut all = Screen::new(4, 10);
        all.feed(b"header\r\none\r\ntwo\r\nthree");
        all.feed(b"\x1b[2;4r\x1b[2J");
        assert_eq!(
            all.visible_text(),
            "\n\n\n",
            "`CSI 2 J` clears every row, including the two outside the region"
        );

        let mut partial = Screen::new(4, 10);
        partial.feed(b"header\r\none\r\ntwo\r\nthree");
        partial.feed(b"\x1b[2;4r\x1b[3;10H\x1b[1J");
        assert_eq!(
            partial.grid.row_text(0),
            "",
            "erase-to-cursor reaches above the region's top row"
        );
        assert_eq!(partial.grid.row_text(1), "");
        assert_eq!(partial.grid.row_text(2), "");
        assert_eq!(partial.grid.row_text(3), "three", "and stops at the cursor");
    }

    // ----------------------------------------------------------------
    // 0-A C9 / C5 / B1: the three observable bits the client cannot derive.
    //
    // All three are server-side facts by construction: the mode is set by a
    // sequence only the emulator sees, so a client that is not told cannot
    // work them out. Each rides `take_patch` under the same "Some only when
    // changed" rule `alt_screen` and `title` already follow, and each is
    // carried WHOLE by `full_patch` so a client attaching late gets the
    // current value rather than the last change it happened to miss.
    // ----------------------------------------------------------------

    /// DECTCEM (`CSI ?25 h/l`). A patch that repeated the level every frame
    /// would ship it 62 times a second per session for no news; a patch that
    /// never shipped it would leave the Panel drawing a cursor the program
    /// asked to hide. `Some` exactly on the change is the only shape that is
    /// neither.
    #[test]
    fn cursor_visibility_rides_the_patch_only_when_it_changes() {
        let mut s = Screen::new(3, 10);
        let first = s
            .take_patch()
            .expect("a fresh screen announces its initial mode bits");
        assert_eq!(
            first.cursor_visible,
            Some(true),
            "the default is visible, and a client has to be told what it is \
             rather than assuming"
        );

        s.feed(b"\x1b[?25l");
        assert_eq!(
            s.take_patch().expect("hiding the cursor is news").cursor_visible,
            Some(false)
        );
        assert!(!s.cursor_visible());

        s.feed(b"\x1b[?25l");
        assert!(
            s.take_patch().is_none(),
            "the same mode set twice is not news"
        );

        s.feed(b"x");
        assert_eq!(
            s.take_patch().expect("printing is news").cursor_visible,
            None,
            "unchanged means ABSENT, not `Some(false)` on every frame"
        );

        s.feed(b"\x1b[?25h");
        assert_eq!(s.take_patch().expect("showing again").cursor_visible, Some(true));
        assert!(s.cursor_visible());
    }

    /// Mode 2004. The Panel needs this to decide whether a paste is wrapped
    /// in `ESC[200~ … ESC[201~`, and it is observable only here — the
    /// sequence never reaches the client.
    #[test]
    fn bracketed_paste_mode_rides_the_patch() {
        let mut s = Screen::new(3, 10);
        assert_eq!(
            s.take_patch().expect("first patch").bracketed_paste,
            Some(false),
            "off by default -- and a client with no value must assume the \
             weakest thing, which is what makes the default worth publishing"
        );
        assert!(!s.bracketed_paste());

        s.feed(b"\x1b[?2004h");
        assert_eq!(
            s.take_patch().expect("enabling is news").bracketed_paste,
            Some(true)
        );
        assert!(s.bracketed_paste());

        s.feed(b"y");
        assert_eq!(
            s.take_patch().expect("printing is news").bracketed_paste,
            None,
            "unchanged means absent"
        );

        s.feed(b"\x1b[?2004l");
        assert_eq!(
            s.take_patch().expect("disabling is news").bracketed_paste,
            Some(false)
        );
    }

    /// OSC 7 is the cheap half of live cwd: it lands entirely inside
    /// `screen/` and touches no platform API, so it does not carry the R1
    /// decision that PID probing does. It is a SUPPLEMENT to the spawn
    /// directory, not a replacement — only shells with integration
    /// configured emit it.
    #[test]
    fn osc7_file_uri_with_empty_or_localhost_host_sets_cwd_and_percent_decodes() {
        let mut empty_host = Screen::new(3, 10);
        empty_host.feed(b"\x1b]7;file:///home/me/src\x1b\\");
        assert_eq!(empty_host.cwd(), Some("/home/me/src"));

        let mut local = Screen::new(3, 10);
        local.feed(b"\x1b]7;file://localhost/home/me\x07");
        assert_eq!(
            local.cwd(),
            Some("/home/me"),
            "`localhost` names this machine as surely as an empty host does"
        );

        let mut encoded = Screen::new(3, 10);
        encoded.feed("\x1b]7;file:///home/me/a%20b/%E4%B8%AD\x07".as_bytes());
        assert_eq!(
            encoded.cwd(),
            Some("/home/me/a b/\u{4e2d}"),
            "one non-ASCII character arrives as THREE escapes, so decoding has \
             to accumulate bytes and convert once -- decoding escape by escape \
             yields mojibake for every non-ASCII path"
        );

        let mut with_semicolon = Screen::new(3, 10);
        with_semicolon.feed(b"\x1b]7;file:///tmp/a;b\x07");
        assert_eq!(
            with_semicolon.cwd(),
            Some("/tmp/a;b"),
            "`vte` splits the payload on `;`, so this path arrives in pieces \
             and has to be rejoined -- the same trap the title arm had"
        );

        let mut patched = Screen::new(3, 10);
        let _ = patched.take_patch();
        patched.feed(b"\x1b]7;file:///tmp\x07");
        assert_eq!(
            patched.take_patch().expect("a new cwd is news").cwd.as_deref(),
            Some("/tmp")
        );
        patched.feed(b"z");
        assert_eq!(
            patched.take_patch().expect("printing is news").cwd,
            None,
            "unchanged means absent, same rule as the two bits above"
        );
    }

    /// A `file://` URI naming another machine describes another machine's
    /// filesystem. Dropping it leaves the previous answer standing, which is
    /// the only honest outcome: a rejected value has the standing to say "I
    /// don't know", never to supply one (判据 §8).
    #[test]
    fn osc7_with_a_foreign_host_is_dropped() {
        let mut s = Screen::new(3, 10);
        s.feed(b"\x1b]7;file:///home/me\x07");
        assert_eq!(
            s.cwd(),
            Some("/home/me"),
            "the setup must land, or the assertion below cannot tell 'dropped' \
             from 'this never worked at all'"
        );

        s.feed(b"\x1b]7;file://build-box/srv/other\x07");
        assert_eq!(
            s.cwd(),
            Some("/home/me"),
            "a foreign host leaves the last known-good value rather than \
             overwriting it with a path that is not this session's"
        );

        let mut never = Screen::new(3, 10);
        never.feed(b"\x1b]7;file://build-box/srv/other\x07");
        assert_eq!(
            never.cwd(),
            None,
            "with nothing known before, it stays unknown -- 'unknown' must not \
             be filled in with the remote path"
        );

        let mut bare_path = Screen::new(3, 10);
        bare_path.feed(b"\x1b]7;/home/me\x07");
        assert_eq!(
            bare_path.cwd(),
            None,
            "OSC 7 is defined as a file:// URI; a bare path is not one, and \
             guessing that it meant one is how a scheme nobody checked becomes \
             a path somebody trusts"
        );
    }

    /// A client attaching after the changes have already been shipped to a
    /// live client gets them from `full_patch`, not from the diff stream it
    /// was not there for.
    ///
    /// This is exactly the pair of calls `PtySession::attach_snapshot` makes
    /// (`convert::patch(&screen.full_patch())`), so the whole snapshot path
    /// for these three fields lives inside `screen/` and needs no wiring
    /// elsewhere. The wire half is asserted below for that reason: a
    /// `full_patch` that carried the values into a `convert::patch` that
    /// dropped them would pass the first three assertions.
    #[test]
    fn attach_snapshot_carries_the_current_mode_bits() {
        let mut s = Screen::new(3, 10);
        s.feed(b"\x1b[?25l\x1b[?2004h\x1b]7;file:///srv\x07");

        let _ = s.take_patch();
        assert!(
            s.take_patch().is_none(),
            "the diff stream must be drained, or this test proves nothing \
             about a LATE attach"
        );

        let snapshot = s.full_patch();
        assert_eq!(snapshot.cursor_visible, Some(false));
        assert_eq!(snapshot.bracketed_paste, Some(true));
        assert_eq!(snapshot.cwd.as_deref(), Some("/srv"));

        let wire = crate::gateway::pty::screen::convert::patch(&snapshot);
        assert_eq!(wire.cursor_visible, Some(false));
        assert_eq!(wire.bracketed_paste, Some(true));
        assert_eq!(wire.cwd.as_deref(), Some("/srv"));
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

        // The pending-wrap state: exactly filling a row leaves
        // `cursor_col == cols`, which is the model's only representation of
        // "a wrap is owed". The new C0 arithmetic all depends on
        // `cursor_col <= cols`, and nothing else exercises the `== cols`
        // end of it. HT from there cancels the pending wrap and sits on the
        // last column, which is what xterm does.
        let mut s = Screen::new(2, 10);
        s.feed(b"abcdefghij");
        assert_eq!(s.grid.cursor(), (0, 10), "a full row leaves a wrap pending");
        s.feed(b"\t");
        assert_eq!(
            s.grid.cursor(),
            (0, 9),
            "HT out of a pending wrap lands on the last column, not the next row"
        );
    }

    /// Checklist item #4 of this plan's real-machine list is a tab-aligned
    /// CJK table, pinned only by a one-time live reading. It claims two
    /// hazards: HT dropped, and a wide glyph counted as one column.
    ///
    /// **The width count is why the input is four glyphs and the stop is
    /// 16.** Stops are every 8 columns, so a miscount only shows up when
    /// the true and miscounted widths fall either side of a multiple of 8.
    /// Four wide glyphs occupy 8 columns and tab to 16; miscounted as one
    /// column each they occupy 4 and tab to 8 — different stops, so the
    /// assertion separates them. With TWO glyphs (the shape this test had
    /// first) the counts are 4 and 2, both of which floor-divide to stop 8:
    /// the `cursor_col += w` → `+= 1` mutation ran and changed nothing, and
    /// the test read as coverage it did not have.
    #[test]
    fn ht_aligns_a_column_across_glyphs_of_different_widths() {
        let mut s = Screen::new(4, 24);
        s.feed("abcdefgh\tX\r\n".as_bytes()); // 8 narrow columns
        s.feed("\u{4e2d}\u{6587}\u{8868}\u{683c}\tX".as_bytes()); // 4 wide = 8 columns

        // Column, not string index: `row_text` drops spacers, so the wide
        // row's `X` sits at index 5 there while standing in column 16.
        let column_of_x = |row: u16| {
            s.grid
                .row_cells(row)
                .iter()
                .position(|c| c.ch == 'X')
                .expect("both rows print an X")
        };
        assert_eq!(
            column_of_x(0),
            16,
            "eight narrow columns tab to the stop at 16"
        );
        assert_eq!(
            column_of_x(1),
            16,
            "four wide glyphs are also eight columns and reach the same stop"
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

    /// `title` is fed by whatever the shell prints, cloned into every patch
    /// and serialized to the wire -- so the bound on it is load-bearing, and
    /// it is not ours. `vte::Parser` accumulates OSC payload into an
    /// `ArrayVec<u8, 1024>` under `feature = "no_std"` and into an unbounded
    /// `Vec<u8>` without it; the `is_full()` back-pressure checks are
    /// `#[cfg(feature = "no_std")]`. `no_std` is in vte's DEFAULT feature set
    /// and this crate takes defaults, so today it holds -- measured, not
    /// assumed: 64 MiB of unterminated OSC payload leaves `title` at 1023
    /// bytes.
    ///
    /// This test exists because that dependency is invisible at every place
    /// someone would look. A future `vte = { version = "0.14",
    /// default-features = false }`, written for an unrelated reason, would
    /// silently turn `title` into an unbounded accumulator driven by remote
    /// output, and nothing in this file mentions the feature. Here it fails
    /// by name instead.
    ///
    /// 64 KiB rather than the 64 MiB used to characterise it: the assertion
    /// only has to separate ~1 KiB from "as much as we fed", and a unit test
    /// should not allocate 64 MiB to say so.
    #[test]
    fn an_unterminated_osc_title_cannot_grow_without_bound() {
        const FED: usize = 64 * 1024;
        let mut s = Screen::new(4, 20);
        let mut bytes = b"\x1b]0;".to_vec();
        bytes.extend(std::iter::repeat_n(b'A', FED));
        bytes.push(0x07);
        s.feed(&bytes);

        let len = s.state.title.as_ref().map_or(0, String::len);
        // Non-vacuity: an unset title also has length 0, so without this the
        // bound below passes for a parser that dropped the OSC entirely --
        // the assertion would be measuring nothing while reading green.
        assert!(
            len > 0,
            "OSC title never arrived, so the bound below is measuring nothing"
        );
        assert!(
            len <= 1024,
            "OSC title accumulator is unbounded -- vte's `no_std` feature was \
             almost certainly dropped from this crate's dependency, which \
             removes the `ArrayVec<u8, 1024>` cap and the `is_full()` \
             back-pressure. `title` is driven by remote output and copied \
             into every patch. Fed {FED} bytes, title kept {len}"
        );
    }
