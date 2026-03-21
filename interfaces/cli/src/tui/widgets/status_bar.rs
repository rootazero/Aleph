// Status bar widget: a single-line bar at the bottom of the screen showing
// connection status, model, session, token count, and a help hint.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::theme::DEFAULT_THEME;

pub struct StatusBar<'a> {
    pub model: &'a str,
    pub session: &'a str,
    pub tokens: u64,
    pub is_connected: bool,
}

impl<'a> StatusBar<'a> {
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

        let line = Line::from(vec![
            Span::styled(" ", text_style),
            Span::styled(dot.to_string(), dot_style),
            Span::styled(format!(" {} ", self.model), text_style),
            Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)), // │
            Span::styled(format!(" {} ", self.session), text_style),
            Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)),
            Span::styled(format!(" {} ", token_str), text_style),
            Span::styled("\u{2502}", sep_style.bg(DEFAULT_THEME.status_bg)),
            Span::styled(" /help for commands ", text_style),
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
        format!("{:.1}M tok", millions)
    } else if tokens >= 1_000 {
        let thousands = tokens as f64 / 1_000.0;
        format!("{:.1}k tok", thousands)
    } else {
        format!("{} tok", tokens)
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
        assert_eq!(format_tokens(999999), "1000.0k tok");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M tok");
        assert_eq!(format_tokens(1_234_567), "1.2M tok");
        assert_eq!(format_tokens(42_500_000), "42.5M tok");
    }
}
