// Status bar widget: a single-line bar at the bottom of the screen showing
// connection status, model, session, token count, and a help hint.

use std::time::Duration;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
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
    /// Live context-window occupancy `(used, window)` from the latest gauge
    /// event, or `None` when unknown. Rendered as a `ctx used/window` segment
    /// tinted by fill ratio.
    pub context_gauge: Option<(u32, u32)>,
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

        let sep = || Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)); // │
        let mut spans = vec![
            Span::styled(" ", text_style),
            Span::styled(dot.to_string(), dot_style),
            Span::styled(format!(" {} ", self.model), text_style),
            sep(),
            Span::styled(format!(" {} ", self.session), text_style),
            sep(),
            Span::styled(format!(" {token_str} "), text_style),
        ];

        // Context-window gauge (e.g. `ctx 12.3k/200.0k`), tinted by fill ratio
        // so a run approaching the window edge reads at a glance. Only shown
        // once a `ContextGauge` event has supplied a real denominator.
        if let Some((used, window)) = self.context_gauge.filter(|&(_, w)| w > 0) {
            spans.push(sep());
            spans.push(Span::styled(
                format!(" ctx {} ", format_context_gauge(used, window)),
                Style::default()
                    .fg(context_gauge_color(used, window))
                    .bg(DEFAULT_THEME.status_bg),
            ));
        }

        spans.push(sep());
        spans.push(Span::styled(
            format!(" T:{} ", self.tool_progress_mode.glyph()),
            text_style,
        ));
        spans.push(sep());
        spans.push(trailing);
        let line = Line::from(spans);

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

/// Compact token count without the ` tok` suffix, for the context-gauge
/// numerator/denominator (e.g. `12.3k`, `200.0k`, `1.0M`, `847`).
fn compact_tokens(n: u32) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", f64::from(n) / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", f64::from(n) / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Format context-window occupancy as `used/window`, e.g. `12.3k/200.0k`.
fn format_context_gauge(used: u32, window: u32) -> String {
    format!("{}/{}", compact_tokens(used), compact_tokens(window))
}

/// Tint the gauge by fill ratio: normal under 70%, amber 70–90%, red at or
/// above 90% so an imminent context overflow is legible before it truncates.
fn context_gauge_color(used: u32, window: u32) -> Color {
    let ratio = f64::from(used) / f64::from(window);
    if ratio >= 0.9 {
        DEFAULT_THEME.error
    } else if ratio >= 0.7 {
        DEFAULT_THEME.warning
    } else {
        DEFAULT_THEME.status_fg
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

    #[test]
    fn context_gauge_formats_used_over_window() {
        assert_eq!(format_context_gauge(12_345, 200_000), "12.3k/200.0k");
        assert_eq!(format_context_gauge(847, 8_000), "847/8.0k");
        assert_eq!(format_context_gauge(1_500_000, 1_000_000), "1.5M/1.0M");
    }

    #[test]
    fn context_gauge_color_bands() {
        // < 70% normal, 70–90% warning, >= 90% error.
        assert_eq!(context_gauge_color(10, 100), DEFAULT_THEME.status_fg);
        assert_eq!(context_gauge_color(75, 100), DEFAULT_THEME.warning);
        assert_eq!(context_gauge_color(95, 100), DEFAULT_THEME.error);
    }
}
