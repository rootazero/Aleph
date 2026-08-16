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

use crate::tui::app::SessionKnobs;
use crate::tui::slash::{SessionKnob, ToolProgressMode};
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
    /// Last-call prompt-cache hit rate as a rounded percentage (0–100), or
    /// `None` when no call has reported cache activity. Rendered as a
    /// `cache N%` segment — a sudden drop is the live signal that a prefix
    /// bust just happened.
    pub cache_stat: Option<u64>,
    /// Agent id behind `cache_stat` when it is not the session root's, so a
    /// delegated sub-agent's cold start is labelled instead of being read as
    /// the root agent's prefix breaking.
    pub cache_stat_agent: Option<&'a str>,
    pub is_connected: bool,
    pub tool_progress_mode: ToolProgressMode,
    /// Advances the working-indicator spinner (shared 50ms tick counter).
    pub spinner_frame: usize,
    /// Elapsed time of the active run, or `None` when idle. When set, the
    /// trailing help hint is replaced by a live working indicator.
    pub run_elapsed: Option<Duration>,
    /// This conversation's persisted knobs, as the server last reported them.
    ///
    /// Each is `None` when the session carries no override — rendered as
    /// *nothing*, not as a guessed value: the TUI does not read the server's
    /// config, so printing "auto" for an unset tier would be this client
    /// inventing a fact. An unset knob is invisible; a set one is named.
    pub knobs: SessionKnobs<'a>,
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

        // Last-call prompt-cache hit rate (e.g. `cache 87%`), shown only once
        // a provider call has reported cache activity. Dimmed to a warning
        // tint under 50% — a low last-call rate right after a healthy streak
        // is the live symptom of a stable-prefix bust.
        if let Some(pct) = self.cache_stat {
            let label = match self.cache_stat_agent {
                Some(agent) => format!(" cache {pct}% ·{agent} "),
                None => format!(" cache {pct}% "),
            };
            spans.push(sep());
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(cache_stat_color(pct))
                    .bg(DEFAULT_THEME.status_bg),
            ));
        }

        // The conversation's own settings — the reason reopening a terminal
        // mid-task now lands you back where you were rather than on the install
        // defaults. Enumerated from `SessionKnob::ALL` so a knob added to the
        // parser cannot quietly fail to appear here.
        for knob in SessionKnob::ALL {
            let Some(value) = knob_value(&self.knobs, knob) else {
                continue;
            };
            spans.push(sep());
            spans.push(Span::styled(
                format!(" {}:{value} ", knob.command()),
                text_style,
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

/// One knob's value, or `None` when the session follows the global default.
///
/// The exhaustive `match` is the point: it is what turns "a knob was added to
/// the command parser but never shown" into a compile error.
const fn knob_value<'a>(knobs: &SessionKnobs<'a>, knob: SessionKnob) -> Option<&'a str> {
    match knob {
        SessionKnob::ExecTier => knobs.exec_tier,
        SessionKnob::Mode => knobs.mode,
        SessionKnob::Think => knobs.think_level,
        SessionKnob::Memory => knobs.memory_mode,
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

/// Tint the cache stat: normal at or above 50%, warning below — cold starts
/// are expected (first call is always a write), so no red/error tier.
fn cache_stat_color(pct: u64) -> Color {
    if pct >= 50 {
        DEFAULT_THEME.status_fg
    } else {
        DEFAULT_THEME.warning
    }
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

    #[test]
    fn cache_stat_color_bands() {
        // >= 50% normal, below warning — no error tier (cold starts are
        // expected, not alarming).
        assert_eq!(cache_stat_color(87), DEFAULT_THEME.status_fg);
        assert_eq!(cache_stat_color(50), DEFAULT_THEME.status_fg);
        assert_eq!(cache_stat_color(12), DEFAULT_THEME.warning);
    }
}
