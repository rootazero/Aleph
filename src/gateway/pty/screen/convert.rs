//! Server screen types → wire types.
//!
//! The conversion lives here rather than as `From` impls on the protocol
//! types because the protocol crate must not depend on alephcore.

use aleph_protocol::pty::{PtyAttrs, PtyColor, PtyRowPatch, PtyScreenPatch, PtyStyleRun};

use super::diff::{ScreenPatch, StyleRun};
use super::grid::{Attrs, Color};

#[must_use]
pub fn colour(c: Color) -> PtyColor {
    match c {
        Color::Default => PtyColor::Default,
        Color::Indexed(n) => PtyColor::indexed(n),
        Color::Rgb(r, g, b) => PtyColor::rgb(r, g, b),
    }
}

#[must_use]
pub fn attrs(a: Attrs) -> PtyAttrs {
    PtyAttrs(a.0)
}

#[must_use]
pub fn run(r: &StyleRun) -> PtyStyleRun {
    PtyStyleRun {
        text: r.text.clone(),
        fg: colour(r.fg),
        bg: colour(r.bg),
        attrs: attrs(r.attrs),
    }
}

#[must_use]
pub fn patch(p: &ScreenPatch) -> PtyScreenPatch {
    PtyScreenPatch {
        rows: p
            .rows
            .iter()
            .map(|r| PtyRowPatch {
                row: r.row,
                runs: r.runs.iter().map(run).collect(),
            })
            .collect(),
        cursor: p.cursor,
        alt_screen: p.alt_screen,
        title: p.title.clone(),
        // Straight through, `None` and all: `None` here is the server
        // screen's own "unchanged since the last patch", so translating it
        // to anything else would invent news. `attach_snapshot` reaches the
        // wire through this same function applied to `Screen::full_patch`,
        // which fills all three with their CURRENT values -- so a client
        // attaching late is served by this line, not by a separate path.
        cursor_visible: p.cursor_visible,
        bracketed_paste: p.bracketed_paste,
        cwd: p.cwd.clone(),
        bell: p.bell,
    }
}
