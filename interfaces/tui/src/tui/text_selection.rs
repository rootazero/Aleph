//! Visible-cell selection, independent of application state and clipboard I/O.
//!
//! Observe the live pane before painting this overlay. Coordinates are inclusive
//! and row-major; touching either half of a wide glyph selects the entire glyph.
//! Only the pane is frozen. A release without pointer movement unfreezes it and
//! delegates the original press position to the caller's normal hit testing.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};
use unicode_width::UnicodeWidthStr;

#[derive(Default)]
pub(super) struct TextSelection {
    snapshot: Option<Snapshot>,
    // The latest live frame lets a new press replace an old selection without
    // allowing an active drag to observe a moving stream.
    live: Option<Snapshot>,
    selection: Option<Selection>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MouseOutcome {
    Ignored,
    Consumed,
    Click { column: u16, row: u16 },
    Copy(String),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Point {
    // Field order deliberately matches visual reading order.
    row: u16,
    column: u16,
}

struct Selection {
    anchor: Point,
    end: Point,
    dragging: bool,
    moved: bool,
}

impl Selection {
    fn bounds(&self) -> (Point, Point) {
        (self.anchor.min(self.end), self.anchor.max(self.end))
    }

    fn columns(&self, row: u16, area: Rect) -> Option<(u16, u16)> {
        let (start, end) = self.bounds();
        if row < start.row || row > end.row {
            return None;
        }
        Some((
            if row == start.row {
                start.column
            } else {
                area.x
            },
            if row == end.row {
                end.column
            } else {
                area.right() - 1
            },
        ))
    }
}

#[derive(Clone)]
struct Glyph {
    start: u16,
    // Exclusive, unlike the inclusive mouse selection endpoints.
    end: u16,
}

#[derive(Clone)]
struct Snapshot {
    requested_area: Rect,
    source_area: Rect,
    buffer: Buffer,
    rows: Vec<Vec<Glyph>>,
}

impl Snapshot {
    fn capture(source: &Buffer, requested_area: Rect) -> Self {
        let area = source.area.intersection(requested_area);
        if area.is_empty() {
            return Self {
                requested_area,
                source_area: source.area,
                buffer: Buffer::empty(area),
                rows: Vec::new(),
            };
        }
        let mut buffer = Buffer::empty(area);
        let mut rows = Vec::with_capacity(usize::from(area.height));
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                buffer[(column, row)] = source[(column, row)].clone();
            }
            let mut glyphs = Vec::new();
            // Scan from the source's left edge so a cropped pane cannot turn a
            // wide glyph's continuation cell into a spurious copyable space.
            let mut column = u32::from(source.area.x);
            while area.width != 0 && column < u32::from(area.right()) {
                let symbol = source[(column as u16, row)].symbol();
                let width = UnicodeWidthStr::width(symbol).max(1);
                let end = column + width.min(usize::from(u16::MAX)) as u32;
                if column >= u32::from(area.x) && end <= u32::from(area.right()) {
                    glyphs.push(Glyph {
                        start: column as u16,
                        end: end as u16,
                    });
                }
                column = end;
            }
            rows.push(glyphs);
        }
        Self {
            requested_area,
            source_area: source.area,
            buffer,
            rows,
        }
    }

    fn contains(&self, point: Point) -> bool {
        let area = self.buffer.area;
        point.column >= area.x
            && point.column < area.right()
            && point.row >= area.y
            && point.row < area.bottom()
    }

    fn clamp(&self, point: Point) -> Point {
        let area = self.buffer.area;
        // Only reachable after arming inside a nonempty area.
        Point {
            column: point.column.clamp(area.x, area.right() - 1),
            row: point.row.clamp(area.y, area.bottom() - 1),
        }
    }
}

impl TextSelection {
    pub(super) fn observe(&mut self, buffer: &Buffer, area: Rect) {
        let live = Snapshot::capture(buffer, area);
        if self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.requested_area != area || snapshot.source_area != buffer.area
        }) {
            self.clear();
        }
        self.live = Some(live.clone());
        if self.selection.is_none() {
            self.snapshot = Some(live);
        }
    }

    pub(super) fn paint(&self, buffer: &mut Buffer) {
        let (Some(snapshot), Some(selection)) = (&self.snapshot, &self.selection) else {
            return;
        };
        // A resize arriving between observe and paint must not index stale cells.
        if snapshot.source_area != buffer.area {
            return;
        }
        let area = snapshot.buffer.area;
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                buffer[(column, row)] = snapshot.buffer[(column, row)].clone();
            }
            if !selection.moved {
                continue;
            }
            let Some((left, right)) = selection.columns(row, area) else {
                continue;
            };
            for glyph in &snapshot.rows[usize::from(row - area.y)] {
                if glyph.start <= right && glyph.end > left {
                    for column in glyph.start..glyph.end {
                        buffer[(column, row)].modifier.insert(Modifier::REVERSED);
                    }
                }
            }
        }
    }

    pub(super) fn mouse(&mut self, event: MouseEvent) -> MouseOutcome {
        let point = Point {
            column: event.column,
            row: event.row,
        };
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.contains(point))
        {
            // Replace a finished selection with the most recent live frame;
            // the frame remains frozen from this press through release.
            let geometry = self
                .snapshot
                .as_ref()
                .map(|snapshot| (snapshot.requested_area, snapshot.source_area));
            let live = self
                .live
                .as_ref()
                .filter(|live| geometry == Some((live.requested_area, live.source_area)))
                .cloned();
            if let Some(live) = live {
                self.snapshot = Some(live);
            }
            self.selection = Some(Selection {
                anchor: point,
                end: point,
                dragging: true,
                moved: false,
            });
            return MouseOutcome::Consumed;
        }
        let Some(snapshot) = &self.snapshot else {
            return MouseOutcome::Ignored;
        };
        match event.kind {
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
                if self
                    .selection
                    .as_ref()
                    .is_some_and(|selection| selection.dragging) =>
            {
                let selection = self.selection.as_mut().expect("armed selection");
                selection.end = snapshot.clamp(point);
                selection.moved |= point != selection.anchor;
                if matches!(event.kind, MouseEventKind::Up(_)) {
                    selection.dragging = false;
                    if !selection.moved {
                        let anchor = selection.anchor;
                        self.clear();
                        return MouseOutcome::Click {
                            column: anchor.column,
                            row: anchor.row,
                        };
                    }
                }
                MouseOutcome::Consumed
            }
            MouseEventKind::Down(MouseButton::Right) if snapshot.contains(point) => self
                .text()
                .map_or(MouseOutcome::Ignored, MouseOutcome::Copy),
            // Wheel routing and clipboard effects belong to the caller.
            _ => MouseOutcome::Ignored,
        }
    }

    /// Return plain selected text; an armed click or all-whitespace range is None.
    pub(super) fn text(&self) -> Option<String> {
        let (snapshot, selection) = (self.snapshot.as_ref()?, self.selection.as_ref()?);
        if !selection.moved {
            return None;
        }
        let area = snapshot.buffer.area;
        let (start, end) = selection.bounds();
        let mut lines = Vec::new();
        for row in start.row..=end.row {
            let (left, right) = selection.columns(row, area)?;
            let mut line = String::new();
            for glyph in &snapshot.rows[usize::from(row - area.y)] {
                if glyph.start <= right && glyph.end > left {
                    let symbol = snapshot.buffer[(glyph.start, row)].symbol();
                    // Styling lives in Cell fields, never in copied ANSI. Reject
                    // control-bearing symbols too, even for hand-built buffers.
                    if !symbol.chars().any(char::is_control) {
                        line.push_str(symbol);
                    }
                }
            }
            line.truncate(line.trim_end_matches(' ').len());
            lines.push(line);
        }
        let text = lines.join("\n");
        (!text.chars().all(char::is_whitespace)).then_some(text)
    }

    /// Release the overlay, retaining the last observed pane for the next press.
    pub(super) fn clear(&mut self) {
        self.selection = None;
    }

    /// True after real pointer movement, including an all-whitespace selection.
    pub(super) fn is_selected(&self) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|selection| selection.moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::style::{Color, Style};

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn drag(selection: &mut TextSelection, from: (u16, u16), to: (u16, u16)) {
        assert_eq!(
            selection.mouse(event(
                MouseEventKind::Down(MouseButton::Left),
                from.0,
                from.1
            )),
            MouseOutcome::Consumed
        );
        assert_eq!(
            selection.mouse(event(MouseEventKind::Drag(MouseButton::Left), to.0, to.1)),
            MouseOutcome::Consumed
        );
        assert_eq!(
            selection.mouse(event(MouseEventKind::Up(MouseButton::Left), to.0, to.1)),
            MouseOutcome::Consumed
        );
    }

    fn fixture(lines: &[&str], width: u16) -> (TextSelection, Buffer) {
        let area = Rect::new(2, 3, width, lines.len() as u16);
        let mut buffer = Buffer::empty(area);
        for (offset, line) in lines.iter().enumerate() {
            buffer.set_string(area.x, area.y + offset as u16, *line, Style::default());
        }
        let mut selection = TextSelection::default();
        selection.observe(&buffer, area);
        (selection, buffer)
    }

    #[test]
    fn forward_reverse_and_multiline_are_inclusive() {
        let (mut selection, _) = fixture(&["abc de", "f gh", "ijk"], 8);
        drag(&mut selection, (3, 3), (3, 5));
        assert_eq!(selection.text().as_deref(), Some("bc de\nf gh\nij"));
        drag(&mut selection, (3, 5), (3, 3));
        assert_eq!(selection.text().as_deref(), Some("bc de\nf gh\nij"));
        drag(&mut selection, (6, 3), (3, 3));
        assert_eq!(selection.text().as_deref(), Some("bc d"));
    }

    #[test]
    fn wide_glyphs_emoji_and_combining_marks_are_atomic() {
        let (mut selection, mut buffer) = fixture(&["A中🙂e\u{301}Z"], 12);
        // Start on the CJK continuation, end on the emoji continuation.
        drag(&mut selection, (4, 3), (6, 3));
        assert_eq!(selection.text().as_deref(), Some("中🙂"));
        selection.paint(&mut buffer);
        for column in 3..=6 {
            assert!(buffer[(column, 3)].modifier.contains(Modifier::REVERSED));
        }
        drag(&mut selection, (7, 3), (3, 3));
        assert_eq!(selection.text().as_deref(), Some("中🙂e\u{301}"));
        drag(&mut selection, (3, 3), (7, 3));
        assert_eq!(selection.text().as_deref(), Some("中🙂e\u{301}"));
    }

    #[test]
    fn click_releases_snapshot_and_does_not_copy() {
        let (mut selection, mut buffer) = fixture(&["old"], 5);
        assert_eq!(
            selection.mouse(event(MouseEventKind::Down(MouseButton::Left), 3, 3)),
            MouseOutcome::Consumed
        );
        assert!(!selection.is_selected());
        assert_eq!(selection.text(), None);
        assert_eq!(
            selection.mouse(event(MouseEventKind::Up(MouseButton::Left), 3, 3)),
            MouseOutcome::Click { column: 3, row: 3 }
        );
        buffer.set_string(2, 3, "new", Style::default());
        let live = buffer.clone();
        selection.paint(&mut buffer);
        assert_eq!(buffer, live);
        assert_eq!(
            selection.mouse(event(MouseEventKind::Down(MouseButton::Right), 3, 3)),
            MouseOutcome::Ignored
        );
    }

    #[test]
    fn right_click_copies_only_nonempty_selection_in_pane() {
        let (mut selection, _) = fixture(&["hello"], 8);
        drag(&mut selection, (2, 3), (4, 3));
        assert!(selection.is_selected());
        assert_eq!(
            selection.mouse(event(MouseEventKind::Down(MouseButton::Right), 3, 3)),
            MouseOutcome::Copy("hel".into())
        );
        assert_eq!(
            selection.mouse(event(MouseEventKind::Down(MouseButton::Right), 0, 0)),
            MouseOutcome::Ignored
        );
        assert_eq!(selection.text().as_deref(), Some("hel"));
        selection.clear();
        assert!(!selection.is_selected());
        assert_eq!(selection.text(), None);
    }

    #[test]
    fn snapshot_freezes_only_pane_and_preserves_colors() {
        let area = Rect::new(0, 0, 10, 4);
        let pane = Rect::new(1, 1, 6, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(
            1,
            1,
            "before",
            Style::default().fg(Color::Red).bg(Color::Blue),
        );
        let mut selection = TextSelection::default();
        selection.observe(&buffer, pane);
        selection.mouse(event(MouseEventKind::Down(MouseButton::Left), 1, 1));
        buffer.set_string(1, 1, "during", Style::default());
        selection.observe(&buffer, pane);
        selection.mouse(event(MouseEventKind::Drag(MouseButton::Left), 3, 1));
        selection.mouse(event(MouseEventKind::Up(MouseButton::Left), 3, 1));
        buffer.set_string(1, 1, "after!", Style::default());
        buffer.set_string(0, 3, "composer", Style::default());
        buffer.set_string(0, 0, "header", Style::default());
        let live = buffer.clone();
        selection.observe(&buffer, pane);
        selection.paint(&mut buffer);
        assert_eq!(selection.text().as_deref(), Some("bef"));
        for row in 0..area.bottom() {
            for column in 0..area.right() {
                if !snapshot_contains(pane, column, row) {
                    assert_eq!(buffer[(column, row)], live[(column, row)]);
                }
            }
        }
        assert_eq!(buffer[(1, 1)].symbol(), "b");
        assert_eq!(buffer[(1, 1)].fg, Color::Red);
        assert_eq!(buffer[(1, 1)].bg, Color::Blue);
        assert!(buffer[(1, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!buffer[(4, 1)].modifier.contains(Modifier::REVERSED));
        selection.clear();
        selection.observe(&live, pane);
        drag(&mut selection, (1, 1), (3, 1));
        assert_eq!(selection.text().as_deref(), Some("aft"));
    }

    fn snapshot_contains(area: Rect, column: u16, row: u16) -> bool {
        column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
    }

    #[test]
    fn resize_or_pane_move_clears_old_coordinates() {
        let (mut selection, buffer) = fixture(&["old"], 6);
        drag(&mut selection, (2, 3), (4, 3));
        let mut resized = Buffer::empty(Rect::new(0, 0, 1, 1));
        let untouched = resized.clone();
        selection.paint(&mut resized);
        assert_eq!(resized, untouched);
        selection.observe(&resized, resized.area);
        assert!(!selection.is_selected());
        assert_eq!(selection.text(), None);
        selection.observe(&buffer, buffer.area);
        drag(&mut selection, (2, 3), (4, 3));
        selection.observe(&buffer, Rect::new(3, 3, 3, 1));
        assert!(!selection.is_selected());
        assert_eq!(
            selection.mouse(event(MouseEventKind::Up(MouseButton::Left), 4, 3)),
            MouseOutcome::Ignored
        );
    }

    #[test]
    fn out_of_range_drags_clamp_and_unarmed_events_are_ignored() {
        let (mut selection, _) = fixture(&["abc", "def"], 3);
        assert_eq!(
            selection.mouse(event(MouseEventKind::Down(MouseButton::Left), 0, 0)),
            MouseOutcome::Ignored
        );
        assert_eq!(
            selection.mouse(event(MouseEventKind::Drag(MouseButton::Left), 2, 3)),
            MouseOutcome::Ignored
        );
        drag(&mut selection, (2, 3), (u16::MAX, u16::MAX));
        assert_eq!(selection.text().as_deref(), Some("abc\ndef"));
        drag(&mut selection, (4, 4), (0, 0));
        assert_eq!(selection.text().as_deref(), Some("abc\ndef"));
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollDown,
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            assert_eq!(selection.mouse(event(kind, 2, 3)), MouseOutcome::Ignored);
        }
        assert_eq!(selection.text().as_deref(), Some("abc\ndef"));
    }

    #[test]
    fn empty_geometry_and_whitespace_are_safe() {
        let mut selection = TextSelection::default();
        assert_eq!(
            selection.mouse(event(MouseEventKind::Down(MouseButton::Left), 0, 0)),
            MouseOutcome::Ignored
        );
        let mut empty = Buffer::empty(Rect::default());
        selection.observe(&empty, empty.area);
        selection.paint(&mut empty);
        assert_eq!(
            selection.mouse(event(MouseEventKind::Down(MouseButton::Left), 0, 0)),
            MouseOutcome::Ignored
        );
        let (mut blank, _) = fixture(&["    ", "    "], 4);
        drag(&mut blank, (2, 3), (5, 4));
        assert_eq!(blank.text(), None);
        assert_eq!(
            blank.mouse(event(MouseEventKind::Down(MouseButton::Right), 3, 3)),
            MouseOutcome::Ignored
        );
        let (mut padded, _) = fixture(&[" a  b  "], 8);
        drag(&mut padded, (2, 3), (9, 3));
        assert_eq!(padded.text().as_deref(), Some(" a  b"));
    }

    #[test]
    fn cropped_wide_glyphs_are_not_copied_as_half_glyphs() {
        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "中ab🙂", Style::default());
        let mut selection = TextSelection::default();
        selection.observe(&buffer, Rect::new(1, 0, 4, 1));
        drag(&mut selection, (1, 0), (4, 0));
        assert_eq!(selection.text().as_deref(), Some("ab"));
    }

    #[test]
    fn disjoint_and_extreme_geometry_never_indexes_outside_source() {
        let mut selection = TextSelection::default();
        let mut buffer = Buffer::empty(Rect::new(2, 3, 4, 2));
        for pane in [
            Rect::new(20, 3, 2, 1),
            Rect::new(2, 20, 2, 1),
            Rect::new(2, 3, 0, 1),
        ] {
            selection.observe(&buffer, pane);
            assert_eq!(
                selection.mouse(event(
                    MouseEventKind::Down(MouseButton::Left),
                    pane.x,
                    pane.y
                )),
                MouseOutcome::Ignored
            );
            selection.paint(&mut buffer);
        }
        let (mut selection, _) = fixture(&["abc"], 3);
        drag(&mut selection, (2, 3), (4, 3));
        let empty_width = Buffer::empty(Rect::new(10, 3, 0, 2));
        selection.observe(&empty_width, Rect::new(20, 3, 2, 1));
        assert_eq!(selection.text(), None);
        let mut edge = Buffer::empty(Rect::new(u16::MAX - 2, u16::MAX - 1, 2, 1));
        edge.set_string(u16::MAX - 2, u16::MAX - 1, "ab", Style::default());
        selection.observe(&edge, edge.area);
        drag(
            &mut selection,
            (u16::MAX - 2, u16::MAX - 1),
            (u16::MAX, u16::MAX),
        );
        assert_eq!(selection.text().as_deref(), Some("ab"));
        selection.paint(&mut edge);
    }

    #[test]
    fn release_uses_final_position_even_without_intermediate_drag_event() {
        let (mut selection, _) = fixture(&["abcd"], 4);
        selection.mouse(event(MouseEventKind::Down(MouseButton::Left), 2, 3));
        assert_eq!(
            selection.mouse(event(MouseEventKind::Up(MouseButton::Left), 4, 3)),
            MouseOutcome::Consumed
        );
        assert_eq!(selection.text().as_deref(), Some("abc"));
        selection.mouse(event(MouseEventKind::Down(MouseButton::Left), 3, 3));
        assert_eq!(selection.text(), None);
        selection.mouse(event(MouseEventKind::Drag(MouseButton::Left), 3, 3));
        assert_eq!(
            selection.mouse(event(MouseEventKind::Up(MouseButton::Left), 3, 3)),
            MouseOutcome::Click { column: 3, row: 3 }
        );
    }

    #[test]
    fn control_bearing_cell_symbols_never_escape_into_copied_text() {
        let (mut selection, mut buffer) = fixture(&["ab"], 3);
        buffer[(2, 3)].set_symbol("\u{1b}");
        selection.observe(&buffer, buffer.area);
        drag(&mut selection, (2, 3), (3, 3));
        assert_eq!(selection.text().as_deref(), Some("b"));
    }
}
