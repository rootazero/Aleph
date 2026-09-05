//! Grid tests. Moved out of the production file by the responsibility
//! split; they exercise `Grid` through its public surface, so they did
//! not have to follow any particular verb into its new file.

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
    assert_eq!(
        g.cursor(),
        (0, 2),
        "a wide glyph advances the cursor by two"
    );
    assert_eq!(
        g.row_text(0),
        "中",
        "the spacer cell must not surface as a char"
    );
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
    assert!(
        !cells[1].is_spacer(),
        "column 1 must not be left as a dangling spacer"
    );
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
    assert_eq!(
        cells[2].ch, ' ',
        "the orphaned owner must be blanked, not left wide with no spacer"
    );
    assert_eq!(cells[3].ch, 'x');
}

/// The same scenario as above, reached through `Grid`'s public API instead
/// of by writing the private fields: `goto` can land the cursor directly
/// on a spacer, and a `put` there must repair the orphaned owner.
///
/// Scope, stated because the earlier wording overclaimed it: this test
/// starts at `Grid`, one layer BELOW the parser. It does not prove that a
/// real `CSI 1;4H` reaches `goto` — real terminal output travels
/// vte → `Perform` → `Grid`, and this begins at the last of those. That
/// half is proved separately by
/// `super::super::perform::tests::goto_onto_a_spacer_then_printing_repairs_the_owner`,
/// which feeds the actual escape sequence over real CJK input. The three
/// tests are one ladder — invariant, `Grid` API, parser — and each rung is
/// worth having only as long as nobody reads one as covering another.
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
    assert_eq!(
        cells[2].ch, ' ',
        "the orphaned owner must be blanked, not left wide with no spacer"
    );
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

    assert_eq!(
        g.row_text(0),
        "row1!",
        "row0 scrolled off the top, row1 moved up"
    );
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
    assert_eq!(
        g.cursor(),
        (1, 4),
        "cursor is clamped into the smaller grid"
    );
}

#[test]
fn resize_to_the_same_dimensions_is_a_cheap_no_op() {
    let mut g = Grid::new(3, 10);
    g.put('x', PLAIN);
    g.resize(3, 10);
    assert_eq!(
        g.row_text(0),
        "x",
        "content is untouched by a same-size resize"
    );
}

/// The `w > self.cols` early return in `put`, which had no test.
///
/// It is the only thing between a one-column grid and an out-of-bounds
/// index in the PTY reader thread, and one column is reachable from the
/// wire: `handlers::pty::check_dimensions` bounds only the UPPER end
/// (`> MAX_TERMINAL_DIMENSION`), so a client may ask for `cols: 1`, and
/// `cols: 0` — the wire's "unset" — arrives here as 1 through
/// `Grid::new`'s `max(1)`. Both are covered below, because they are two
/// ways to reach the same grid and only one of them looks deliberate.
///
/// Reddens by PANIC, not by assertion, if the early return is deleted:
/// the wrap arm then moves the cursor to column 0 of the next row and the
/// `w == 2` spacer write indexes column 1 of a one-column row.
#[test]
fn a_wide_glyph_on_a_one_column_grid_is_dropped_rather_than_indexed() {
    for cols in [1_u16, 0] {
        let mut g = Grid::new(2, cols);
        g.put('中', PLAIN);
        assert_eq!(
            g.row_text(0),
            "",
            "a glyph that cannot fit must not be written (cols={cols})"
        );
        assert_eq!(
            g.row_text(1),
            "",
            "and it must not have wrapped onto the next row (cols={cols})"
        );
        assert_eq!(
            g.cursor(),
            (0, 0),
            "a dropped glyph advances nothing (cols={cols})"
        );

        // The grid is still usable afterwards: dropping the glyph is a
        // no-op, not a poisoned state.
        g.put('a', PLAIN);
        assert_eq!(g.row_text(0), "a", "cols={cols}");
    }
}
