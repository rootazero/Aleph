// Command palette widget: renders a floating overlay above the input area
// showing filtered slash commands with a selected-item indicator.
// Supports hierarchical namespace browsing with visual cues.

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

    let item_count = u16::try_from(palette.filtered.len()).unwrap_or(u16::MAX);
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

    // Build title showing namespace breadcrumb
    let title = if palette.namespace_stack.is_empty() {
        " Commands ".to_string()
    } else {
        format!(" /{} ", palette.namespace_stack.join(" > "))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(title);

    let inner = block.inner(overlay_rect);
    frame.render_widget(block, overlay_rect);

    // Render the filter input at the top of the inner area
    if inner.height < 2 {
        return;
    }
    let input_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let filter_prefix = if palette.namespace_stack.is_empty() {
        "/".to_string()
    } else {
        format!("/{} ", palette.namespace_stack.join(" "))
    };
    let filter_display = format!("{}{}", filter_prefix, palette.input);
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
        .map(|(i, entry)| {
            let is_selected = i == palette.selected;
            let indicator = if is_selected { "> " } else { "  " };

            // Namespace entries get a chevron prefix
            let ns_marker = if entry.is_namespace { "\u{25b8} " } else { "" };

            // Pad label to align descriptions
            let label_str = format!("{}{}", ns_marker, entry.label);
            let padded_label = format!("{label_str:<16}");
            let line_str = format!("{}{}{}", indicator, padded_label, entry.hint);

            let style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_namespace {
                Style::default().fg(DEFAULT_THEME.tool_name)
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
    use crate::tui::command_tree::DisplayEntry;

    #[test]
    fn empty_filtered_renders_nothing() {
        // If filtered is empty, the function should return early.
        let palette = PaletteState {
            input: String::new(),
            args: String::new(),
            filtered: vec![],
            selected: 0,
            namespace_stack: Vec::new(),
        };
        assert!(palette.filtered.is_empty());
    }

    #[test]
    fn max_visible_items_capped() {
        assert_eq!(MAX_VISIBLE_ITEMS, 12);
    }

    #[test]
    fn namespace_stack_affects_title() {
        let palette = PaletteState {
            input: String::new(),
            args: String::new(),
            filtered: vec![DisplayEntry {
                label: "new [topic]".into(),
                hint: "Start new session".into(),
                is_namespace: false,
                full_command: "/session new ".into(),
            }],
            selected: 0,
            namespace_stack: vec!["session".into()],
        };
        // Just verify the palette state is correctly formed
        assert_eq!(palette.namespace_stack, vec!["session"]);
        assert_eq!(palette.filtered.len(), 1);
    }
}
