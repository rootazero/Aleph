// Input area widget: wraps tui_textarea::TextArea with styled borders,
// focus-aware coloring, and dynamic height calculation.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders},
    Frame,
};
use tui_textarea::TextArea;

use crate::tui::theme::DEFAULT_THEME;

pub struct InputWidget<'a> {
    pub textarea: &'a TextArea<'a>,
    pub focused: bool,
}

impl<'a> InputWidget<'a> {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let border_color = if self.focused {
            DEFAULT_THEME.border_focused
        } else {
            DEFAULT_THEME.border
        };

        let title = if self.focused {
            " Input (Enter=send, Shift+Enter=newline) "
        } else {
            " Input "
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(title);

        // Clone the textarea so we can set a block on it without mutating the original
        let mut ta = self.textarea.clone();
        ta.set_block(block);

        // Set cursor line style based on focus
        if self.focused {
            ta.set_cursor_line_style(Style::default());
            ta.set_cursor_style(Style::default().bg(Color::White).fg(Color::Black));
        } else {
            ta.set_cursor_line_style(Style::default());
            ta.set_cursor_style(Style::default());
        }

        frame.render_widget(&ta, area);
    }
}

/// Calculate the height for the input area based on the number of lines
/// in the textarea, clamped between min and max.
///
/// The height includes 2 extra rows for the top and bottom borders.
pub fn input_height(textarea: &TextArea, min: u16, max: u16) -> u16 {
    let line_count = textarea.lines().len() as u16;
    let desired = line_count.saturating_add(2); // +2 for borders
    desired.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_height_single_line() {
        let ta = TextArea::default();
        // Default textarea has 1 line, so height = 1 + 2 = 3
        let h = input_height(&ta, 3, 10);
        assert_eq!(h, 3);
    }

    #[test]
    fn input_height_clamped_min() {
        let ta = TextArea::default();
        // Even if textarea is small, min is respected
        let h = input_height(&ta, 5, 10);
        assert_eq!(h, 5);
    }

    #[test]
    fn input_height_clamped_max() {
        let lines: Vec<String> = (0..20).map(|i| format!("line {}", i)).collect();
        let ta = TextArea::new(lines);
        // 20 lines + 2 = 22, but max is 10
        let h = input_height(&ta, 3, 10);
        assert_eq!(h, 10);
    }
}
