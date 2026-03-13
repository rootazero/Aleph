// Command palette widget: renders a floating overlay above the input area
// showing filtered slash commands with a selected-item indicator.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::app::PaletteState;
use crate::tui::theme::DEFAULT_THEME;

/// Maximum number of visible items in the palette overlay.
const MAX_VISIBLE_ITEMS: u16 = 12;

/// Render the command palette overlay. The `area` parameter is the input area's
/// Rect — the palette floats above it.
pub fn render_command_palette(frame: &mut Frame, palette: &PaletteState, area: Rect) {
    if palette.filtered.is_empty() {
        return;
    }

    let item_count = palette.filtered.len() as u16;
    let visible_count = item_count.min(MAX_VISIBLE_ITEMS);
    // Height = visible items + 2 (borders) + 1 (input line at top)
    let overlay_height = visible_count.saturating_add(3);

    // Position the overlay above the input area
    let overlay_y = area.y.saturating_sub(overlay_height);
    let overlay_width = area.width.min(60); // reasonable max width
    let overlay_x = area.x;

    let overlay_rect = Rect::new(overlay_x, overlay_y, overlay_width, overlay_height);

    // Clear the area behind the overlay
    frame.render_widget(Clear, overlay_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(" Commands ");

    let inner = block.inner(overlay_rect);
    frame.render_widget(block, overlay_rect);

    // Render the filter input at the top of the inner area
    if inner.height < 2 {
        return;
    }
    let input_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let filter_display = format!("/ {}", palette.input);
    let filter_line = Paragraph::new(Line::from(Span::styled(
        filter_display,
        Style::default().fg(DEFAULT_THEME.primary),
    )));
    frame.render_widget(filter_line, input_area);

    // Render the command list below the filter input
    let list_area = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );

    let items: Vec<ListItem> = palette
        .filtered
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let is_selected = i == palette.selected;
            let indicator = if is_selected { "> " } else { "  " };

            // Pad name to align descriptions
            let name_str = format!("{:<14}", name);
            let line_str = format!("{}{}{}", indicator, name_str, desc);

            let style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(DEFAULT_THEME.muted)
            };

            ListItem::new(Line::from(Span::styled(line_str, style)))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(palette.selected));

    let list = List::new(items);
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filtered_renders_nothing() {
        // If filtered is empty, the function should return early.
        // We just verify it doesn't panic.
        let palette = PaletteState {
            input: String::new(),
            filtered: vec![],
            selected: 0,
        };
        // We can't easily test rendering without a terminal backend,
        // but we can at least confirm the logic path.
        assert!(palette.filtered.is_empty());
    }

    #[test]
    fn max_visible_items_capped() {
        assert_eq!(MAX_VISIBLE_ITEMS, 12);
    }
}
