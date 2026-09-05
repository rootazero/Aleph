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
    /// The three observable mode bits as of the last `take_patch`. `None`
    /// means "never published", which is why the very first patch announces
    /// all three: a client that has been told nothing cannot tell a default
    /// from a value nobody sent.
    last_sent_cursor_visible: Option<bool>,
    last_sent_bracketed_paste: Option<bool>,
    last_sent_cwd: Option<String>,
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
    /// DECTCEM (`CSI ?25 h/l`). Visible unless a program says otherwise.
    cursor_visible: bool,
    /// Bracketed paste (mode 2004). Off until a program turns it on, and the
    /// client must assume the same: wrapping a paste the program did not ask
    /// to have wrapped puts `ESC[200~` into its input.
    bracketed_paste: bool,
    /// Live working directory from `OSC 7`. `None` = the program has never
    /// reported one, so a caller falls through to the next source (the
    /// foreground process, then the spawn directory) rather than reading
    /// this absence as "no directory".
    cwd: Option<String>,
    bell: bool,
}

/// Hand-written rather than derived because ONE field's default is not its
/// type's: a cursor is visible until a program hides it, and
/// `#[derive(Default)]` would make every new session start out reporting a
/// hidden cursor. The derive was correct until this field existed, which is
/// exactly the kind of change that keeps a derive looking right.
impl Default for ScreenState {
    fn default() -> Self {
        Self {
            fg: Color::default(),
            bg: Color::default(),
            attrs: Attrs::default(),
            title: None,
            osc_progress: None,
            cursor_visible: true,
            bracketed_paste: false,
            cwd: None,
            bell: false,
        }
    }
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
            last_sent_cursor_visible: None,
            last_sent_bracketed_paste: None,
            last_sent_cwd: None,
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

    /// Whether the program wants a cursor drawn (DECTCEM).
    #[must_use]
    pub const fn cursor_visible(&self) -> bool {
        self.state.cursor_visible
    }

    /// Whether the program has bracketed paste enabled (mode 2004). A client
    /// wraps a paste in `ESC[200~ … ESC[201~` only when this is true.
    #[must_use]
    pub const fn bracketed_paste(&self) -> bool {
        self.state.bracketed_paste
    }

    /// The live working directory the program last reported through `OSC 7`.
    ///
    /// `None` means "this program has told me nothing" — never "there is no
    /// directory". OSC 7 only arrives from shells with integration
    /// configured, so this is a supplement to the spawn directory and to a
    /// foreground-process probe, not a replacement for either.
    #[must_use]
    pub fn cwd(&self) -> Option<&str> {
        self.state.cwd.as_deref()
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
    ///
    /// `retained_alt` is the mirror of that case and gets the same treatment:
    /// a parked ALTERNATE buffer that missed a resize comes back at the old
    /// size through `?47`/`?1047`, and then `attach_snapshot` reports the old
    /// dimensions while `take_patch` emits row indices past the new height —
    /// the stale-row-index failure `Grid::resize`'s own dirty-set comment
    /// warns about, arriving through a different door. Nothing downstream
    /// triggers a corrective resize, so it has to happen here.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.grid.resize(rows, cols);
        for parked in [self.saved.as_mut(), self.retained_alt.as_mut()]
            .into_iter()
            .flatten()
        {
            parked.resize(rows, cols);
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
        // Both parked buffers, for the same reason `resize` handles both:
        // whichever one comes back must not come back stale.
        for parked in [self.saved.as_mut(), self.retained_alt.as_mut()]
            .into_iter()
            .flatten()
        {
            parked.set_scrollback_limit(lines);
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
        let cursor_visible = self.state.cursor_visible;
        let cursor_visible_changed = Some(cursor_visible) != self.last_sent_cursor_visible;
        let bracketed_paste = self.state.bracketed_paste;
        let bracketed_paste_changed = Some(bracketed_paste) != self.last_sent_bracketed_paste;
        let cwd_changed = self.state.cwd != self.last_sent_cwd;

        let patch = super::diff::ScreenPatch {
            rows: super::diff::patch_rows(&self.grid, dirty),
            cursor: Some(self.grid.cursor()),
            alt_screen: alt_changed.then_some(alt),
            title: published_clear(title_changed, &self.state.title),
            cursor_visible: cursor_visible_changed.then_some(cursor_visible),
            bracketed_paste: bracketed_paste_changed.then_some(bracketed_paste),
            cwd: published_clear(cwd_changed, &self.state.cwd),
            bell,
        };
        // Cursor is always present above, so emptiness is decided on the
        // fields that actually carry news. Every new bit has to be named
        // here as well as in the struct above: a bit that changed but was
        // left out of this condition ships a patch whose only content is
        // dropped as "nothing changed" -- silently, since the frame simply
        // never arrives.
        if patch.rows.is_empty()
            && !title_changed
            && !alt_changed
            && !bell
            && !cursor_visible_changed
            && !bracketed_paste_changed
            && !cwd_changed
        {
            return None;
        }
        self.last_sent_title.clone_from(&self.state.title);
        self.last_sent_alt = Some(alt);
        self.last_sent_cursor_visible = Some(cursor_visible);
        self.last_sent_bracketed_paste = Some(bracketed_paste);
        self.last_sent_cwd.clone_from(&self.state.cwd);
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
            // A full snapshot has no "unchanged", so `None` here is
            // unambiguous: it means the server holds no title. A DIFF cannot
            // say that with `None` (see `published_clear`) and spells it
            // `Some("")` instead, so one client rule covers both: an empty
            // OR absent title means there is none.
            //
            // CURRENT values, not "changed" ones: this is the attach path,
            // and a client that was not here for the change has to be told
            // the level. `PtySession::attach_snapshot` reaches the wire by
            // handing this straight to `convert::patch`, so these three
            // lines are the whole of the snapshot wiring for these bits.
            cursor_visible: Some(self.state.cursor_visible),
            bracketed_paste: Some(self.state.bracketed_paste),
            cwd: self.state.cwd.clone(),
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
            // Absolute: the position was absolute when it was saved, so
            // routing it through DECOM would add the region's top again.
            self.screen.grid.set_cursor(row, col);
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
        // The two observable mode bits go back to the terminal's defaults,
        // not to whatever the program that just died left them at. A `?2004h`
        // surviving a reset keeps a client wrapping pastes for a shell that
        // never asked for it, and a `?25l` surviving one hides the cursor for
        // the life of the session -- both are the stale-evidence shape that
        // makes a reset worth having.
        self.screen.state.cursor_visible = true;
        self.screen.state.bracketed_paste = false;
        self.screen.saved_cursor = None;
        self.exit_alt_screen_for_reset();
        self.screen.grid.reset_modes();
        self.screen.grid.goto(0, 0);
    }

    /// RIS (`ESC c`) -- the full reset. The soft reset plus the erase, the
    /// title, and the retained `OSC 9;4` progress level.
    ///
    /// **Scrollback is deliberately kept** (the reason is at
    /// [`Grid::reset`]): `visible_text` never reads it, so clearing it would
    /// take away what the user scrolled back to and give no consumer
    /// anything.
    ///
    /// **The progress level IS cleared**, and it is the one piece of state
    /// here that is not a terminal mode. It is cleared because of what a
    /// reset is FOR: the case RIS exists to fix is a crashed agent whose
    /// wrapper runs `reset`, and the detection manifests read that level as
    /// evidence of work in progress. A level left standing keeps the runtime
    /// table publishing `Working` for a process that is gone -- the same
    /// stale-evidence failure the cleared grid removes, arriving through the
    /// one channel the erase does not touch.
    fn full_reset(&mut self) {
        self.soft_reset();
        self.screen.state.title = None;
        self.screen.state.osc_progress = None;
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

/// One publishable optional string on a diff: `None` when it did not
/// change, and a CLEAR sent as an empty string rather than as `None`.
///
/// `None` on this wire already means "unchanged" -- `PtyScreenPatch`'s fields
/// are `Option<String>` with `skip_serializing_if`, so a `None` occupies no
/// bytes and says nothing. Sending a Some-to-None transition as `None` is
/// therefore a report of success that changes nothing at the client: RIS
/// clears the title on the server, the change is consumed into
/// `last_sent_title`, and the Panel tab keeps the pre-RIS title for the life
/// of the session (判据 §11 -- a no-op that reports success).
///
/// `shared/protocol` is frozen this round, so a clear travels as `Some("")`.
/// An empty string is already this codebase's spelling of "no title": an
/// empty `OSC 0` payload is not treated as a title, and a tab label falls
/// through to the program or shell name when it is empty. **The client rule
/// is therefore one rule -- empty OR absent means none** -- which also covers
/// the `None` a full snapshot sends (see `full_patch`).
///
/// Shared by `title` and `cwd` rather than written twice: `cwd` has no
/// producer that clears it today, so a duplicated rule there would be a
/// second expression of one fact with only one of them exercised, and the
/// two would drift the moment a later round gives `cwd` a clear (判据 §1).
fn published_clear(changed: bool, value: &Option<String>) -> Option<String> {
    changed.then(|| value.clone().unwrap_or_default())
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
