//! `vte::Perform` implementation — turns the PTY byte stream into grid writes.
//!
//! Split by dispatch face: the `vte::Perform` impl below is a uniform
//! forwarding block, and the real work lives one file per face --
//! [`csi`], [`esc`] (which also owns the C0 table) and [`osc`]. The
//! forwarding is deliberately verb-free so the dispatch-table censuses in
//! `tests.rs` have exactly one place to read each table from; they assert
//! that the bodies here claim nothing, which is what keeps this file from
//! becoming a second face of the same dispatch (判据 §9).

use super::grid::{Attrs, Color, Grid};

/// The parser plus the state it mutates. `Parser` is retained across `feed`
/// calls because escape sequences straddle read boundaries: an OSC title can
/// arrive in two chunks, and a parser rebuilt per read would lose the tail.
pub struct Screen {
    pub grid: Grid,
    /// The saved primary screen while the alternate screen is active.
    saved: Option<Grid>,
    /// The alternate buffer while the PRIMARY screen is active -- the other
    /// half of the same one-buffer model as [`Self::saved`].
    ///
    /// There is one alternate buffer, not one per mode. 47 and 1047 leave it
    /// alone on entry, so a program that exits and re-enters through them
    /// finds its screen still there; 1049 clears it on entry, which is the
    /// only semantic difference between the two spellings. Parking it here
    /// on exit rather than dropping it is what makes the legacy pair's
    /// promise true, and 1049 replacing it on entry is what keeps 1049's.
    retained_alt: Option<Grid>,
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
    /// Latest ConEmu `OSC 9;4` progress payload -- everything after `9;`,
    /// e.g. `"4;3;"` or `"4;0;0"`. That is the exact shape the detection
    /// manifests' `osc_progress` region regexes are written against
    /// (`crates/agent-detect/src/manifests/grok.toml` matches `^4;1;-1$`),
    /// so this field's format is owned by those rules, not chosen here.
    ///
    /// A LEVEL, not an edge: a program sets it and leaves it until it sets
    /// something else, and the manifests are written for exactly that --
    /// Claude's rules assume `4;3` stays painted while it waits for
    /// permission, which is why no rule reads `4;3` alone as working.
    osc_progress: Option<String>,
    bell: bool,
}

impl Screen {
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            grid: Grid::new(rows, cols),
            saved: None,
            retained_alt: None,
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

    /// The latest ConEmu progress payload, or `None` when the program has
    /// never reported one.
    ///
    /// `None` means "this program has told me nothing", never "there is no
    /// progress" (判据 §8). The detection engine spells the same absence as
    /// an empty string, so `unwrap_or_default()` at the call site is the
    /// faithful conversion, not a shortcut.
    #[must_use]
    pub fn osc_progress(&self) -> Option<&str> {
        self.state.osc_progress.as_deref()
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

mod csi;
mod esc;
mod osc;

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

    /// DECSC's write half, shared by `ESC 7` and private mode 1048.
    ///
    /// One slot for both spellings on purpose: they are the same verb, and a
    /// second slot would let a save made through one be restored as nothing
    /// through the other -- silently, since "nothing saved" is already a
    /// legal state that does nothing.
    fn save_cursor(&mut self) {
        self.screen.saved_cursor = Some(SavedCursor {
            pos: self.screen.grid.cursor(),
            style: self.style(),
        });
    }

    /// DECRC's read half. With nothing saved this does nothing; see the
    /// `ESC 8` arm for why homing instead would be worse.
    fn restore_cursor(&mut self) {
        if let Some(saved) = self.screen.saved_cursor {
            let (row, col) = saved.pos;
            self.screen.grid.goto(row, col);
            let (fg, bg, attrs) = saved.style;
            self.screen.state.fg = fg;
            self.screen.state.bg = bg;
            self.screen.state.attrs = attrs;
        }
    }

    /// DECSTR (`CSI ! p`) -- the soft reset. Modes and the saved-cursor slot
    /// go back to their defaults and the cursor homes, but the cells, the
    /// scrollback and the title stay: none of those three is a mode, and a
    /// soft reset that erased the screen would be RIS by another name.
    fn soft_reset(&mut self) {
        self.screen.state.fg = Color::Default;
        self.screen.state.bg = Color::Default;
        self.screen.state.attrs = Attrs::NONE;
        self.screen.saved_cursor = None;
        self.exit_alt_screen_for_reset();
        self.screen.grid.reset_modes();
        self.screen.grid.goto(0, 0);
    }

    /// RIS (`ESC c`) -- the full reset. The soft reset plus the erase and the
    /// title.
    ///
    /// **Scrollback is deliberately kept** (the reason is at
    /// [`Grid::reset`]). The progress level from `OSC 9;4` is deliberately
    /// NOT cleared either: this round's contract enumerates what RIS drops
    /// and the level is not on it. That is a real edge -- a crashed agent's
    /// wrapper running `reset` leaves a stale `working` level behind -- and
    /// it is reported as a concern rather than fixed here, because widening
    /// a reset past its specification is how a reset starts erasing things
    /// its callers still need.
    fn full_reset(&mut self) {
        self.soft_reset();
        self.screen.state.title = None;
        self.screen.grid.reset();
    }

    /// Both resets leave the primary screen current. Uses the same swap the
    /// mode does, so the dirty-marking and the one-buffer bookkeeping have a
    /// single implementation rather than a second one that drifts.
    fn exit_alt_screen_for_reset(&mut self) {
        if self.screen.saved.is_some() {
            self.toggle_alt_screen(false, AltBuffer::Legacy);
        }
    }
}

/// Which spelling of the alternate screen asked for the swap. The two
/// differ only on entry, and only about the buffer -- see the
/// `retained_alt` field on `Screen`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AltBuffer {
    /// Modes 47 and 1047: keep whatever the alternate buffer already holds.
    Legacy,
    /// Mode 1049: start from a blank alternate buffer every time.
    Cleared,
}

/// Uniform forwarding. Every body here is exactly one delegation and must
/// stay that way -- the censuses in `tests.rs` read the real tables out of
/// `csi.rs` / `esc.rs` and separately pin that these bodies name no verb of
/// their own, so an arm added here instead of there fails rather than
/// escaping the guard.
impl vte::Perform for Performer<'_> {
    fn print(&mut self, c: char) {
        let style = self.style();
        self.screen.grid.put(c, style);
    }

    fn execute(&mut self, byte: u8) {
        self.c0(byte);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        self.esc(intermediates, byte);
    }

    fn csi_dispatch(&mut self, params: &vte::Params, inter: &[u8], _ignore: bool, action: char) {
        self.csi(params, inter, action);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        self.osc(params);
    }

    // DCS: explicitly nothing, three times over.
    //
    // `vte` runs the whole DCS state machine itself and routes payload bytes
    // to `put` -- never to `print` -- so a device-control string cannot reach
    // the grid whether these exist or not. They exist because "no branch"
    // and "a branch that deliberately does nothing" read identically at the
    // call site and only one of them is a decision. The two things
    // libghostty's DCS handler is for (XTGETTCAP and DECRQSS replies) both
    // write back to the PTY, and nothing in `screen/` ever writes back, so
    // there is no consumer here to starve.
    //
    // The tempting wrong version is `put` forwarding to `print`; the guard
    // `dcs_hook_put_unhook_are_explicit_no_ops` fails on exactly that.
    fn hook(&mut self, _params: &vte::Params, _inter: &[u8], _ignore: bool, _action: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}
}

#[cfg(test)]
mod tests;
