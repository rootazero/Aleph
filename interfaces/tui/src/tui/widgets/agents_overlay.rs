// `/agents` overlay: a selectable list of this session's sub-agents, and —
// on Enter — one agent's run view (its child transcript, fetched through the
// same `chat.history` RPC every conversation uses; the address travels on
// `SubagentNode.child_session`). Esc backs out one level at a time, pi-style:
// "↑↓ select · enter view · esc back".

use aleph_protocol::subagent_tree::NodeLifecycle;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{agent_display_order, AgentsOverlayState, AppState};
use crate::tui::theme::{DEFAULT_THEME, SPINNER_FRAMES};

use super::agents_panel::{agent_row_text, lifecycle_glyph, lifecycle_style};

/// Maximum visible list rows.
const MAX_VISIBLE_ROWS: u16 = 12;

/// Render the overlay: list mode above the input (picker placement), detail
/// mode as a large centered view over the transcript.
pub fn render_agents_overlay(frame: &mut Frame, state: &AppState, input_area: Rect) {
    let Some(overlay) = &state.agents_overlay else {
        return;
    };
    match &overlay.detail {
        Some(detail) => render_detail(frame, detail, frame.area()),
        None => render_list(frame, state, overlay, input_area),
    }
}

fn render_list(frame: &mut Frame, state: &AppState, overlay: &AgentsOverlayState, area: Rect) {
    let ordered = agent_display_order(&state.agents);
    let row_count = u16::try_from(ordered.len()).unwrap_or(u16::MAX);
    let visible = row_count.clamp(1, MAX_VISIBLE_ROWS);
    // rows + borders + hint line
    let overlay_height = visible.saturating_add(3);
    let overlay_y = area.y.saturating_sub(overlay_height);
    let overlay_width = area.width.min(100);
    let overlay_rect = Rect::new(area.x, overlay_y, overlay_width, overlay_height);

    frame.render_widget(Clear, overlay_rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(" Agents ");
    let inner = block.inner(overlay_rect);
    frame.render_widget(block, overlay_rect);
    if inner.height < 2 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    if ordered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no sub-agents in this session yet)",
            Style::default().fg(DEFAULT_THEME.muted),
        )));
    } else {
        // Clamped centered window over the rows (codex/pi list idiom).
        let max_visible = inner.height.saturating_sub(1) as usize;
        let selected = overlay.selected.min(ordered.len().saturating_sub(1));
        let start = selected
            .saturating_sub(max_visible / 2)
            .min(ordered.len().saturating_sub(max_visible.max(1)));
        let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
        let spinner = SPINNER_FRAMES
            .get(state.spinner_frame % SPINNER_FRAMES.len())
            .copied()
            .unwrap_or("\u{25cf}");
        for (row, node) in ordered
            .iter()
            .enumerate()
            .skip(start)
            .take(max_visible.max(1))
        {
            let is_selected = row == selected;
            let glyph = if node.lifecycle == NodeLifecycle::Running {
                spinner
            } else {
                lifecycle_glyph(node.lifecycle)
            };
            let indicator = if is_selected { "> " } else { "  " };
            let text = format!("{indicator}{glyph} {}", agent_row_text(node, now_ms));
            let style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD)
            } else if node.lifecycle == NodeLifecycle::Running {
                lifecycle_style(NodeLifecycle::Running)
            } else {
                Style::default().fg(DEFAULT_THEME.muted)
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
    }
    lines.push(Line::from(Span::styled(
        " \u{2191}\u{2193} select \u{00b7} enter view \u{00b7} esc back",
        Style::default().fg(DEFAULT_THEME.muted),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_detail(frame: &mut Frame, detail: &crate::tui::app::AgentDetailState, area: Rect) {
    // Large centered view: leave a small margin so the transcript underneath
    // is visibly "behind" it.
    let margin_x = area.width / 10;
    let margin_y = area.height / 10;
    let rect = Rect::new(
        area.x + margin_x,
        area.y + margin_y,
        area.width.saturating_sub(margin_x * 2).max(20),
        area.height.saturating_sub(margin_y * 2).max(6),
    );
    frame.render_widget(Clear, rect);

    let title = format!(" Agent \u{00b7} {} ", clamp_chars(&detail.title, 60));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(title);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 2 {
        return;
    }

    let body_area = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(1),
    );
    let hint_area = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );

    let lines: Vec<Line> = detail
        .lines
        .iter()
        .map(|l| Line::from(detail_line_span(l)))
        .collect();
    let scroll =
        u16::try_from(detail.scroll.min(detail.lines.len().saturating_sub(1))).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        body_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " \u{2191}\u{2193}/PgUp/PgDn scroll \u{00b7} esc back",
            Style::default().fg(DEFAULT_THEME.muted),
        ))),
        hint_area,
    );
}

/// Style a detail line: role separators bright, metadata dim, body default.
fn detail_line_span(line: &str) -> Span<'_> {
    if line.starts_with("\u{2500}\u{2500} ") {
        Span::styled(
            line,
            Style::default()
                .fg(DEFAULT_THEME.heading)
                .add_modifier(Modifier::BOLD),
        )
    } else if line.starts_with("  \u{00b7}") {
        Span::styled(line, Style::default().fg(DEFAULT_THEME.muted))
    } else {
        Span::raw(line)
    }
}

fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}
