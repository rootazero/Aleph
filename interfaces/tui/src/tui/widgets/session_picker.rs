// Session picker widget: renders a floating overlay above the input area
// listing the sessions returned by `sessions.list`, with a filter line and a
// selected-item indicator. Confirming an entry switches to that session.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::app::SessionPickerState;
use crate::tui::theme::DEFAULT_THEME;

/// Maximum number of visible items in the picker overlay.
const MAX_VISIBLE_ITEMS: u16 = 12;

/// Render the session picker overlay. `area` is the input area's Rect — the
/// picker floats above it (same placement as the command palette).
pub fn render_session_picker(frame: &mut Frame, picker: &SessionPickerState, area: Rect) {
    let item_count = u16::try_from(picker.filtered.len()).unwrap_or(u16::MAX);
    let visible_count = item_count.clamp(1, MAX_VISIBLE_ITEMS);
    // Height = visible items + 2 (borders) + 1 (filter line at top)
    let overlay_height = visible_count.saturating_add(3);

    let overlay_y = area.y.saturating_sub(overlay_height);
    let overlay_width = area.width.min(60);
    let overlay_rect = Rect::new(area.x, overlay_y, overlay_width, overlay_height);

    frame.render_widget(Clear, overlay_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(" Switch session ");

    let inner = block.inner(overlay_rect);
    frame.render_widget(block, overlay_rect);

    if inner.height < 2 {
        return;
    }

    // Filter line at the top.
    let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let filter_display = format!("filter: {}", picker.input);
    let filter_line = Paragraph::new(Line::from(Span::styled(
        filter_display,
        Style::default().fg(DEFAULT_THEME.primary),
    )));
    frame.render_widget(filter_line, filter_area);

    // Entry list below the filter line.
    let list_area = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );

    if picker.filtered.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  (no matching sessions)",
            Style::default().fg(DEFAULT_THEME.muted),
        )));
        frame.render_widget(empty, list_area);
        return;
    }

    let items: Vec<ListItem> = picker
        .filtered
        .iter()
        .enumerate()
        .filter_map(|(row, &entry_idx)| {
            let entry = picker.entries.get(entry_idx)?;
            let is_selected = row == picker.selected;
            let indicator = if is_selected { "> " } else { "  " };
            let line_str = format!("{}{}", indicator, entry.label);
            let style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(DEFAULT_THEME.muted)
            };
            Some(ListItem::new(Line::from(Span::styled(line_str, style))))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(picker.selected));

    let list = List::new(items);
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::SessionEntry;

    #[test]
    fn max_visible_items_capped() {
        assert_eq!(MAX_VISIBLE_ITEMS, 12);
    }

    #[test]
    fn picker_state_holds_entries() {
        let picker = SessionPickerState {
            input: String::new(),
            entries: vec![SessionEntry {
                key: "s1".into(),
                label: "Session one (3 msgs)".into(),
            }],
            filtered: vec![0],
            selected: 0,
        };
        assert_eq!(picker.filtered.len(), 1);
        assert_eq!(picker.entries[0].key, "s1");
    }
}
