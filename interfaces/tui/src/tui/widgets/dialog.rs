// Dialog widget: renders an inline confirmation dialog for AskUser events
// as a centered overlay with a question and selectable options.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{ApprovalState, DialogState};
use crate::tui::theme::DEFAULT_THEME;

/// Render the confirmation dialog as a centered overlay.
pub fn render_dialog(frame: &mut Frame, dialog: &DialogState, area: Rect) {
    // Calculate dialog dimensions
    let dialog_width = area.width.clamp(20, 50);
    let option_count = u16::try_from(dialog.options.len()).unwrap_or(u16::MAX);
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
        Constraint::Length(1),                   // blank line
        Constraint::Min(2),                      // question
        Constraint::Length(1),                   // blank line
        Constraint::Length(option_count.max(1)), // options
        Constraint::Length(1),                   // hint line
    ])
    .split(inner);

    let question_area = chunks.get(1).copied().unwrap_or_default();
    let options_area = chunks.get(3).copied().unwrap_or_default();
    let hint_area = chunks.get(4).copied().unwrap_or_default();

    // Render question
    let question = Paragraph::new(Line::from(Span::styled(
        dialog.question.clone(),
        Style::default().fg(DEFAULT_THEME.primary),
    )))
    .wrap(Wrap { trim: true });
    frame.render_widget(question, question_area);

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
            Line::from(Span::styled(format!("  [{}] {}", i + 1, opt), style))
        })
        .collect();

    let options_widget = Paragraph::new(option_lines);
    frame.render_widget(options_widget, options_area);

    // Render hint
    let hint = Paragraph::new(Line::from(Span::styled(
        "Press number key to select, Enter to confirm".to_string(),
        Style::default().fg(DEFAULT_THEME.muted),
    )));
    frame.render_widget(hint, hint_area);
}

/// Render the tool-approval overlay: a red-bordered modal a parked Ask-tier run
/// is waiting on. Deliberately distinct from [`render_dialog`] (AskUser) so a
/// security decision never looks like an ordinary agent question. Shares only
/// [`centered_rect`] — the layout is copied rather than abstracted (two
/// consumers; the wrong abstraction would cost more than the duplication).
pub fn render_approval(frame: &mut Frame, approval: &ApprovalState, area: Rect) {
    let width = area.width.clamp(28, 60);
    let option_count = u16::try_from(approval.decisions.len()).unwrap_or(3);
    // 2 borders + 1 blank + question (2) + 1 blank + options + 1 hint
    let height = (option_count.saturating_add(7)).min(area.height);
    let rect = centered_rect(width, height, area);

    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.error))
        .title(" \u{26a0} Tool approval required ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(1),                   // blank line
        Constraint::Min(2),                      // command (+ reason)
        Constraint::Length(1),                   // blank line
        Constraint::Length(option_count.max(1)), // decisions
        Constraint::Length(1),                   // hint line
    ])
    .split(inner);

    let question_area = chunks.get(1).copied().unwrap_or_default();
    let options_area = chunks.get(3).copied().unwrap_or_default();
    let hint_area = chunks.get(4).copied().unwrap_or_default();

    // Command being gated, plus the server's reason (dim) when present.
    let mut question_lines = vec![Line::from(Span::styled(
        approval.command.clone(),
        Style::default().fg(DEFAULT_THEME.primary),
    ))];
    if let Some(reason) = &approval.reason {
        question_lines.push(Line::from(Span::styled(
            format!("Reason: {reason}"),
            Style::default().fg(DEFAULT_THEME.muted),
        )));
    }
    frame.render_widget(
        Paragraph::new(question_lines).wrap(Wrap { trim: true }),
        question_area,
    );

    let option_lines: Vec<Line> = approval
        .decisions
        .iter()
        .enumerate()
        .map(|(i, (label, _decision))| {
            let style = if i == approval.selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(DEFAULT_THEME.muted)
            };
            Line::from(Span::styled(format!("  [{}] {}", i + 1, label), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(option_lines), options_area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Number key or ↑↓ + Enter to decide".to_string(),
            Style::default().fg(DEFAULT_THEME.muted),
        ))),
        hint_area,
    );
}

/// Calculate a centered rect within the given area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
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
