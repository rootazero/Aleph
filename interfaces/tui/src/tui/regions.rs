//! Where on the screen a click means something.
//!
//! Built while painting, consumed by the next mouse event. A terminal gives
//! us a cell coordinate and nothing else — no DOM, no hit-testing — so the
//! only way a click can know what it landed on is for the renderer to write
//! down what it put there.
//!
//! # Why the table is rebuilt every frame rather than kept in sync
//!
//! The transcript reflows on every scroll, resize, streamed token and settled
//! tool row. A table maintained incrementally would be a second model of the
//! layout, and the first time it disagreed with the painted one a click would
//! silently expand the wrong row (判据 §1: the same fact in two places, and
//! the cheap copy is the one that lies). Rebuilding is O(visible rows) and
//! happens inside the pass that is already walking them.
//!
//! # Why the table is read one frame late
//!
//! A click is delivered against the frame the user was looking at when they
//! clicked, which is the frame that built the table. That is not a staleness
//! bug to be fixed; it is the only correct reading.

use ratatui::layout::Rect;

/// What clicking a region does.
///
/// Deliberately only the two kinds that have a renderer today. The plan's
/// longer list (`show-more`, `expanded card`) named regions this TUI does not
/// paint; enumerating them here would be two more arms that nothing can ever
/// produce and nothing can ever be observed to handle (判据 §17: a display
/// thing must be able to point at the line that renders it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// The docked `[ ↓ Back to bottom · ctrl+end ]` button. Painted *over* the
    /// transcript, so it has to win the hit test against whatever row it
    /// covers — which is what [`RegionKind::priority`] is for.
    BackToBottom,
    /// A tool row's header, or the `… +N lines` hint under it: either toggles
    /// that row between folded and whole.
    ToggleRow,
}

impl RegionKind {
    /// Lower wins. A `match` rather than a `#[derive(PartialOrd)]` on
    /// declaration order so adding a kind is a decision someone has to write
    /// down, not a consequence of where they happened to put it.
    const fn priority(self) -> u8 {
        match self {
            Self::BackToBottom => 0,
            Self::ToggleRow => 1,
        }
    }
}

/// One clickable rectangle and what it addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub rect: Rect,
    pub kind: RegionKind,
    /// The tool call id for [`RegionKind::ToggleRow`]; empty for kinds that
    /// address the screen rather than a row.
    pub row_id: String,
}

/// Every clickable region of the last painted frame.
#[derive(Debug, Default)]
pub struct RegionTable {
    regions: Vec<Region>,
}

impl RegionTable {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn push(&mut self, rect: Rect, kind: RegionKind, row_id: String) {
        // A zero-area rect can never be hit, and storing it would put entries
        // in the table that the hit test can only ever skip.
        if rect.width > 0 && rect.height > 0 {
            self.regions.push(Region { rect, kind, row_id });
        }
    }

    /// The region a click at `(column, row)` addresses, highest priority
    /// first.
    ///
    /// Overlap is expected, not exceptional: the back-to-bottom button is
    /// drawn on top of a transcript row that may itself be a tool header.
    #[must_use]
    pub fn hit(&self, column: u16, row: u16) -> Option<&Region> {
        self.regions
            .iter()
            .filter(|r| {
                column >= r.rect.x
                    && column < r.rect.x.saturating_add(r.rect.width)
                    && row >= r.rect.y
                    && row < r.rect.y.saturating_add(r.rect.height)
            })
            .min_by_key(|r| r.kind.priority())
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.regions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn a_click_outside_every_region_hits_nothing() {
        let mut t = RegionTable::default();
        t.push(rect(0, 5, 10, 1), RegionKind::ToggleRow, "c1".into());
        assert!(t.hit(0, 4).is_none());
        assert!(t.hit(10, 5).is_none(), "x is half-open on the right");
        assert!(t.hit(9, 5).is_some());
    }

    /// The reason the table is ordered at all: the docked button is painted
    /// over a transcript row, so both regions contain the click and only one
    /// of them is what the user aimed at.
    ///
    /// # When this goes red
    ///
    /// Returning the first match instead of the highest-priority one — the
    /// obvious implementation, and the one under which the button becomes
    /// unclickable the moment it happens to cover a tool header.
    #[test]
    fn the_docked_button_wins_the_row_it_covers() {
        let mut t = RegionTable::default();
        // Pushed FIRST, so a first-match hit test would return it.
        t.push(rect(0, 9, 40, 1), RegionKind::ToggleRow, "c1".into());
        t.push(rect(20, 9, 20, 1), RegionKind::BackToBottom, String::new());

        let hit = t.hit(25, 9).expect("inside both");
        assert_eq!(hit.kind, RegionKind::BackToBottom);

        // And outside the button's columns the row underneath is still live.
        let hit = t.hit(5, 9).expect("inside the row only");
        assert_eq!(hit.kind, RegionKind::ToggleRow);
        assert_eq!(hit.row_id, "c1");
    }

    #[test]
    fn an_empty_rect_is_never_stored() {
        let mut t = RegionTable::default();
        t.push(rect(3, 3, 0, 1), RegionKind::ToggleRow, "c1".into());
        t.push(rect(3, 3, 5, 0), RegionKind::ToggleRow, "c2".into());
        assert_eq!(t.len(), 0);
        assert!(t.hit(3, 3).is_none());
    }
}
