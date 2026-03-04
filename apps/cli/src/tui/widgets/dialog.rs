// Dialog widget: renders an inline confirmation dialog for AskUser events
// as a centered overlay with a question and selectable options.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::DialogState;
use crate::tui::theme::DEFAULT_THEME;

/// Render the confirmation dialog as a centered overlay.
pub fn render_dialog(frame: &mut Frame, dialog: &DialogState, area: Rect) {
    // Calculate dialog dimensions
    let dialog_width = area.width.min(50).max(20);
    let option_count = dialog.options.len() as u16;
    // Height: 2 borders + 1 blank + question lines (estimate 2) + 1 blank + options + 1 hint
    let dialog_height = (option_count.saturating_add(7)).min(area.height);

    // Center the dialog
    let dialog_rect = centered_rect(dialog_width, dialog_height, area);

    // Clear background behind the dialog
    frame.render_widget(Clear, dialog_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.warning))
        .title(" Agent needs your input ");

    let inner = block.inner(dialog_rect);
    frame.render_widget(block, dialog_rect);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Split inner area into question + options + hint
    let chunks = Layout::vertical([
        Constraint::Length(1), // blank line
        Constraint::Min(2),   // question
        Constraint::Length(1), // blank line
        Constraint::Length(option_count.max(1)), // options
        Constraint::Length(1), // hint line
    ])
    .split(inner);

    // Render question
    let question = Paragraph::new(Line::from(Span::styled(
        dialog.question.clone(),
        Style::default().fg(DEFAULT_THEME.primary),
    )))
    .wrap(Wrap { trim: true });
    frame.render_widget(question, chunks[1]);

    // Render options
    let option_lines: Vec<Line> = dialog
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let is_selected = i == dialog.selected;
            let style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(DEFAULT_THEME.muted)
            };
            Line::from(Span::styled(
                format!("  [{}] {}", i + 1, opt),
                style,
            ))
        })
        .collect();

    let options_widget = Paragraph::new(option_lines);
    frame.render_widget(options_widget, chunks[3]);

    // Render hint
    let hint = Paragraph::new(Line::from(Span::styled(
        "Press number key to select, Enter to confirm".to_string(),
        Style::default().fg(DEFAULT_THEME.muted),
    )));
    frame.render_widget(hint, chunks[4]);
}

/// Calculate a centered rect within the given area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area
        .x
        .saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_in_large_area() {
        let area = Rect::new(0, 0, 100, 40);
        let r = centered_rect(50, 10, area);
        assert_eq!(r.x, 25);
        assert_eq!(r.y, 15);
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 10);
    }

    #[test]
    fn centered_rect_clamps_to_area() {
        let area = Rect::new(0, 0, 20, 10);
        let r = centered_rect(50, 20, area);
        // Width and height should be clamped
        assert_eq!(r.width, 20);
        assert_eq!(r.height, 10);
    }
}
