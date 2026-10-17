//! Pure viewport decisions for loaded user entries.
//! The transcript remains the only source of text; an anchor retains identity,
//! never a cached physical row or a second copy of the message.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::regions::JumpTarget;

pub(super) const JUMP_LABEL: &str = "[click to jump]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickyAnchor {
    pub session_key: String,
    pub entry_id: String,
    pub intra_line: usize,
}

#[derive(Debug, Default)]
pub struct StickyUserState {
    pub anchor: Option<StickyAnchor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct UserPlacement {
    pub start: usize,
    pub end: usize, // Exclusive; excludes the following blank separator.
}

impl StickyUserState {
    /// A mouse hit is valid only for the exact frame's transcript generation.
    pub fn jump(&mut self, hit: &JumpTarget, session: &str, generation: u64) -> bool {
        if hit.session_key != session || hit.generation != generation {
            return false;
        }
        self.jump_to_entry(session, hit.entry_id.clone());
        true
    }

    /// `/jump-user`: anchor on an entry the caller located in the transcript
    /// (identity, never a text search). Same target as the mouse, no mouse.
    pub fn jump_to_entry(&mut self, session: &str, entry_id: String) {
        self.anchor = Some(StickyAnchor {
            session_key: session.to_owned(),
            entry_id,
            intra_line: 0,
        });
    }

    /// Any manual scroll drops the anchor: without this the next frame would
    /// re-pin the viewport the reader just moved away from.
    pub fn clear(&mut self) {
        self.anchor = None;
    }

    /// Resolve the anchor against *this frame's* measured row positions.
    /// No stale cached physical line survives resize, folding or streaming.
    pub(super) fn anchored_offset(
        &mut self,
        session: &str,
        entries: &[(String, UserPlacement)],
        total: usize,
        height: usize,
    ) -> Option<usize> {
        let anchor = self.anchor.as_ref()?;
        if anchor.session_key != session {
            self.anchor = None;
            return None;
        }
        let Some((_, placement)) = entries.iter().find(|(id, _)| id == &anchor.entry_id) else {
            self.anchor = None;
            return None;
        };
        let top = placement.start
            + anchor
                .intra_line
                .min(placement.end.saturating_sub(placement.start + 1));
        Some(total.saturating_sub(height).saturating_sub(top))
    }
}

/// Select the user message whose sticky chip represents this viewport.
///
/// User rows are ordered as they appear in the transcript. The viewport is a
/// half-open `[start, viewport_end)` range, so a message whose start equals
/// `viewport_end` is not in the range and the preceding message remains
/// selected. The latest eligible row wins, selecting the latest user at the
/// bottom and walking backward one message at a time while scrolling up. Above
/// the first user row, the oldest row remains as context so the chip is always
/// present for a non-empty transcript.
pub(super) fn select_user_id(
    rows: &[(String, UserPlacement)],
    viewport_end: usize,
) -> Option<&str> {
    rows.iter()
        .rev()
        .find(|(_, placement)| placement.start < viewport_end)
        .or_else(|| rows.first())
        .map(|(id, _)| id.as_str())
}

/// Fits a one-line excerpt and the *complete* literal action label. Widths
/// are terminal columns rather than bytes or Unicode scalar counts.
///
/// When space runs out the EXCERPT shrinks first — down to nothing — and only
/// a width that cannot hold the complete label (or a one-row chat area, where
/// the bar would be the viewport) draws no bar at all (spec §6.1).
pub(super) fn label(text: &str, width: u16, height: u16) -> Option<String> {
    let width = usize::from(width);
    let label_width = UnicodeWidthStr::width(JUMP_LABEL);
    if height < 2 || width < label_width {
        return None;
    }
    // Multi-line input flattens to spaces in the EXCERPT only; the transcript
    // entry keeps its original text.
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if width == label_width || flattened.is_empty() {
        return Some(JUMP_LABEL.to_string());
    }
    // One column goes to the space between excerpt and label.
    let excerpt_budget = width - label_width - 1;
    if excerpt_budget < 3 {
        return Some(JUMP_LABEL.to_string());
    }
    let flat_width = UnicodeWidthStr::width(flattened.as_str());
    if flat_width <= excerpt_budget {
        return Some(format!("{flattened} {JUMP_LABEL}"));
    }
    // Reserve three cells for the user's requested ASCII truncation marker.
    let keep = excerpt_budget - 3;
    let mut excerpt = String::new();
    let mut used = 0;
    for c in flattened.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > keep {
            break;
        }
        excerpt.push(c);
        used += cw;
    }
    Some(format!("{}... {JUMP_LABEL}", excerpt.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_empty_without_user_rows_and_uses_latest_at_bottom() {
        assert_eq!(select_user_id(&[], 100), None);
        let rows = vec![
            ("first".into(), UserPlacement { start: 2, end: 5 }),
            ("second".into(), UserPlacement { start: 10, end: 14 }),
            ("third".into(), UserPlacement { start: 20, end: 24 }),
        ];
        assert_eq!(select_user_id(&rows, 25), Some("third"));
        // A short transcript still has a bottom viewport starting at row 0.
        let short_rows = vec![("only".into(), UserPlacement { start: 0, end: 1 })];
        assert_eq!(select_user_id(&short_rows, 1), Some("only"));
    }

    #[test]
    fn selection_switches_on_half_open_viewport_end_in_both_directions() {
        let rows = vec![
            ("first".into(), UserPlacement { start: 2, end: 5 }),
            ("second".into(), UserPlacement { start: 10, end: 14 }),
            ("third".into(), UserPlacement { start: 20, end: 24 }),
        ];
        // The end exactly at a message start has crossed that message.
        assert_eq!(select_user_id(&rows, 20), Some("second"));
        // Any intersection/reach into the message selects it.
        assert_eq!(select_user_id(&rows, 21), Some("third"));
        assert_eq!(select_user_id(&rows, 11), Some("second"));
        // Moving upward and then downward restores the same sequence.
        assert_eq!(select_user_id(&rows, 10), Some("first"));
        assert_eq!(select_user_id(&rows, 11), Some("second"));
        assert_eq!(select_user_id(&rows, 21), Some("third"));
        // Above the first message, the oldest user remains the visible context.
        assert_eq!(select_user_id(&rows, 2), Some("first"));
    }

    #[test]
    fn identity_not_text_and_reflow_determines_offset() {
        let mut state = StickyUserState::default();
        let hit = JumpTarget {
            session_key: "s".into(),
            generation: 4,
            entry_id: "second".into(),
        };
        assert!(!state.jump(&hit, "other", 4));
        assert!(!state.jump(&hit, "s", 5));
        assert!(state.jump(&hit, "s", 4));
        let positions = vec![
            ("first".into(), UserPlacement { start: 2, end: 5 }),
            ("second".into(), UserPlacement { start: 20, end: 23 }),
        ];
        assert_eq!(state.anchored_offset("s", &positions, 50, 10), Some(20));
        // Streaming adds rows below; folding/resize changes the target's row.
        let reflowed = vec![
            ("first".into(), UserPlacement { start: 2, end: 5 }),
            ("second".into(), UserPlacement { start: 12, end: 16 }),
        ];
        assert_eq!(state.anchored_offset("s", &reflowed, 80, 9), Some(59));
        assert!(state.anchored_offset("other", &reflowed, 80, 9).is_none());
        assert!(state.anchor.is_none());
    }

    #[test]
    fn slash_and_mouse_land_on_the_same_anchor() {
        let mut state = StickyUserState::default();
        state.jump_to_entry("s", "u1".to_string());
        let positions = vec![("u1".into(), UserPlacement { start: 3, end: 6 })];
        assert_eq!(state.anchored_offset("s", &positions, 30, 10), Some(17));
        // A manual scroll drops the anchor; the next frame re-pins nothing.
        state.clear();
        assert!(state.anchored_offset("s", &positions, 30, 10).is_none());
    }

    #[test]
    fn narrow_cjk_and_emoji_keep_action_complete() {
        // Flattened, fits: excerpt in full.
        assert_eq!(
            label("你好 🙂\n下一行", 40, 2).as_deref(),
            Some("你好 🙂 下一行 [click to jump]")
        );
        // Budget 9 columns for the excerpt: 7 kept + ellipsis, cut by columns.
        assert_eq!(
            label("你好 🙂\n下一行", 25, 2).as_deref(),
            Some("你好... [click to jump]")
        );
        assert_eq!(
            label("abcdef", 20, 2).as_deref(),
            Some("a... [click to jump]")
        );
        assert_eq!(
            label("abcdef", 19, 2).as_deref(),
            Some("... [click to jump]")
        );
        assert_eq!(label("abcdef", 18, 2).as_deref(), Some(JUMP_LABEL));
        assert_eq!(label("abcdef", 17, 2).as_deref(), Some(JUMP_LABEL));
        // Exactly the label's width: the excerpt is gone, the label complete.
        assert_eq!(label("x", 15, 2).as_deref(), Some(JUMP_LABEL));
        assert_eq!(label("abcdef", 16, 2).as_deref(), Some(JUMP_LABEL));
        // An empty message gets the label alone rather than a leading space.
        assert_eq!(label("", 30, 2).as_deref(), Some(JUMP_LABEL));
        // Less than the label, or a one-row chat area: no bar at all.
        assert!(label("x", 14, 2).is_none());
        assert!(label("x", 40, 1).is_none());
    }
}
