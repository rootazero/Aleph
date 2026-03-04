// Tool block widget: renders a single tool execution as a bordered block
// with status indicator (spinner/checkmark/cross), tool name, params, and
// optional error messages.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::tui::app::{ToolExecution, ToolStatus};
use crate::tui::theme::DEFAULT_THEME;

/// Braille spinner frames for running tools.
const SPINNER_FRAMES: &[&str] = &[
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
    "\u{2827}", "\u{2807}", "\u{280f}",
];

/// Render a tool execution block as a set of lines.
///
/// Three visual states:
/// - Running: spinner on right, yellow border
/// - Success: checkmark + duration on right, green border
/// - Failed:  cross + duration on right, red border, error line
pub fn render_tool_block(
    tool: &ToolExecution,
    spinner_frame: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let w = width as usize;
    if w < 8 {
        // Too narrow to render anything meaningful
        return vec![];
    }

    let inner_width = w.saturating_sub(2); // account for border chars

    let (status_char, border_color) = match tool.status {
        ToolStatus::Running => {
            let frame = spinner_frame % SPINNER_FRAMES.len();
            (
                format!("\u{27f3} {}", SPINNER_FRAMES[frame]), // ⟳ + spinner
                DEFAULT_THEME.tool_running,
            )
        }
        ToolStatus::Success => {
            let dur = format_duration(&tool.duration);
            (format!("{} \u{2713}", dur), DEFAULT_THEME.tool_success) // ✓
        }
        ToolStatus::Failed => {
            let dur = format_duration(&tool.duration);
            (format!("{} \u{2717}", dur), DEFAULT_THEME.tool_failed) // ✗
        }
    };

    let border_style = Style::default().fg(border_color);
    let name_style = Style::default().fg(DEFAULT_THEME.tool_name);
    let param_style = Style::default().fg(DEFAULT_THEME.tool_param);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Top border: ┌ tool_name ─────────── status ┐
    let name_part = format!(" {} ", tool.name);
    let status_part = format!(" {} ", status_char);
    let name_len = name_part.len();
    let status_len = status_part.len();
    let dash_count = inner_width.saturating_sub(name_len + status_len);
    let dashes = "\u{2500}".repeat(dash_count);

    lines.push(Line::from(vec![
        Span::styled("\u{250c}".to_string(), border_style),
        Span::styled(name_part, name_style),
        Span::styled(dashes, border_style),
        Span::styled(status_part, border_style),
        Span::styled("\u{2510}".to_string(), border_style),
    ]));

    // Content line: │ params │
    let params_display = truncate_to_width(&tool.params, inner_width.saturating_sub(2));
    let pad = inner_width.saturating_sub(params_display.len() + 1);
    lines.push(Line::from(vec![
        Span::styled("\u{2502} ".to_string(), border_style),
        Span::styled(params_display, param_style),
        Span::styled(" ".repeat(pad), param_style),
        Span::styled("\u{2502}".to_string(), border_style),
    ]));

    // Error line (only if failed and error exists)
    if tool.status == ToolStatus::Failed {
        if let Some(ref err) = tool.error {
            let error_prefix = "Error: ";
            let max_err_len = inner_width.saturating_sub(error_prefix.len() + 2);
            let error_display = truncate_to_width(err, max_err_len);
            let error_full = format!("{}{}", error_prefix, error_display);
            let err_pad = inner_width.saturating_sub(error_full.len() + 1);
            lines.push(Line::from(vec![
                Span::styled("\u{2502} ".to_string(), border_style),
                Span::styled(error_full, Style::default().fg(DEFAULT_THEME.error)),
                Span::styled(" ".repeat(err_pad), param_style),
                Span::styled("\u{2502}".to_string(), border_style),
            ]));
        }
    }

    // Bottom border: └───────────────────────────────┘
    let bottom_dashes = "\u{2500}".repeat(inner_width);
    lines.push(Line::from(vec![
        Span::styled("\u{2514}".to_string(), border_style),
        Span::styled(bottom_dashes, border_style),
        Span::styled("\u{2518}".to_string(), border_style),
    ]));

    lines
}

/// Format an optional duration as "N.Ns".
fn format_duration(duration: &Option<std::time::Duration>) -> String {
    match duration {
        Some(d) => format!("{:.1}s", d.as_secs_f64()),
        None => String::new(),
    }
}

/// Truncate a string to fit within a given character width.
/// Uses char_indices for UTF-8 safety.
fn truncate_to_width(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .nth(max.saturating_sub(3))
        .map_or(s.len(), |(i, _)| i);
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_tool(status: ToolStatus, error: Option<String>) -> ToolExecution {
        ToolExecution {
            id: "t1".to_string(),
            name: "web_search".to_string(),
            params: "Rust generics tutorial".to_string(),
            status,
            duration: Some(Duration::from_millis(1200)),
            progress: None,
            error,
        }
    }

    #[test]
    fn running_tool_renders_spinner() {
        let tool = ToolExecution {
            status: ToolStatus::Running,
            duration: None,
            ..make_tool(ToolStatus::Running, None)
        };
        let lines = render_tool_block(&tool, 0, 40);
        assert!(lines.len() >= 3, "Should have top, content, bottom");
    }

    #[test]
    fn success_tool_renders_checkmark() {
        let tool = make_tool(ToolStatus::Success, None);
        let lines = render_tool_block(&tool, 0, 40);
        assert!(lines.len() >= 3);
        let top: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(top.contains("\u{2713}"), "Should contain checkmark");
    }

    #[test]
    fn failed_tool_renders_error() {
        let tool = make_tool(ToolStatus::Failed, Some("file not found".into()));
        let lines = render_tool_block(&tool, 0, 40);
        // Should have top + params + error + bottom = 4 lines
        assert!(lines.len() >= 4, "Failed tool should have error line");
    }

    #[test]
    fn too_narrow_renders_nothing() {
        let tool = make_tool(ToolStatus::Success, None);
        let lines = render_tool_block(&tool, 0, 5);
        assert!(lines.is_empty());
    }
}
