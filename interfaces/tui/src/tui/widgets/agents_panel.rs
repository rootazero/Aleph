// Pinned agents panel: this session's background sub-agents as live rows
// (pi-style: spinner · task · tool uses · tokens · elapsed), docked above the
// input. Running agents first, finished ones folded into a "+N more" line —
// the `/agents` overlay is the full, selectable view.
//
// Data: `AppState.agents` — a cold `subagent.tree` snapshot merged with live
// `run.subagent_tree` deltas through the shared protocol `apply_event`.

use aleph_protocol::subagent_tree::{NodeLifecycle, SubagentNode};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::app::agent_display_order;
use crate::tui::theme::{DEFAULT_THEME, SPINNER_FRAMES};

/// Cap on agent rows in the dock (the overlay shows everything).
const MAX_AGENT_ROWS: usize = 5;

/// Status glyph for a settled node. Running nodes render the shared spinner.
#[must_use]
pub(crate) fn lifecycle_glyph(lifecycle: NodeLifecycle) -> &'static str {
    match lifecycle {
        NodeLifecycle::Running => "\u{25cf}", // ● (spinner replaces it live)
        NodeLifecycle::Completed => "\u{2713}", // ✓
        NodeLifecycle::Failed => "\u{2717}",  // ✗
        NodeLifecycle::Cancelled => "\u{2298}", // ⊘
        NodeLifecycle::TimedOut => "\u{231b}", // ⌛
    }
}

pub(crate) fn lifecycle_style(lifecycle: NodeLifecycle) -> Style {
    match lifecycle {
        NodeLifecycle::Running => Style::default().fg(DEFAULT_THEME.tool_running),
        NodeLifecycle::Completed => Style::default().fg(DEFAULT_THEME.tool_success),
        NodeLifecycle::Failed | NodeLifecycle::TimedOut => {
            Style::default().fg(DEFAULT_THEME.tool_failed)
        }
        NodeLifecycle::Cancelled => Style::default().fg(DEFAULT_THEME.muted),
    }
}

/// One agent row's caption: task · tools · tokens · elapsed · activity.
#[must_use]
pub(crate) fn agent_row_text(node: &SubagentNode, now_ms: u64) -> String {
    let mut out = node.task.clone();
    if node.tool_count > 0 {
        out.push_str(&format!(
            " \u{00b7} {} tool use{s}",
            node.tool_count,
            s = if node.tool_count == 1 { "" } else { "s" }
        ));
    }
    if let Some(tokens) = node.total_tokens {
        out.push_str(&format!(" \u{00b7} {} tok", fmt_count(tokens)));
    }
    out.push_str(&format!(
        " \u{00b7} {}",
        fmt_elapsed_ms(elapsed_ms(node, now_ms))
    ));
    if node.lifecycle == NodeLifecycle::Running {
        match node.last_activity.as_deref() {
            Some("llm_thinking") => out.push_str(" \u{00b7} thinking\u{2026}"),
            Some("tool_called" | "tool_returned") => {
                if let Some(tool) = node.last_tool.as_deref() {
                    out.push_str(&format!(" \u{00b7} {tool}"));
                }
            }
            _ => {}
        }
    }
    out
}

/// Wall-clock for a node: terminal nodes carry their total; running ones are
/// measured against the server's spawn stamp (both are unix ms — a skewed
/// client clock shows a skewed elapsed, never a panic).
fn elapsed_ms(node: &SubagentNode, now_ms: u64) -> u64 {
    match node.lifecycle {
        NodeLifecycle::Running => now_ms.saturating_sub(node.started_at_ms),
        _ => node.elapsed_ms,
    }
}

/// `12.3k` / `1.2M` count ladder (tokens).
#[must_use]
pub(crate) fn fmt_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1000.0),
        10_000..=999_999 => format!("{}k", n / 1000),
        1_000_000..=9_999_999 => format!("{:.1}M", n as f64 / 1_000_000.0),
        _ => format!("{}M", n / 1_000_000),
    }
}

/// `4.2s` under a minute, `5m42s` beyond.
#[must_use]
pub(crate) fn fmt_elapsed_ms(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (ms / 1000) / 60;
        let s = (ms / 1000) % 60;
        format!("{m}m{s:02}s")
    }
}

/// Rows this panel needs; 0 when the session has no sub-agents.
#[must_use]
pub fn agents_panel_height(agents: &[SubagentNode]) -> u16 {
    if agents.is_empty() {
        return 0;
    }
    let shown = agents.len().min(MAX_AGENT_ROWS);
    let more_line = usize::from(agents.len() > MAX_AGENT_ROWS);
    u16::try_from(1 + shown + more_line).unwrap_or(u16::MAX)
}

/// Render the panel into `area` (sized by [`agents_panel_height`]).
pub fn render_agents_panel(
    frame: &mut Frame,
    agents: &[SubagentNode],
    spinner_frame: usize,
    now_ms: u64,
    area: Rect,
) {
    if area.height == 0 || agents.is_empty() {
        return;
    }
    let width = area.width as usize;
    let muted = Style::default().fg(DEFAULT_THEME.muted);
    let ordered = agent_display_order(agents);
    let running = ordered
        .iter()
        .filter(|n| n.lifecycle == NodeLifecycle::Running)
        .count();
    let finished = ordered.len() - running;

    let mut lines: Vec<Line> = Vec::new();
    let mut header = format!(" \u{25cf} Agents \u{00b7} {} running", running);
    if finished > 0 {
        header.push_str(&format!(", {finished} finished"));
    }
    header.push_str(" \u{00b7} /agents to browse");
    lines.push(Line::from(Span::styled(
        clamp_chars(&header, width),
        Style::default()
            .fg(DEFAULT_THEME.primary)
            .add_modifier(Modifier::BOLD),
    )));

    let spinner = SPINNER_FRAMES
        .get(spinner_frame % SPINNER_FRAMES.len())
        .copied()
        .unwrap_or("\u{25cf}");
    for node in ordered.iter().take(MAX_AGENT_ROWS) {
        let (glyph, style) = if node.lifecycle == NodeLifecycle::Running {
            (spinner, lifecycle_style(NodeLifecycle::Running))
        } else {
            (lifecycle_glyph(node.lifecycle), muted)
        };
        let row = format!("   {glyph} {}", agent_row_text(node, now_ms));
        lines.push(Line::from(Span::styled(clamp_chars(&row, width), style)));
    }
    if ordered.len() > MAX_AGENT_ROWS {
        let hidden = ordered.len() - MAX_AGENT_ROWS;
        let hidden_finished = ordered
            .iter()
            .skip(MAX_AGENT_ROWS)
            .filter(|n| n.lifecycle != NodeLifecycle::Running)
            .count();
        lines.push(Line::from(Span::styled(
            format!("   +{hidden} more ({hidden_finished} finished)"),
            muted,
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Character-safe single-line clamp (CJK-safe; same rule as the tasks panel).
fn clamp_chars(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, lifecycle: NodeLifecycle, started: u64) -> SubagentNode {
        SubagentNode {
            node_id: id.into(),
            parent_id: None,
            depth: 1,
            root_session: "agent:s".into(),
            task: format!("task {id}"),
            model: None,
            lifecycle,
            started_at_ms: started,
            elapsed_ms: 5000,
            tool_count: 3,
            last_tool: Some("grep".into()),
            last_activity: Some("tool_called".into()),
            result_preview: None,
            child_session: None,
            total_tokens: Some(232_500),
        }
    }

    #[test]
    fn empty_panel_takes_no_rows() {
        assert_eq!(agents_panel_height(&[]), 0);
    }

    #[test]
    fn height_is_header_rows_and_optional_more_line() {
        let two: Vec<SubagentNode> = (0..2)
            .map(|i| node(&format!("n{i}"), NodeLifecycle::Running, i))
            .collect();
        assert_eq!(agents_panel_height(&two), 3);
        let seven: Vec<SubagentNode> = (0..7)
            .map(|i| node(&format!("n{i}"), NodeLifecycle::Completed, i))
            .collect();
        assert_eq!(agents_panel_height(&seven), 1 + 5 + 1);
    }

    #[test]
    fn row_text_carries_tools_tokens_elapsed_and_activity() {
        let n = node("a", NodeLifecycle::Running, 0);
        let text = agent_row_text(&n, 304_300);
        assert!(text.contains("3 tool uses"), "{text}");
        assert!(text.contains("232k tok"), "{text}");
        assert!(text.contains("5m04s"), "{text}");
        assert!(text.contains("grep"), "{text}");
    }

    #[test]
    fn terminal_rows_use_the_recorded_duration_not_the_clock() {
        let n = node("a", NodeLifecycle::Completed, 0);
        // A completed node's elapsed must not keep growing with now_ms.
        assert!(agent_row_text(&n, 999_999_999).contains("5.0s"));
    }

    #[test]
    fn count_ladder() {
        assert_eq!(fmt_count(950), "950");
        assert_eq!(fmt_count(2_340), "2.3k");
        assert_eq!(fmt_count(232_500), "232k");
        assert_eq!(fmt_count(1_200_000), "1.2M");
    }
}
