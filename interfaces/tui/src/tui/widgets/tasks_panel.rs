// Pinned tasks panel: the conversation's execution list (scratchpad plan)
// rendered pi-style as a compact checklist docked between the chat area and
// the input. Lives in the dock rather than the transcript so progress is
// glanceable mid-run without scrolling (codex pins only a counter in the
// terminal title; pi's tasks widget is the shape this follows).
//
// Data: `AppState.plan` — fed by `chat.history.plan` (cold), the scratchpad
// tool's result snapshot (live), and `RunSummary.plan` (authoritative settle).

use aleph_protocol::plan::{PlanItemStatus, PlanSnapshot};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::theme::DEFAULT_THEME;

/// Cap on checklist rows (excluding the header and elision lines). The plan
/// itself is bounded server-side at 40 items (`PROMPT_PLAN_LIMITS`), but a
/// dock panel competes with the transcript for rows.
const MAX_TASK_ROWS: usize = 8;

/// Which item rows to show, honestly, when the list exceeds the budget.
///
/// Completed steps are elided FROM THE TOP first (they are the least
/// actionable), then the tail is truncated. Both elisions name the count they
/// hid — a bare cut reads as "that's the whole list".
#[derive(Debug, PartialEq, Eq)]
struct RowWindow {
    /// Completed rows hidden from the top.
    elided_done: usize,
    /// Index range of items actually rendered.
    start: usize,
    end: usize,
    /// Rows hidden after `end`.
    elided_tail: usize,
}

fn row_window(plan: &PlanSnapshot) -> RowWindow {
    let total = plan.items.len();
    if total <= MAX_TASK_ROWS {
        return RowWindow {
            elided_done: 0,
            start: 0,
            end: total,
            elided_tail: 0,
        };
    }
    // Drop leading completed items until the remainder fits (or none lead).
    let leading_done = plan
        .items
        .iter()
        .take_while(|i| i.status == PlanItemStatus::Completed)
        .count();
    let over = total - MAX_TASK_ROWS;
    let elided_done = leading_done.min(over);
    let start = elided_done;
    let end = (start + MAX_TASK_ROWS).min(total);
    RowWindow {
        elided_done,
        start,
        end,
        elided_tail: total - end,
    }
}

/// Rows this panel needs, 0 when there is nothing to show.
#[must_use]
pub fn tasks_panel_height(plan: Option<&PlanSnapshot>, visible: bool) -> u16 {
    let Some(plan) = plan.filter(|p| visible && p.has_content()) else {
        return 0;
    };
    let window = row_window(plan);
    let mut rows = 1 + (window.end - window.start); // header + items
    if window.elided_done > 0 {
        rows += 1;
    }
    if window.elided_tail > 0 {
        rows += 1;
    }
    u16::try_from(rows).unwrap_or(u16::MAX)
}

/// Render the panel into `area` (already sized by [`tasks_panel_height`]).
pub fn render_tasks_panel(frame: &mut Frame, plan: &PlanSnapshot, area: Rect) {
    if area.height == 0 {
        return;
    }
    let width = area.width as usize;
    let muted = Style::default().fg(DEFAULT_THEME.muted);
    let mut lines: Vec<Line> = Vec::new();

    // Header: "● 6 tasks (1 done, 1 in progress, 4 open) · objective"
    let total = plan.total();
    let done = plan.done_count();
    let in_progress = plan
        .items
        .iter()
        .filter(|i| i.status == PlanItemStatus::InProgress)
        .count();
    let open = total - done - in_progress;
    let mut header = format!(
        " \u{25cf} {total} task{s}",
        s = if total == 1 { "" } else { "s" }
    );
    if total > 0 {
        header.push_str(&format!(
            " ({done} done, {in_progress} in progress, {open} open)"
        ));
    }
    if let Some(objective) = plan.objective.as_deref() {
        header.push_str(" \u{00b7} ");
        header.push_str(objective);
    }
    lines.push(Line::from(Span::styled(
        clamp_chars(&header, width),
        Style::default()
            .fg(DEFAULT_THEME.primary)
            .add_modifier(Modifier::BOLD),
    )));

    let window = row_window(plan);
    if window.elided_done > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "   \u{2026} {} earlier step(s), all done",
                window.elided_done
            ),
            muted,
        )));
    }
    for (idx, item) in plan
        .items
        .iter()
        .enumerate()
        .take(window.end)
        .skip(window.start)
    {
        let n = idx + 1;
        let (glyph, style) = match item.status {
            PlanItemStatus::Completed => (
                "\u{2713}", // ✓
                muted.add_modifier(Modifier::CROSSED_OUT),
            ),
            PlanItemStatus::InProgress => (
                "\u{273b}", // ❋
                Style::default()
                    .fg(DEFAULT_THEME.tool_name)
                    .add_modifier(Modifier::BOLD),
            ),
            PlanItemStatus::Pending => ("\u{25a1}", muted), // □
        };
        let row = format!("   {glyph} #{n} {}", item.text);
        lines.push(Line::from(Span::styled(clamp_chars(&row, width), style)));
    }
    if window.elided_tail > 0 {
        lines.push(Line::from(Span::styled(
            format!("   \u{2026} {} more", window.elided_tail),
            muted,
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Character-safe single-line clamp with an ellipsis (`&s[..n]` panics inside
/// a multi-byte char — plan text is routinely CJK).
fn clamp_chars(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::plan::PlanItem;

    fn plan(statuses: &[PlanItemStatus]) -> PlanSnapshot {
        PlanSnapshot {
            objective: Some("ship it".into()),
            items: statuses
                .iter()
                .enumerate()
                .map(|(i, s)| PlanItem {
                    text: format!("step {i}"),
                    status: *s,
                })
                .collect(),
            complete: false,
        }
    }

    #[test]
    fn hidden_or_empty_panel_takes_no_rows() {
        assert_eq!(tasks_panel_height(None, true), 0);
        let empty = PlanSnapshot::default();
        assert_eq!(tasks_panel_height(Some(&empty), true), 0);
        let p = plan(&[PlanItemStatus::Pending]);
        assert_eq!(tasks_panel_height(Some(&p), false), 0, "/todo off");
    }

    #[test]
    fn small_plan_is_header_plus_items() {
        let p = plan(&[PlanItemStatus::Completed, PlanItemStatus::Pending]);
        assert_eq!(tasks_panel_height(Some(&p), true), 3);
    }

    #[test]
    fn oversized_plan_elides_leading_done_first_and_names_counts() {
        use PlanItemStatus::{Completed, InProgress, Pending};
        let mut statuses = vec![Completed; 6];
        statuses.push(InProgress);
        statuses.extend(vec![Pending; 5]); // 12 items total
        let p = plan(&statuses);
        let w = row_window(&p);
        // 12 items, cap 8 → 4 over; 6 leading done → elide 4 of them.
        assert_eq!(w.elided_done, 4);
        assert_eq!(w.start, 4);
        assert_eq!(w.end, 12);
        assert_eq!(w.elided_tail, 0);
        // header + elision line + 8 rows
        assert_eq!(tasks_panel_height(Some(&p), true), 10);
    }

    #[test]
    fn oversized_plan_with_no_leading_done_truncates_the_tail() {
        let p = plan(&vec![PlanItemStatus::Pending; 12]);
        let w = row_window(&p);
        assert_eq!(w.elided_done, 0);
        assert_eq!((w.start, w.end), (0, 8));
        assert_eq!(w.elided_tail, 4);
    }

    #[test]
    fn clamp_is_char_safe_on_cjk() {
        let clamped = clamp_chars("任务甲乙丙丁", 4);
        assert_eq!(clamped.chars().count(), 4);
        assert!(clamped.ends_with('\u{2026}'));
    }
}
