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
    dialog::{render_approval, render_dialog},
    input_area::{input_height, InputWidget},
    session_picker::render_session_picker,
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

    let chat_area = chunks.first().copied().unwrap_or_default();
    let input_area = chunks.get(1).copied().unwrap_or_default();
    let status_area = chunks.get(2).copied().unwrap_or_default();

    // Chat area
    render_chat_area(frame, state, chat_area);

    // Input area
    let input_widget = InputWidget {
        textarea,
        focused: state.focus == Focus::Input,
    };
    input_widget.render(frame, input_area);

    // Status bar
    let status = StatusBar {
        model: &state.model_name,
        session: &state.session_key,
        tokens: state.total_tokens,
        context_gauge: state.context_gauge,
        cache_stat: state.cache_stat,
        cache_stat_agent: state.cache_stat_agent.as_deref(),
        is_connected: state.is_connected,
        tool_progress_mode: state.tool_progress_mode,
        spinner_frame: state.spinner_frame,
        // Only while a run is genuinely in flight (belt-and-suspenders: the
        // timer is cleared at every run-end site, but gate on current_run too).
        run_elapsed: state
            .run_started_at
            .filter(|_| state.current_run.is_some())
            .map(|t| t.elapsed()),
    };
    status.render(frame, status_area);

    // Overlays (rendered last, on top)
    if let Some(palette) = &state.palette {
        render_command_palette(frame, palette, input_area);
    }
    if let Some(picker) = &state.session_picker {
        render_session_picker(frame, picker, input_area);
    }
    if let Some(dialog) = &state.dialog {
        render_dialog(frame, dialog, frame.area());
    }
    // Approval overlay renders above everything — a parked run is waiting on it.
    if let Some(approval) = &state.approval {
        render_approval(frame, approval, frame.area());
    }
}
