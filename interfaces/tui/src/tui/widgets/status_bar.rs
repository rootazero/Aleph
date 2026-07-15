// Status bar widget: a single-line bar at the bottom of the screen showing
// connection status, model, session, token count, and a help hint.

use std::time::Duration;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::slash::ToolProgressMode;
use crate::tui::theme::DEFAULT_THEME;

/// Braille spinner frames for the working indicator (matches the tool-block set).
const RUN_SPINNER: &[&str] = &[
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

pub struct StatusBar<'a> {
    pub model: &'a str,
    pub session: &'a str,
    pub tokens: u64,
    pub is_connected: bool,
    pub tool_progress_mode: ToolProgressMode,
    /// Advances the working-indicator spinner (shared 50ms tick counter).
    pub spinner_frame: usize,
    /// Elapsed time of the active run, or `None` when idle. When set, the
    /// trailing help hint is replaced by a live working indicator.
    pub run_elapsed: Option<Duration>,
}

impl StatusBar<'_> {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let (dot, dot_color) = if self.is_connected {
            ("\u{25cf}", DEFAULT_THEME.connected) // ●
        } else {
            ("\u{25cb}", DEFAULT_THEME.disconnected) // ○
        };

        let sep_style = Style::default().fg(DEFAULT_THEME.muted);
        let text_style = Style::default()
            .fg(DEFAULT_THEME.status_fg)
            .bg(DEFAULT_THEME.status_bg);
        let dot_style = Style::default().fg(dot_color).bg(DEFAULT_THEME.status_bg);

        let token_str = format_tokens(self.tokens);

        // While a run is active, replace the static help hint with a live
        // working indicator (spinner + elapsed + interrupt affordance) so the
        // TUI shows progress and surfaces the otherwise-undiscoverable Ctrl+C
        // cancel. Falls back to the help hint when idle.
        let trailing = match self.run_elapsed {
            Some(elapsed) => {
                let frame = self.spinner_frame % RUN_SPINNER.len();
                let spinner = RUN_SPINNER.get(frame).copied().unwrap_or("");
                Span::styled(
                    format!(
                        " {spinner} Working {}s \u{00b7} Ctrl+C to interrupt ",
                        elapsed.as_secs()
                    ),
                    Style::default()
                        .fg(DEFAULT_THEME.tool_running)
                        .bg(DEFAULT_THEME.status_bg),
                )
            }
            None => Span::styled(" /help for commands ", text_style),
        };

        let line = Line::from(vec![
            Span::styled(" ", text_style),
            Span::styled(dot.to_string(), dot_style),
            Span::styled(format!(" {} ", self.model), text_style),
            Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)), // │
            Span::styled(format!(" {} ", self.session), text_style),
            Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)),
            Span::styled(format!(" {token_str} "), text_style),
            Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)),
            Span::styled(
                format!(" T:{} ", self.tool_progress_mode.glyph()),
                text_style,
            ),
            Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)),
            trailing,
        ]);

        let paragraph = Paragraph::new(line).style(Style::default().bg(DEFAULT_THEME.status_bg));
        frame.render_widget(paragraph, area);
    }
}

/// Format a token count as a human-readable string.
/// 0-999 -> "N tok", 1000-999999 -> "N.Nk tok", 1000000+ -> "N.NM tok"
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        format!("{millions:.1}M tok")
    } else if tokens >= 1_000 {
        let thousands = tokens as f64 / 1_000.0;
        format!("{thousands:.1}k tok")
    } else {
        format!("{tokens} tok")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(0), "0 tok");
        assert_eq!(format_tokens(42), "42 tok");
        assert_eq!(format_tokens(999), "999 tok");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1000), "1.0k tok");
        assert_eq!(format_tokens(1234), "1.2k tok");
        assert_eq!(format_tokens(3200), "3.2k tok");
        assert_eq!(format_tokens(999_999), "1000.0k tok");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M tok");
        assert_eq!(format_tokens(1_234_567), "1.2M tok");
        assert_eq!(format_tokens(42_500_000), "42.5M tok");
    }
}
