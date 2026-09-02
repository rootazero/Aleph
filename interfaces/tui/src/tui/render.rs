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
    agents_overlay::render_agents_overlay,
    agents_panel::{agents_panel_height, render_agents_panel},
    btw_panel::render_btw_panel,
    chat_area::render_chat_area,
    command_palette::render_command_palette,
    dialog::{render_approval, render_dialog},
    input_area::{input_height, InputWidget},
    provider_picker::render_provider_picker,
    session_picker::render_session_picker,
    status_bar::StatusBar,
    tasks_panel::{render_tasks_panel, tasks_panel_height},
};

/// Render the full TUI layout: chat area, docked tasks/agents panels, input
/// area, status bar, and overlays.
pub fn render(frame: &mut Frame, state: &mut AppState, textarea: &TextArea) {
    let input_h = input_height(textarea, 3, 8);
    let tasks_h = tasks_panel_height(state.plan.as_ref(), state.tasks_panel_visible);
    let agents_h = agents_panel_height(&state.agents);

    let chunks = Layout::vertical([
        Constraint::Min(5),           // Chat area
        Constraint::Length(tasks_h),  // Tasks panel (0 = hidden)
        Constraint::Length(agents_h), // Agents panel (0 = hidden)
        Constraint::Length(input_h),  // Input area
        Constraint::Length(1),        // Status bar
    ])
    .split(frame.area());

    let chat_area = chunks.first().copied().unwrap_or_default();
    let tasks_area = chunks.get(1).copied().unwrap_or_default();
    let agents_area = chunks.get(2).copied().unwrap_or_default();
    let input_area = chunks.get(3).copied().unwrap_or_default();
    let status_area = chunks.get(4).copied().unwrap_or_default();

    // Chat area
    render_chat_area(frame, state, chat_area);

    // Docked tasks + agents panels (each collapses to zero rows when empty).
    if let Some(plan) = state.plan.as_ref().filter(|_| tasks_h > 0) {
        render_tasks_panel(frame, plan, tasks_area);
    }
    if agents_h > 0 {
        let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
        render_agents_panel(
            frame,
            &state.agents,
            state.spinner_frame,
            now_ms,
            agents_area,
        );
    }

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
        running_agents: state.running_agent_count(),
        is_connected: state.is_connected,
        tool_progress_mode: state.tool_progress_mode,
        spinner_frame: state.spinner_frame,
        // Only while a run is genuinely in flight (belt-and-suspenders: the
        // timer is cleared at every run-end site, but gate on current_run too).
        run_elapsed: state
            .run_started_at
            .filter(|_| state.current_run.is_some())
            .map(|t| t.elapsed()),
        knobs: state.session_knobs(),
    };
    status.render(frame, status_area);

    // Overlays (rendered last, on top)
    if let Some(palette) = &state.palette {
        render_command_palette(frame, palette, input_area);
    }
    if let Some(picker) = &state.session_picker {
        render_session_picker(frame, picker, input_area);
    }
    if let Some(picker) = &state.provider_picker {
        render_provider_picker(frame, picker, input_area);
    }
    // Agents overlay (list floats above the input like the pickers; the
    // per-agent run view centers over the transcript).
    render_agents_overlay(frame, state, input_area);
    if let Some(dialog) = &state.dialog {
        render_dialog(frame, dialog, frame.area());
    }
    // The side-question overlay renders over the transcript it is deliberately
    // not part of. Below the approval overlay: a parked run is waiting on that
    // one, and nothing is waiting on this.
    if state.btw.open {
        render_btw_panel(frame, &state.btw, state.spinner_frame, frame.area());
    }
    // Approval overlay renders above everything — a parked run is waiting on it.
    if let Some(approval) = &state.approval {
        render_approval(frame, approval, frame.area());
    }
}
