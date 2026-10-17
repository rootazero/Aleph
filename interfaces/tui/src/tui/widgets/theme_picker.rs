//! Local appearance picker. No rows or feedback are added to chat history.
use crate::tui::{
    app::ThemePickerState,
    theme::{current_theme_name, theme},
};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn render_theme_picker(frame: &mut Frame, picker: &ThemePickerState, input: Rect) {
    let colors = theme();
    let height = (u16::try_from(picker.names.len()).unwrap_or(12).min(12) + 6).min(input.y);
    let area = Rect::new(
        input.x,
        input.y.saturating_sub(height),
        input.width.min(90),
        height,
    );
    super::clear_overlay(frame, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colors.border_focused))
        .style(Style::default().fg(colors.primary).bg(colors.surface))
        .title(" TUI themes · ↑↓ select · Enter apply · Esc close ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let list_height = inner.height.saturating_sub(3);
    let active = current_theme_name();
    let items: Vec<_> = picker
        .names
        .iter()
        .map(|name| {
            let label = if name == &active {
                format!("{name} · active")
            } else {
                name.clone()
            };
            ListItem::new(Line::from(Span::raw(label)))
        })
        .collect();
    let mut selection = ListState::default();
    selection.select(Some(picker.selected));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("› ")
            .highlight_style(Style::default().fg(colors.heading).bg(colors.selection_bg)),
        Rect::new(inner.x, inner.y, inner.width, list_height),
        &mut selection,
    );
    let feedback = picker
        .feedback
        .as_deref()
        .unwrap_or("Local appearance only · chat history is unchanged");
    let diagnostic = picker
        .diagnostics
        .first()
        .map_or(String::new(), |d| format!("\nTheme skipped: {d}"));
    frame.render_widget(
        Paragraph::new(format!("{feedback}{diagnostic}"))
            .style(Style::default().fg(colors.muted))
            .wrap(Wrap { trim: false }),
        Rect::new(
            inner.x,
            inner.y + list_height,
            inner.width,
            inner.height - list_height,
        ),
    );
}
