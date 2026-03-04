use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::theme::DEFAULT_THEME;

/// Convert markdown text to styled ratatui Lines for terminal display.
///
/// Supports a subset of markdown: bold, italic, inline code, fenced code blocks,
/// headings (h1-h3), bulleted lists, blockquotes, and links.
pub fn markdown_to_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    let mut result: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        if in_code_block {
            if line.trim_start().starts_with("```") {
                // Close code block
                in_code_block = false;
                render_code_block(&code_lang, &code_lines, width, &mut result);
                code_lang.clear();
                code_lines.clear();
            } else {
                code_lines.push(line.to_string());
            }
            continue;
        }

        // Check for code block opening
        if line.trim_start().starts_with("```") {
            in_code_block = true;
            let trimmed = line.trim_start().trim_start_matches('`');
            code_lang = trimmed.trim().to_string();
            continue;
        }

        // Empty line
        if line.trim().is_empty() {
            result.push(Line::default());
            continue;
        }

        // Heading
        if line.starts_with('#') {
            if let Some(heading_line) = parse_heading(line) {
                result.push(heading_line);
                continue;
            }
        }

        // Blockquote
        if line.starts_with('>') {
            let content = line.trim_start_matches('>').trim_start();
            let mut spans = vec![Span::styled(
                "\u{250a} ".to_string(),
                Style::default().fg(DEFAULT_THEME.quote),
            )];
            let inline = parse_inline(
                content,
                Style::default().fg(DEFAULT_THEME.quote),
            );
            spans.extend(inline);
            let wrapped = wrap_line_spans(&spans, width);
            result.extend(wrapped);
            continue;
        }

        // List item
        if is_list_item(line) {
            let content = strip_list_marker(line);
            let mut spans = vec![Span::styled(
                "  \u{2022} ".to_string(),
                Style::default().fg(DEFAULT_THEME.primary),
            )];
            let inline = parse_inline(&content, Style::default());
            spans.extend(inline);
            let wrapped = wrap_line_spans(&spans, width);
            result.extend(wrapped);
            continue;
        }

        // Normal paragraph line
        let spans = parse_inline(line, Style::default());
        let wrapped = wrap_line_spans(&spans, width);
        result.extend(wrapped);
    }

    // Handle unterminated code block — render what we have
    if in_code_block {
        render_code_block(&code_lang, &code_lines, width, &mut result);
    }

    result
}

/// Check if a line is a list item (starts with `- ` or `* `)
fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ") || trimmed.starts_with("* ")
}

/// Strip the list marker from a line, returning the content after `- ` or `* `
fn strip_list_marker(line: &str) -> String {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        // Use char_indices for UTF-8 safety
        let mut chars = trimmed.char_indices();
        chars.next(); // skip marker char
        chars.next(); // skip space
        if let Some((idx, _)) = chars.next() {
            trimmed.get(idx..).unwrap_or("").to_string()
        } else {
            // marker + space only, no content after
            trimmed.get(2..).unwrap_or("").to_string()
        }
    } else {
        trimmed.to_string()
    }
}

/// Parse heading lines. Returns None if the line isn't actually a heading.
fn parse_heading(line: &str) -> Option<Line<'static>> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 3 {
        return None;
    }

    // Must have a space after the hashes
    let after_hashes = trimmed.get(level..)?;
    if !after_hashes.starts_with(' ') {
        return None;
    }
    let text = after_hashes.trim_start().to_string();

    let style = match level {
        1 => Style::default()
            .fg(DEFAULT_THEME.heading)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default()
            .fg(DEFAULT_THEME.heading)
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(DEFAULT_THEME.heading)
            .add_modifier(Modifier::BOLD),
    };

    Some(Line::from(Span::styled(text, style)))
}

/// Parse inline markdown formatting, returning styled spans.
///
/// Handles: **bold**, *italic*, `inline code`, [link text](url)
fn parse_inline(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let len = chars.len();
    let mut i = 0;
    let mut plain_start = 0;

    while i < len {
        let (byte_idx, ch) = chars[i];

        match ch {
            '*' => {
                // Check for bold (**) or italic (*)
                if i + 1 < len && chars[i + 1].1 == '*' {
                    // Bold: **text**
                    if let Some(end) = find_double_marker(&chars, i + 2, '*') {
                        // Flush plain text before this marker
                        flush_plain(text, plain_start, byte_idx, base_style, &mut spans);
                        let inner_start = chars[i + 2].0;
                        let inner_end = chars[end].0;
                        let inner = text.get(inner_start..inner_end).unwrap_or("");
                        spans.push(Span::styled(
                            inner.to_string(),
                            base_style.add_modifier(Modifier::BOLD),
                        ));
                        i = end + 2; // skip past closing **
                        plain_start = if i < len { chars[i].0 } else { text.len() };
                        continue;
                    }
                }
                // Single italic: *text*
                if let Some(end) = find_single_marker(&chars, i + 1, '*') {
                    flush_plain(text, plain_start, byte_idx, base_style, &mut spans);
                    let inner_start = chars[i + 1].0;
                    let inner_end = chars[end].0;
                    let inner = text.get(inner_start..inner_end).unwrap_or("");
                    spans.push(Span::styled(
                        inner.to_string(),
                        base_style.add_modifier(Modifier::ITALIC),
                    ));
                    i = end + 1;
                    plain_start = if i < len { chars[i].0 } else { text.len() };
                    continue;
                }
                i += 1;
            }
            '`' => {
                // Inline code: `text`
                if let Some(end) = find_single_marker(&chars, i + 1, '`') {
                    flush_plain(text, plain_start, byte_idx, base_style, &mut spans);
                    let inner_start = chars[i + 1].0;
                    let inner_end = chars[end].0;
                    let inner = text.get(inner_start..inner_end).unwrap_or("");
                    spans.push(Span::styled(
                        inner.to_string(),
                        Style::default().bg(DEFAULT_THEME.code_bg),
                    ));
                    i = end + 1;
                    plain_start = if i < len { chars[i].0 } else { text.len() };
                    continue;
                }
                i += 1;
            }
            '[' => {
                // Link: [text](url)
                if let Some((link_text, after_link_idx)) = parse_link(&chars, text, i) {
                    flush_plain(text, plain_start, byte_idx, base_style, &mut spans);
                    spans.push(Span::styled(
                        link_text,
                        Style::default()
                            .fg(DEFAULT_THEME.link)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                    i = after_link_idx;
                    plain_start = if i < len { chars[i].0 } else { text.len() };
                    continue;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    // Flush remaining plain text
    if plain_start < text.len() {
        let remaining = text.get(plain_start..).unwrap_or("");
        if !remaining.is_empty() {
            spans.push(Span::styled(remaining.to_string(), base_style));
        }
    }

    spans
}

/// Flush accumulated plain text (from plain_start to current byte index) as a styled span.
fn flush_plain(
    text: &str,
    start: usize,
    end: usize,
    style: Style,
    spans: &mut Vec<Span<'static>>,
) {
    if start < end {
        if let Some(s) = text.get(start..end) {
            if !s.is_empty() {
                spans.push(Span::styled(s.to_string(), style));
            }
        }
    }
}

/// Find a single closing marker character, returning the char index (not byte index).
fn find_single_marker(chars: &[(usize, char)], from: usize, marker: char) -> Option<usize> {
    for idx in from..chars.len() {
        if chars[idx].1 == marker {
            return Some(idx);
        }
    }
    None
}

/// Find a double closing marker (e.g., **), returning the char index of the first char.
fn find_double_marker(chars: &[(usize, char)], from: usize, marker: char) -> Option<usize> {
    let len = chars.len();
    for idx in from..len.saturating_sub(1) {
        if chars[idx].1 == marker && chars[idx + 1].1 == marker {
            return Some(idx);
        }
    }
    None
}

/// Parse a markdown link: [text](url). Returns (link_text, char_index_after_closing_paren).
fn parse_link(
    chars: &[(usize, char)],
    text: &str,
    start: usize,
) -> Option<(String, usize)> {
    // start is at '['
    // Find closing ']'
    let mut i = start + 1;
    while i < chars.len() && chars[i].1 != ']' {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let bracket_close = i;

    // Next char must be '('
    i += 1;
    if i >= chars.len() || chars[i].1 != '(' {
        return None;
    }

    // Find closing ')'
    i += 1;
    while i < chars.len() && chars[i].1 != ')' {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }

    // Extract link text
    let text_start = chars[start + 1].0;
    let text_end = chars[bracket_close].0;
    let link_text = text.get(text_start..text_end).unwrap_or("").to_string();

    Some((link_text, i + 1))
}

/// Render a fenced code block with borders and language label.
fn render_code_block(
    lang: &str,
    lines: &[String],
    width: usize,
    result: &mut Vec<Line<'static>>,
) {
    let border_style = Style::default().fg(DEFAULT_THEME.code_block_border);
    let code_style = Style::default().bg(DEFAULT_THEME.code_bg);
    let inner_width = if width > 4 { width - 2 } else { width };

    // Top border: ┌─ lang ──────
    let label = if lang.is_empty() {
        String::new()
    } else {
        format!(" {} ", lang)
    };
    let label_width = UnicodeWidthStr::width(label.as_str());
    let dash_count = inner_width.saturating_sub(label_width + 1);
    let top = format!(
        "\u{250c}\u{2500}{}{}",
        label,
        "\u{2500}".repeat(dash_count)
    );
    result.push(Line::from(Span::styled(top, border_style)));

    // Code lines
    for code_line in lines {
        let display = format!("\u{2502} {}", code_line);
        result.push(Line::from(Span::styled(display, code_style)));
    }

    // Bottom border: └──────────────
    let bottom = format!(
        "\u{2514}{}",
        "\u{2500}".repeat(inner_width)
    );
    result.push(Line::from(Span::styled(bottom, border_style)));
}

/// Wrap a line of spans if total visual width exceeds the given width.
///
/// Simple v1: flattens spans to plain text, wraps, and returns new lines.
/// Inline formatting is lost on wrapped continuation lines (acceptable for v1).
fn wrap_line_spans(spans: &[Span<'static>], width: usize) -> Vec<Line<'static>> {
    if width == 0 || spans.is_empty() {
        return vec![Line::from(spans.to_vec())];
    }

    // Calculate total visual width
    let total_width: usize = spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();

    if total_width <= width {
        return vec![Line::from(spans.to_vec())];
    }

    // Flatten to plain text for wrapping
    let plain: String = spans.iter().map(|s| s.content.as_ref()).collect();

    // Use textwrap to wrap the text
    let wrapped = textwrap::wrap(&plain, width);

    wrapped
        .into_iter()
        .map(|cow| Line::from(Span::raw(cow.into_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    /// Helper to extract plain text from a Line
    fn line_to_plain_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Helper to check if any span in a line has a given modifier
    fn has_modifier(line: &Line, modifier: Modifier) -> bool {
        line.spans
            .iter()
            .any(|s| s.style.add_modifier.contains(modifier))
    }

    /// Helper to check if any span in a line has a given bg color
    fn has_bg_color(line: &Line, color: Color) -> bool {
        line.spans.iter().any(|s| s.style.bg == Some(color))
    }

    /// Helper to check if any span in a line has a given fg color
    fn has_fg_color(line: &Line, color: Color) -> bool {
        line.spans.iter().any(|s| s.style.fg == Some(color))
    }

    #[test]
    fn plain_text() {
        let lines = markdown_to_lines("Hello world", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_to_plain_text(&lines[0]), "Hello world");
    }

    #[test]
    fn bold_text() {
        let lines = markdown_to_lines("Hello **world**", 80);
        assert_eq!(lines.len(), 1);
        let text = line_to_plain_text(&lines[0]);
        assert!(text.contains("world"));
        assert!(has_modifier(&lines[0], Modifier::BOLD));
    }

    #[test]
    fn italic_text() {
        let lines = markdown_to_lines("Hello *world*", 80);
        assert_eq!(lines.len(), 1);
        let text = line_to_plain_text(&lines[0]);
        assert!(text.contains("world"));
        assert!(has_modifier(&lines[0], Modifier::ITALIC));
    }

    #[test]
    fn inline_code() {
        let lines = markdown_to_lines("Use `cargo build`", 80);
        assert_eq!(lines.len(), 1);
        let text = line_to_plain_text(&lines[0]);
        assert!(text.contains("cargo build"));
        assert!(has_bg_color(&lines[0], DEFAULT_THEME.code_bg));
    }

    #[test]
    fn code_block() {
        let input = "```rust\nfn main() {}\n```";
        let lines = markdown_to_lines(input, 80);
        // Should produce at least 3 lines: top border, code line, bottom border
        assert!(lines.len() >= 3, "code block should have >= 3 lines, got {}", lines.len());
        // Top border should contain the language
        let top = line_to_plain_text(&lines[0]);
        assert!(top.contains("rust"), "top border should contain language label");
        // Code line should contain the code
        let code = line_to_plain_text(&lines[1]);
        assert!(code.contains("fn main()"), "code line should contain the code");
    }

    #[test]
    fn heading_h1() {
        let lines = markdown_to_lines("# Title", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_to_plain_text(&lines[0]), "Title");
        assert!(has_modifier(&lines[0], Modifier::BOLD));
        assert!(has_modifier(&lines[0], Modifier::UNDERLINED));
    }

    #[test]
    fn heading_h2() {
        let lines = markdown_to_lines("## Title", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_to_plain_text(&lines[0]), "Title");
        assert!(has_modifier(&lines[0], Modifier::BOLD));
        // h2 should NOT be underlined
        assert!(!has_modifier(&lines[0], Modifier::UNDERLINED));
    }

    #[test]
    fn list_item() {
        let input = "- item one\n- item two";
        let lines = markdown_to_lines(input, 80);
        assert!(lines.len() >= 2, "list should have >= 2 lines, got {}", lines.len());
        let first = line_to_plain_text(&lines[0]);
        let second = line_to_plain_text(&lines[1]);
        assert!(first.contains("\u{2022}"), "first line should have bullet");
        assert!(first.contains("item one"));
        assert!(second.contains("\u{2022}"), "second line should have bullet");
        assert!(second.contains("item two"));
    }

    #[test]
    fn blockquote() {
        let lines = markdown_to_lines("> quoted text", 80);
        assert!(!lines.is_empty());
        let text = line_to_plain_text(&lines[0]);
        assert!(text.contains("\u{250a}"), "blockquote should contain ┊ prefix");
        assert!(text.contains("quoted text"));
    }

    #[test]
    fn link_text() {
        let lines = markdown_to_lines("[click](http://example.com)", 80);
        assert_eq!(lines.len(), 1);
        let text = line_to_plain_text(&lines[0]);
        assert!(text.contains("click"), "link text should be present");
        // URL should be discarded from display
        assert!(!text.contains("http://"), "URL should not appear in output");
        assert!(has_modifier(&lines[0], Modifier::UNDERLINED));
        assert!(has_fg_color(&lines[0], DEFAULT_THEME.link));
    }

    #[test]
    fn wraps_long_lines() {
        let long_text = "a ".repeat(50); // 100 chars
        let lines = markdown_to_lines(&long_text, 40);
        assert!(
            lines.len() > 1,
            "100-char text at width=40 should wrap to > 1 line, got {}",
            lines.len()
        );
    }

    #[test]
    fn empty_lines_preserved() {
        let input = "a\n\nb";
        let lines = markdown_to_lines(input, 80);
        assert_eq!(lines.len(), 3, "should have 3 lines: a, empty, b");
        assert_eq!(line_to_plain_text(&lines[0]), "a");
        assert!(line_to_plain_text(&lines[1]).is_empty());
        assert_eq!(line_to_plain_text(&lines[2]), "b");
    }

    #[test]
    fn unterminated_code_block() {
        let input = "```rust\nfn main()";
        let lines = markdown_to_lines(input, 80);
        // Should still render something (graceful degradation)
        assert!(!lines.is_empty(), "unterminated code block should produce output");
        // Should contain the code
        let all_text: String = lines.iter().map(|l| line_to_plain_text(l)).collect::<Vec<_>>().join("\n");
        assert!(all_text.contains("fn main()"), "code should still appear");
    }
}
