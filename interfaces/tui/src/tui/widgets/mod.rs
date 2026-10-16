pub mod agent_panel;
pub mod agents_overlay;
pub mod agents_panel;
pub mod btw_panel;
pub mod chat_area;
pub mod command_palette;
pub mod context_overlay;
pub mod dialog;
pub mod header;
pub mod hint_line;
pub mod input_area;
pub mod provider_picker;
pub mod session_picker;
pub mod status_bar;
pub mod sticky_user;
pub mod tasks_panel;
pub mod tool_row;

/// Clear previous content and restore the active theme beneath an overlay.
pub(crate) fn clear_overlay(frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    frame.render_widget(ratatui::widgets::Clear, area);
    let colors = crate::tui::theme::theme();
    frame.buffer_mut().set_style(
        area,
        ratatui::style::Style::default()
            .fg(colors.primary)
            .bg(colors.background),
    );
}
