// The composer: a flat rule, a gold `❯`, and the text.
//
// # Why the box went away
//
// The old input was a `Block::bordered` with the title
// `Input (Enter=send, \+Enter=newline)`. Three costs: two of the terminal's
// columns and two of its rows went to drawing a rectangle; the title was the
// only place those two key names were written down, so they were invisible
// the moment the box was; and a bordered box reads as a form field, which is
// the wrong affordance for a prompt.
//
// What replaced each part: the rule marks the boundary in one row, the `❯`
// marks where typing goes, and the key names moved to the hint line
// (`widgets::hint_line`), which is where every other key name lives.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use shared_ui_logic::transcript::SemanticColor;
use tui_textarea::TextArea;

use crate::tui::theme::palette;

/// The prompt caret. `SemanticColor::Prompt` is the gold Phase A minted for
/// exactly this and had no reader until now.
pub const CARET: &str = "\u{276f}"; // ❯

/// Columns reserved to the left of the text for [`CARET`] plus its space.
pub const GUTTER: u16 = 2;

/// The empty-composer suggestion.
///
/// A concrete sentence rather than a category ("ask a question"): the point of
/// the affordance is to show what this thing is for, and the `/` half is the
/// one discovery path a first-time reader has no other way to find.
pub const PLACEHOLDER: &str = "Try \"what changed on this branch?\"  \u{b7}  / for commands";

pub struct InputWidget<'a> {
    pub textarea: &'a TextArea<'a>,
    pub focused: bool,
}

impl InputWidget<'_> {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(u16::from(area.height >= 3)),
        ])
        .split(area);
        let rule_area = rows.first().copied().unwrap_or_default();
        let text_area = rows.get(1).copied().unwrap_or_default();

        // The rule: one row, full width, dim. It brightens with focus for the
        // same reason the border used to — it is the only thing left that can
        // say where the keyboard is.
        let rule_color = if self.focused {
            crate::tui::theme::theme().border_focused
        } else {
            crate::tui::theme::theme().border
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "\u{2500}".repeat(rule_area.width as usize),
                Style::default().fg(rule_color),
            ))),
            rule_area,
        );

        if let Some(bottom) = rows.get(2).filter(|r| r.height > 0) {
            frame.render_widget(
                Paragraph::new("─".repeat(bottom.width as usize))
                    .style(Style::default().fg(rule_color)),
                *bottom,
            );
        }

        if text_area.height == 0 || text_area.width <= GUTTER {
            return;
        }
        let cols =
            Layout::horizontal([Constraint::Length(GUTTER), Constraint::Min(0)]).split(text_area);
        let caret_area = cols.first().copied().unwrap_or_default();
        let ta_area = cols.get(1).copied().unwrap_or_default();

        // The caret marks the FIRST line only: on a multi-line draft the
        // continuation rows are the same message, and repeating the mark
        // would read as several prompts.
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{CARET} "),
                Style::default()
                    .fg(palette().color(SemanticColor::Prompt))
                    .add_modifier(Modifier::BOLD),
            ))),
            Rect {
                height: 1,
                ..caret_area
            },
        );

        // Clone the textarea so styling it does not mutate the caller's.
        let mut ta = self.textarea.clone();
        ta.set_placeholder_text(PLACEHOLDER);
        // The hint must never read as typed text: a dark, background-blended
        // grey where the palette is RGB, the DIM attribute elsewhere.
        let palette = palette();
        ta.set_placeholder_style(match palette.faint(SemanticColor::Dim) {
            Some(grey) => Style::default().fg(grey),
            None => Style::default()
                .fg(palette.color(SemanticColor::Dim))
                .add_modifier(Modifier::DIM),
        });
        let colors = crate::tui::theme::theme();
        ta.set_style(Style::default().fg(colors.primary).bg(colors.background));
        ta.set_selection_style(Style::default().bg(colors.selection_bg));
        ta.set_cursor_line_style(Style::default());
        if self.focused {
            ta.set_cursor_style(
                Style::default()
                    .fg(colors.primary)
                    .bg(colors.background)
                    .add_modifier(Modifier::REVERSED),
            );
        } else {
            ta.set_cursor_style(Style::default());
        }
        frame.render_widget(&ta, ta_area);

        // Park the terminal's REAL cursor on the painted one. The block above
        // is only a styled cell; the IME composition window (Chinese/Japanese
        // pinyin, dead keys) is anchored at the real cursor, which otherwise
        // sits wherever the last frame write ended — bottom-right — and the
        // pre-edit text shows up there instead of at the caret. The painted
        // cell is located in the buffer rather than recomputed, so textarea
        // scrolling and wide (CJK) glyphs cannot make the two disagree.
        if self.focused {
            if let Some(pos) = painted_cursor(frame.buffer_mut(), ta_area) {
                frame.set_cursor_position(pos);
            }
        }
    }
}

/// The cell tui-textarea painted with the (REVERSED) cursor style, if any.
fn painted_cursor(buf: &ratatui::buffer::Buffer, area: Rect) -> Option<ratatui::layout::Position> {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if buf
                .cell((x, y))
                .is_some_and(|c| c.modifier.contains(Modifier::REVERSED))
            {
                return Some(ratatui::layout::Position { x, y });
            }
        }
    }
    None
}

/// Height of the composer chunk: two thin rules plus the draft, clamped.
///
/// `min`/`max` count the whole chunk, rule included, so a caller's numbers
/// keep meaning "rows of screen this thing may occupy".
pub fn input_height(textarea: &TextArea, min: u16, max: u16) -> u16 {
    let line_count = u16::try_from(textarea.lines().len()).unwrap_or(u16::MAX);
    let desired = line_count.saturating_add(2); // two thin rules
    desired.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(textarea: &TextArea<'_>, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("backend");
        term.draw(|f| {
            InputWidget {
                textarea,
                focused: true,
            }
            .render(f, f.area());
        })
        .expect("draw");
        term.backend().buffer().clone()
    }

    fn row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn input_height_single_line() {
        let ta = TextArea::default();
        // One draft line between two thin rules.
        assert_eq!(input_height(&ta, 3, 10), 3);
    }

    #[test]
    fn input_height_clamped_min() {
        let ta = TextArea::default();
        assert_eq!(input_height(&ta, 5, 10), 5);
    }

    #[test]
    fn input_height_clamped_max() {
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let ta = TextArea::new(lines);
        assert_eq!(input_height(&ta, 3, 10), 10);
    }

    /// A draft that grows takes rows with it, up to the cap — the "grows with
    /// content" half of the spec, which the old fixed box could not do
    /// without also growing its borders.
    #[test]
    fn the_composer_grows_one_row_per_line() {
        for n in 1..=6usize {
            let ta = TextArea::new((0..n).map(|i| format!("l{i}")).collect());
            assert_eq!(
                input_height(&ta, 2, 9),
                u16::try_from(n).expect("small") + 2,
                "{n} lines"
            );
        }
    }

    /// The shape: a rule across the top, the caret at column 0 of the first
    /// text row, and no box-drawing corners anywhere.
    ///
    /// # When this goes red
    ///
    /// Putting a `Block` back on the textarea (the corners return and the
    /// text shifts a column), or dropping the gutter split (the caret and
    /// the text land on the same cell).
    #[test]
    fn the_composer_is_a_rule_and_a_caret_not_a_box() {
        let ta = TextArea::from(vec!["hello".to_string()]);
        let buf = draw(&ta, 20, 3);
        assert_eq!(
            row(&buf, 0),
            "\u{2500}".repeat(20),
            "row 0 must be the rule"
        );
        assert_eq!(row(&buf, 1).trim_end(), format!("{CARET} hello"));
        assert_eq!(row(&buf, 2), "─".repeat(20));
        for y in 0..3 {
            let line = row(&buf, y);
            for corner in ['\u{250c}', '\u{2510}', '\u{2514}', '\u{2518}', '\u{2502}'] {
                assert!(!line.contains(corner), "row {y} still draws a box: {line}");
            }
        }
    }

    /// The empty composer says what to type. The placeholder is the ONLY
    /// remaining pointer to `/`, so its absence is not cosmetic.
    #[test]
    fn an_empty_composer_shows_the_suggestion() {
        let buf = draw(&TextArea::default(), 60, 2);
        let line = row(&buf, 1);
        assert!(line.contains("Try \""), "{line}");
        assert!(line.contains("/ for commands"), "{line}");
    }

    /// A composer with a draft in it shows the draft, not the suggestion.
    #[test]
    fn a_draft_replaces_the_suggestion() {
        let buf = draw(&TextArea::from(vec!["x".to_string()]), 60, 2);
        let line = row(&buf, 1);
        assert!(!line.contains("Try \""), "{line}");
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    use ratatui::{backend::TestBackend, layout::Position, Terminal};

    fn real_cursor(textarea: &TextArea<'_>, focused: bool) -> Option<Position> {
        let mut term = Terminal::new(TestBackend::new(40, 5)).expect("backend");
        let mut at = None;
        term.draw(|f| {
            InputWidget { textarea, focused }.render(f, f.area());
        })
        .expect("draw");
        if focused {
            at = Some(term.get_cursor_position().expect("cursor"));
        }
        at
    }

    /// The IME pre-edit window follows the real cursor, so it must sit on
    /// the caret from the very first keystroke — the empty, placeholder
    /// frame included — not wherever the previous write ended.
    #[test]
    fn real_cursor_tracks_the_caret_from_the_empty_frame_on() {
        let empty = TextArea::default();
        assert_eq!(
            real_cursor(&empty, true),
            Some(Position { x: GUTTER, y: 1 })
        );
        let mut typed = TextArea::default();
        typed.insert_str("你好ab");
        // Two wide glyphs (4 cells) + two ASCII cells after the gutter.
        assert_eq!(
            real_cursor(&typed, true),
            Some(Position {
                x: GUTTER + 6,
                y: 1
            })
        );
    }
}
