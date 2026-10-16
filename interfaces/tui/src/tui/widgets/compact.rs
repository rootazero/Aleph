//! Cell-aware, style-preserving single-line clipping for compact dock widgets.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Dock rows must remain one physical row even when a server caption contains
/// newlines. Keep the original caption in the protocol state, not a second copy.
pub(super) fn fit_line(mut line: Line<'static>, width: usize) -> Line<'static> {
    for span in &mut line.spans {
        if span.content.chars().any(char::is_control) {
            span.content = span
                .content
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect::<String>()
                .into();
        }
    }
    if line.width() <= width {
        return line;
    }
    if width == 0 {
        line.spans.clear();
        return line;
    }
    let keep = width - 1; // Reserve the ellipsis cell.
    let mut spans = Vec::new();
    let mut used = 0;
    let mut ellipsis_style = line.style;
    for span in line.spans {
        ellipsis_style = span.style;
        let mut head = String::new();
        for c in span.content.chars() {
            // UnicodeWidthChar is not sufficient for variation selectors and
            // ZWJ sequences: the terminal width belongs to the candidate
            // prefix, not to each scalar value independently.
            head.push(c);
            if used + UnicodeWidthStr::width(head.as_str()) > keep {
                head.pop();
                break;
            }
        }
        if head.len() == span.content.len() {
            used += UnicodeWidthStr::width(head.as_str());
            spans.push(span);
        } else {
            if !head.is_empty() {
                spans.push(Span::styled(head, span.style));
            }
            break;
        }
    }
    spans.push(Span::styled("…", ellipsis_style));
    line.spans = spans;
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    fn text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn clipping_counts_terminal_cells_and_preserves_styles() {
        let accent = Style::default().fg(Color::Cyan);
        let muted = Style::default().fg(Color::DarkGray);
        let line = Line::from(vec![
            Span::styled("● ", accent),
            Span::styled("任务甲乙", muted),
        ]);
        let clipped = fit_line(line.clone(), 7);
        assert_eq!(text(&clipped), "● 任务…");
        assert_eq!(clipped.width(), 7);
        assert_eq!(clipped.spans[0].style, accent);
        assert_eq!(clipped.spans[1].style, muted);
        for width in 0..15 {
            assert!(fit_line(line.clone(), width).width() <= width);
        }
    }

    #[test]
    fn zero_width_and_combining_marks_are_safe() {
        let line = Line::from("e\u{301}clair");
        assert_eq!(text(&fit_line(line.clone(), 3)), "e\u{301}c…");
        assert_eq!(text(&fit_line(line.clone(), 0)), "");
        assert_eq!(text(&fit_line(line, 1)), "…");
    }

    #[test]
    fn a_multiline_caption_cannot_consume_another_dock_row() {
        // Match the production dock path: Line::from(str) strips newlines
        // before fit_line can normalize them, while Span preserves the caption.
        let line = fit_line(Line::from(Span::raw("task\nnext\r\tline\u{1b}")), 80);
        assert_eq!(text(&line), "task next  line ");
    }

    #[test]
    fn unicode_sequences_and_span_boundaries_never_overflow_the_target() {
        let line = Line::from(vec![
            Span::raw("♥️xx"),
            Span::styled("👩‍💻yy", Style::default().fg(Color::Magenta)),
        ]);
        for width in 0..=12 {
            let clipped = fit_line(line.clone(), width);
            assert!(
                clipped.width() <= width,
                "width {width}: {:?} has width {}",
                text(&clipped),
                clipped.width()
            );
        }
        let split = Line::from(vec![Span::raw("👩"), Span::raw("‍💻xx")]);
        for width in 0..=10 {
            assert!(fit_line(split.clone(), width).width() <= width);
        }
        let clipped = fit_line(Line::from("♥️xx"), 2);
        assert!(clipped.width() <= 2);
        assert_ne!(text(&clipped), "♥️…");
    }
}
