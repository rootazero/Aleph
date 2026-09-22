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
use crate::tui::theme::theme;

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
        .border_style(Style::default().fg(theme().border_focused))
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
        Style::default().fg(theme().primary),
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
                    .fg(theme().primary)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_namespace {
                Style::default().fg(theme().tool_name)
            } else {
                Style::default().fg(theme().muted)
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Draw the palette into an 80x24 test backend and return the visible
    /// rows as strings. Pins the widget's visible text so a refactor that
    /// silently drops the title, filter line, or list rows gets caught.
    fn draw_rows(palette: &PaletteState) -> Vec<String> {
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).expect("backend");
        // The input area lives at the bottom of the screen; the palette
        // floats above it. Pick a row that leaves room for the overlay.
        let area = ratatui::layout::Rect::new(0, 23, 80, 1);
        term.draw(|f| render_command_palette(f, palette, area))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        (0..24)
            .map(|y| {
                (0..80)
                    .map(|x| {
                        buf.cell((x, y))
                            .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
                    })
                    .collect::<String>()
            })
            .collect()
    }

    fn row_containing(rows: &[String], needle: &str) -> usize {
        rows.iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} never painted:\n{}", rows.join("\n")))
    }

    /// Sanity check on the helper before pinning visible text — the empty
    /// path returns early and leaves the buffer untouched, so the rows are
    /// blank. If this ever fails the helper is wrong, not the widget.
    #[test]
    fn empty_filtered_renders_nothing() {
        let palette = PaletteState {
            input: String::new(),
            args: String::new(),
            filtered: vec![],
            selected: 0,
            namespace_stack: Vec::new(),
        };
        let rows = draw_rows(&palette);
        assert!(rows.iter().all(|r| r.trim().is_empty()));
    }

    #[test]
    fn max_visible_items_capped() {
        assert_eq!(MAX_VISIBLE_ITEMS, 12);
    }

    /// Pin the visible text of a one-entry palette: the title "Commands",
    /// the filter line `/help`, and the entry's `> ` selector plus label
    /// and hint. A refactor that drops the title bar or the selector
    /// glyph would be invisible until a user noticed — this test makes it
    /// loud.
    #[test]
    fn command_palette_renders_a_single_entry() {
        let palette = PaletteState {
            input: "help".into(),
            args: String::new(),
            filtered: vec![DisplayEntry {
                label: "help".into(),
                hint: "Show help".into(),
                is_namespace: false,
                full_command: "/help".into(),
            }],
            selected: 0,
            namespace_stack: Vec::new(),
        };
        let rows = draw_rows(&palette);

        let title_row = row_containing(&rows, "Commands");
        assert!(
            rows[title_row].contains("Commands"),
            "title row must include 'Commands': {:?}",
            rows[title_row]
        );

        let filter_row = title_row + 1;
        assert!(
            rows[filter_row].contains("/help"),
            "filter row must echo the typed input: {:?}",
            rows[filter_row]
        );

        let entry_row = filter_row + 1;
        let line = &rows[entry_row];
        assert!(
            line.contains("> "),
            "selected entry must show '> ': {line:?}"
        );
        assert!(line.contains("help"), "entry label must appear: {line:?}");
        assert!(
            line.contains("Show help"),
            "entry hint must appear: {line:?}"
        );
    }

    /// The selected entry carries the `> ` selector; every other entry
    /// keeps the leading space so the column lines up. Pin both so a
    /// refactor that drops the selector on the unselected row (or hoists
    /// it onto every row) is caught.
    #[test]
    fn command_palette_highlights_only_the_selected_entry() {
        let palette = PaletteState {
            input: String::new(),
            args: String::new(),
            filtered: vec![
                DisplayEntry {
                    label: "first".into(),
                    hint: "hint one".into(),
                    is_namespace: false,
                    full_command: "/first".into(),
                },
                DisplayEntry {
                    label: "second".into(),
                    hint: "hint two".into(),
                    is_namespace: false,
                    full_command: "/second".into(),
                },
            ],
            // `selected: 1` so the second entry carries the selector.
            selected: 1,
            namespace_stack: Vec::new(),
        };
        let rows = draw_rows(&palette);

        // Locate the two list rows by their hints — the helper asserts
        // these strings are present, which is the failure mode we care about.
        let first_row = row_containing(&rows, "hint one");
        let second_row = row_containing(&rows, "hint two");
        assert_ne!(
            first_row, second_row,
            "both hints must paint on separate rows"
        );

        let first_line = &rows[first_row];
        let second_line = &rows[second_row];
        assert!(
            !first_line.contains("> "),
            "unselected entry must not carry '> ': {first_line:?}"
        );
        // The unselected row keeps the two-space gutter instead of `> `.
        // The widget sits inside a bordered block, so the row may also
        // carry a leading border cell — anchor on "  first" rather than
        // `starts_with("  ")`, which is too strict.
        assert!(
            first_line.contains("  first"),
            "unselected row keeps two-space gutter before the label: {first_line:?}"
        );
        assert!(
            second_line.contains("> "),
            "selected entry must carry '> ': {second_line:?}"
        );
        assert!(
            second_line.contains("second"),
            "selected label must appear: {second_line:?}"
        );
    }

    /// Namespace entries get a chevron marker (`▸ `) so they read as
    /// drill-downs rather than runnable commands. A refactor that drops
    /// the marker makes the palette feel flat — pin it.
    #[test]
    fn command_palette_marks_namespace_entries_with_a_chevron() {
        let palette = PaletteState {
            input: String::new(),
            args: String::new(),
            filtered: vec![DisplayEntry {
                label: "session".into(),
                hint: "Manage sessions".into(),
                is_namespace: true,
                full_command: "/session ".into(),
            }],
            selected: 0,
            namespace_stack: Vec::new(),
        };
        let rows = draw_rows(&palette);

        // The chevron paints in the same row as the hint, since each
        // entry owns one row.
        let entry_row = row_containing(&rows, "Manage sessions");
        let line = &rows[entry_row];
        assert!(
            line.contains('\u{25b8}'),
            "namespace entry must carry the chevron marker (▸): {line:?}"
        );
        assert!(
            line.contains("session"),
            "namespace label must still appear: {line:?}"
        );
    }
}
