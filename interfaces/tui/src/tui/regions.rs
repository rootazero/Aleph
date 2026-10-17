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

/// What a click on the sticky user bar's `[click to jump]` label carries:
/// the identity of the bar's message **as painted** (spec §6.1).
///
/// The click is delivered one frame after the table was built — the frame
/// the user was looking at. Replacing the transcript or switching sessions
/// makes the hit stale, and the handler rejects it rather than jumping to
/// whatever now occupies the painted row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpTarget {
    /// Session the bar was painted for.
    pub session_key: String,
    /// `AppState::transcript_generation` at paint time.
    pub generation: u64,
    /// Stable id of the `TranscriptEntry::UserText` to land on.
    pub entry_id: String,
}

/// What clicking a region does.
///
/// Deliberately only the kinds that have a renderer today. The plan's
/// longer list (`show-more`, `expanded card`) named regions this TUI does not
/// paint; enumerating them here would be arms that nothing can ever
/// produce and nothing can ever be observed to handle (判据 §17: a display
/// thing must be able to point at the line that renders it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// The docked `[ ↓ Back to bottom · ctrl+end ]` button. Painted *over* the
    /// transcript, so it has to win the hit test against whatever row it
    /// covers — which is what [`RegionKind::priority`] is for.
    BackToBottom,
    /// The complete `[click to jump]` label at the right end of the sticky
    /// user bar: jumps the viewport back to the bar's user message. Painted
    /// over the transcript like [`RegionKind::BackToBottom`], so it must win
    /// against the row it covers.
    JumpToUser,
    /// The excerpt part of the sticky user bar: blocks the click from
    /// reaching the transcript row the bar is painted over, but jumps
    /// nothing — only the label acts.
    StickyUserShield,
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
            Self::JumpToUser => 1,
            Self::StickyUserShield => 2,
            Self::ToggleRow => 3,
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
    /// The sticky bar's jump payload; `None` for every other kind. Kept off
    /// `RegionKind` so the kind stays `Copy` for its `match` consumers.
    pub jump: Option<JumpTarget>,
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
            self.regions.push(Region {
                rect,
                kind,
                row_id,
                jump: None,
            });
        }
    }

    /// Register the sticky user bar's jump zone — the only region that
    /// carries a [`JumpTarget`].
    pub fn push_jump(&mut self, rect: Rect, target: JumpTarget) {
        if rect.width > 0 && rect.height > 0 {
            self.regions.push(Region {
                rect,
                kind: RegionKind::JumpToUser,
                row_id: String::new(),
                jump: Some(target),
            });
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

    /// The sticky user bar is painted over a transcript row, so both its
    /// zones have to win the hit test: the label zone jumps (and carries the
    /// painted identity with it), the excerpt zone only blocks — a click
    /// there must not fall through and toggle the row underneath.
    #[test]
    fn the_sticky_bar_beats_the_row_it_covers() {
        let mut t = RegionTable::default();
        // The covered tool row, pushed FIRST like the renderer does.
        t.push(rect(0, 0, 40, 1), RegionKind::ToggleRow, "c1".into());
        t.push(
            rect(0, 0, 10, 1),
            RegionKind::StickyUserShield,
            String::new(),
        );
        t.push_jump(
            rect(10, 0, 15, 1),
            JumpTarget {
                session_key: "s".into(),
                generation: 7,
                entry_id: "u9".into(),
            },
        );

        let hit = t.hit(12, 0).expect("inside the label zone");
        assert_eq!(hit.kind, RegionKind::JumpToUser);
        assert_eq!(hit.jump.as_ref().map(|j| j.entry_id.as_str()), Some("u9"));
        assert_eq!(hit.jump.as_ref().map(|j| j.generation), Some(7));

        let hit = t.hit(5, 0).expect("inside the shield zone");
        assert_eq!(hit.kind, RegionKind::StickyUserShield);
        assert!(hit.jump.is_none());

        // Past the bar's right edge the row underneath is live again.
        let hit = t.hit(30, 0).expect("beyond the bar");
        assert_eq!(hit.kind, RegionKind::ToggleRow);
    }
}
