// Render function: layout splitting and widget delegation.
//
// Splits the terminal frame into three vertical sections (chat, input, status bar),
// then delegates rendering to each widget. Overlays (command palette, dialog)
// are rendered last so they appear on top.

use ratatui::layout::{Constraint, Layout};
use ratatui::Frame;
use tui_textarea::TextArea;

use crate::tui::app::{AppState, Focus};
use crate::tui::widgets::{
    chat_area::render_chat_area,
    command_palette::render_command_palette,
    dialog::render_dialog,
    input_area::{input_height, InputWidget},
    status_bar::StatusBar,
};

/// Render the full TUI layout: chat area, input area, status bar, and overlays.
pub fn render(frame: &mut Frame, state: &AppState, textarea: &TextArea) {
    let input_h = input_height(textarea, 3, 8);

    let chunks = Layout::vertical([
        Constraint::Min(5),          // Chat area
        Constraint::Length(input_h), // Input area
        Constraint::Length(1),       // Status bar
    ])
    .split(frame.area());

    // Chat area
    render_chat_area(frame, state, chunks[0]);

    // Input area
    let input_widget = InputWidget {
        textarea,
        focused: state.focus == Focus::Input,
    };
    input_widget.render(frame, chunks[1]);

    // Status bar
    let status = StatusBar {
        model: &state.model_name,
        session: &state.session_key,
        tokens: state.total_tokens,
        is_connected: state.is_connected,
    };
    status.render(frame, chunks[2]);

    // Overlays (rendered last, on top)
    if let Some(palette) = &state.palette {
        render_command_palette(frame, palette, chunks[1]);
    }
    if let Some(dialog) = &state.dialog {
        render_dialog(frame, dialog, frame.area());
    }
}
