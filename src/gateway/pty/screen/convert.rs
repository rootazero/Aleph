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
        // The three below are reserved wire fields with no producer yet: the
        // server screen does not track these modes, so `None` here is the
        // literal truth ("unchanged / not known"), not a placeholder standing
        // in for a value we have. Each is wired by Stream B task B3, which
        // adds the `ScreenPatch` fields these will read.
        //
        // wired by Stream B, guarded by
        // `cursor_visibility_rides_the_patch_only_when_it_changes`
        cursor_visible: None,
        // wired by Stream B, guarded by `bracketed_paste_mode_rides_the_patch`
        bracketed_paste: None,
        // wired by Stream B, guarded by
        // `osc7_file_uri_with_empty_or_localhost_host_sets_cwd_and_percent_decodes`
        cwd: None,
        bell: p.bell,
    }
}
