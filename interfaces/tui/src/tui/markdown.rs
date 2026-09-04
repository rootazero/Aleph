use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::rc::Rc;
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
            let inline = parse_inline(content, Style::default().fg(DEFAULT_THEME.quote));
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

/// Frozen-prefix half of an incremental streaming render — see
/// [`markdown_to_lines_incremental`]. `Rc`-shared so the cache and the
/// current frame can both hold the prefix without a deep copy.
#[derive(Debug)]
pub struct StreamPrefix {
    /// Byte offset into the source text up to which `lines` renders.
    pub safe_offset: usize,
    /// Pane width the prefix was wrapped for; a resize invalidates (mirrors
    /// the width check `CachedEntry` does for the whole-message cache).
    pub width: u16,
    /// Rendered lines for exactly `text[..safe_offset]` — nothing more. The
    /// unfrozen tail is never baked in here (it keeps changing byte for
    /// byte; a snapshot of it would resurface as a stale duplicate the next
    /// time the boundary advanced).
    pub lines: Rc<Vec<Line<'static>>>,
}

/// One frame of an incremental streaming render: the frozen prefix (shared
/// with the cache, so holding it costs one `Rc` bump) plus the freshly
/// rendered unfrozen tail.
#[derive(Debug)]
pub struct StreamLines {
    pub prefix: Rc<Vec<Line<'static>>>,
    pub tail: Vec<Line<'static>>,
}

impl StreamLines {
    /// Total rendered line count across the prefix/tail seam.
    pub fn line_count(&self) -> usize {
        self.prefix.len() + self.tail.len()
    }

    /// Borrow line `i` across the prefix/tail seam.
    pub fn get(&self, i: usize) -> Option<&Line<'static>> {
        if i < self.prefix.len() {
            self.prefix.get(i)
        } else {
            self.tail.get(i - self.prefix.len())
        }
    }
}

/// Incremental variant of [`markdown_to_lines`] for a still-growing message.
///
/// `cache` holds the [`StreamPrefix`] from the previous call. Only the text
/// from `cache.safe_offset` to the new
/// `shared_ui_logic::markdown_stream::safe_freeze_offset` boundary is
/// re-converted; the frozen prefix is reused via `Rc` with **zero deep
/// copies** (the previous signature cloned the whole cached `Vec<Line>` on
/// every call — twice on boundary-advance calls — purely to satisfy the
/// borrow checker). Falls back to a full re-run of [`markdown_to_lines`] on
/// the very first call (`cache == None`) and whenever the requested `width`
/// no longer matches the cached one (a resize mid-stream).
///
/// A stale cached offset (longer than `text` or off its char boundary —
/// possible only after a wholesale content swap the cache didn't observe) is
/// treated as "no cache": the prefix is dropped and the whole text renders
/// as tail, so a stale prefix can never paint text that is no longer there.
///
/// **Note on the cost model**: unlike Panel's HTML-string cache (which only
/// re-processes the newly-safe delta and appends pre-rendered HTML), this
/// re-runs `markdown_to_lines` on the whole safe prefix `text[..new_offset]`
/// when the boundary advances, because `markdown_to_lines` returns
/// `Vec<Line<'static>>` with wrapped/styled spans that aren't trivially
/// concatenable the way HTML strings are (a `Line` wrapped at a width
/// boundary can differ depending on what came before it in the same
/// paragraph). This still avoids reprocessing whenever the boundary DOESN'T
/// advance (the common case — most ticks arrive between safe-offset
/// advances), and the tail-only reprocessing is always bounded by "how far
/// behind the safe boundary trails," not by total message length. If
/// profiling after this ships shows the prefix reformat is still too costly
/// for very long streaming messages, that's a Phase 2 candidate — not
/// attempted here (YAGNI).
pub fn markdown_to_lines_incremental(
    text: &str,
    width: u16,
    cache: &mut Option<StreamPrefix>,
) -> StreamLines {
    let prev_offset = match cache {
        Some(p) if p.width == width => p.safe_offset,
        _ => {
            // No cache yet, or the pane was resized — start over.
            *cache = None;
            0
        }
    };
    let (prefix, tail_start) =
        match shared_ui_logic::markdown_stream::safe_freeze_offset(text, prev_offset) {
            Some(new_offset) if new_offset > prev_offset => {
                // The safe prefix grew. Re-run full conversion ONLY on the
                // safe prefix (cheap relative to the whole growing text as
                // long as fences close reasonably often), matching
                // markdown_to_lines's own fence-tracking semantics (it always
                // starts a fresh scan at `in_code_block = false`, which is
                // valid exactly at a safe-offset boundary by construction).
                let lines = Rc::new(markdown_to_lines(&text[..new_offset], width));
                *cache = Some(StreamPrefix {
                    safe_offset: new_offset,
                    width,
                    lines: Rc::clone(&lines),
                });
                (lines, new_offset)
            }
            _ => match cache {
                Some(p) if p.safe_offset <= text.len() && text.is_char_boundary(p.safe_offset) => {
                    let off = p.safe_offset;
                    (Rc::clone(&p.lines), off)
                }
                _ => {
                    // No usable cache: nothing is frozen, everything is tail.
                    if cache.is_some() {
                        *cache = None;
                    }
                    (Rc::new(Vec::new()), 0)
                }
            },
        };
    let tail_text = &text[tail_start..];
    let tail = if tail_text.is_empty() {
        Vec::new()
    } else {
        markdown_to_lines(tail_text, width)
    };
    StreamLines { prefix, tail }
}

/// Check if a line is a list item (starts with `- ` or `* `)
fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ") || trimmed.starts_with("* ")
}

/// Strip the list marker from a line, returning the content after `- ` or `* `
fn strip_list_marker(line: &str) -> String {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .unwrap_or(trimmed)
        .to_string()
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
        let Some(&(byte_idx, ch)) = chars.get(i) else {
            break;
        };

        match ch {
            '*' => {
                // Check for bold (**) or italic (*)
                let is_bold = chars.get(i + 1).is_some_and(|&(_, c)| c == '*');
                if is_bold {
                    // Bold: **text**
                    if let Some(end) = find_double_marker(&chars, i + 2, '*') {
                        // Flush plain text before this marker
                        flush_plain(text, plain_start, byte_idx, base_style, &mut spans);
                        let inner_start = chars.get(i + 2).map_or(text.len(), |c| c.0);
                        let inner_end = chars.get(end).map_or(text.len(), |c| c.0);
                        let inner = text.get(inner_start..inner_end).unwrap_or("");
                        spans.push(Span::styled(
                            inner.to_string(),
                            base_style.add_modifier(Modifier::BOLD),
                        ));
                        i = end + 2; // skip past closing **
                        plain_start = chars.get(i).map_or(text.len(), |c| c.0);
                        continue;
                    }
                }
                // Single italic: *text*
                if let Some(end) = find_single_marker(&chars, i + 1, '*') {
                    flush_plain(text, plain_start, byte_idx, base_style, &mut spans);
                    let inner_start = chars.get(i + 1).map_or(text.len(), |c| c.0);
                    let inner_end = chars.get(end).map_or(text.len(), |c| c.0);
                    let inner = text.get(inner_start..inner_end).unwrap_or("");
                    spans.push(Span::styled(
                        inner.to_string(),
                        base_style.add_modifier(Modifier::ITALIC),
                    ));
                    i = end + 1;
                    plain_start = chars.get(i).map_or(text.len(), |c| c.0);
                    continue;
                }
                i += 1;
            }
            '`' => {
                // Inline code: `text`
                if let Some(end) = find_single_marker(&chars, i + 1, '`') {
                    flush_plain(text, plain_start, byte_idx, base_style, &mut spans);
                    let inner_start = chars.get(i + 1).map_or(text.len(), |c| c.0);
                    let inner_end = chars.get(end).map_or(text.len(), |c| c.0);
                    let inner = text.get(inner_start..inner_end).unwrap_or("");
                    spans.push(Span::styled(
                        inner.to_string(),
                        Style::default().bg(DEFAULT_THEME.code_bg),
                    ));
                    i = end + 1;
                    plain_start = chars.get(i).map_or(text.len(), |c| c.0);
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
                    plain_start = chars.get(i).map_or(text.len(), |c| c.0);
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

/// Flush accumulated plain text (from `plain_start` to current byte index) as a styled span.
fn flush_plain(text: &str, start: usize, end: usize, style: Style, spans: &mut Vec<Span<'static>>) {
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
    (from..chars.len()).find(|&idx| chars.get(idx).is_some_and(|c| c.1 == marker))
}

/// Find a double closing marker (e.g., **), returning the char index of the first char.
fn find_double_marker(chars: &[(usize, char)], from: usize, marker: char) -> Option<usize> {
    let len = chars.len();
    (from..len.saturating_sub(1)).find(|&idx| {
        let first = chars.get(idx).is_some_and(|c| c.1 == marker);
        let second = chars.get(idx + 1).is_some_and(|c| c.1 == marker);
        first && second
    })
}

/// Parse a markdown link: [text](url). Returns (`link_text`, `char_index_after_closing_paren`).
fn parse_link(chars: &[(usize, char)], text: &str, start: usize) -> Option<(String, usize)> {
    // start is at '['
    // Find closing ']'
    let mut i = start + 1;
    while chars.get(i).is_some_and(|&(_, c)| c != ']') {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let bracket_close = i;

    // Next char must be '('
    i += 1;
    if chars.get(i).is_none_or(|&(_, c)| c != '(') {
        return None;
    }

    // Find closing ')'
    i += 1;
    while chars.get(i).is_some_and(|&(_, c)| c != ')') {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }

    // Extract link text
    let text_start = chars.get(start + 1).map_or(text.len(), |c| c.0);
    let text_end = chars.get(bracket_close).map_or(text.len(), |c| c.0);
    let link_text = text.get(text_start..text_end).unwrap_or("").to_string();

    Some((link_text, i + 1))
}

/// Render a fenced code block with borders and language label.
fn render_code_block(lang: &str, lines: &[String], width: usize, result: &mut Vec<Line<'static>>) {
    let border_style = Style::default().fg(DEFAULT_THEME.code_block_border);
    let code_style = Style::default().bg(DEFAULT_THEME.code_bg);
    let inner_width = if width > 4 { width - 2 } else { width };

    // Top border: ┌─ lang ──────
    let label = if lang.is_empty() {
        String::new()
    } else {
        format!(" {lang} ")
    };
    let label_width = UnicodeWidthStr::width(label.as_str());
    let dash_count = inner_width.saturating_sub(label_width + 1);
    let top = format!("\u{250c}\u{2500}{}{}", label, "\u{2500}".repeat(dash_count));
    result.push(Line::from(Span::styled(top, border_style)));

    // Code lines. Wrap each to the inner width (minus the "│ " gutter) so a long
    // code line becomes multiple physical rows instead of overflowing the pane.
    // The chat scroll window is computed from the logical line count, so an
    // unbounded row here would desync the height and clip the newest content.
    let code_wrap_width = inner_width.saturating_sub(2).max(1);
    for code_line in lines {
        if code_line.is_empty() {
            result.push(Line::from(Span::styled(
                "\u{2502} ".to_string(),
                code_style,
            )));
            continue;
        }
        for wrapped in textwrap::wrap(code_line, code_wrap_width) {
            let display = format!("\u{2502} {wrapped}");
            result.push(Line::from(Span::styled(display, code_style)));
        }
    }

    // Bottom border: └──────────────
    let bottom = format!("\u{2514}{}", "\u{2500}".repeat(inner_width));
    result.push(Line::from(Span::styled(bottom, border_style)));
}

/// Wrap a line of spans if total visual width exceeds the given width,
/// preserving each span's style across the wrap boundaries.
///
/// The concatenated plain text is wrapped with `textwrap`; each resulting row is
/// mapped back to a byte range in the plain text and the styled spans are
/// re-sliced against that range, so bold/italic/inline-code/link styling (and
/// the colored bullet/quote prefix) survive on every wrapped row — including
/// spans that straddle a wrap boundary, which are split with their style carried
/// to both halves.
fn wrap_line_spans(spans: &[Span<'static>], width: usize) -> Vec<Line<'static>> {
    if width == 0 || spans.is_empty() {
        return vec![Line::from(spans.to_vec())];
    }

    // Fast path: fits on one line — keep the styled spans intact.
    let total_width: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if total_width <= width {
        return vec![Line::from(spans.to_vec())];
    }

    // Build the concatenated plain text plus a parallel map of
    // (byte_start, byte_end, style) segments so every byte offset in `plain`
    // can be traced back to its originating span's style.
    let mut plain = String::new();
    let mut segments: Vec<(usize, usize, Style)> = Vec::with_capacity(spans.len());
    for span in spans {
        let start = plain.len();
        plain.push_str(span.content.as_ref());
        let end = plain.len();
        if end > start {
            segments.push((start, end, span.style));
        }
    }

    // Wrap, then map each row back to its byte range in `plain` and re-slice the
    // styled segments. textwrap may drop the whitespace it broke on, so locate
    // each row's content starting at/after a running cursor rather than assuming
    // the rows are byte-adjacent.
    let wrapped = textwrap::wrap(&plain, width);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(wrapped.len());
    let mut cursor = 0usize;
    let mut row_spans: Vec<Span<'static>> = Vec::new();
    for row in &wrapped {
        let row = row.as_ref();
        let row_start = plain[cursor..].find(row).map_or(cursor, |off| cursor + off);
        let row_end = row_start + row.len();
        cursor = row_end;

        row_spans.clear();
        for (seg_start, seg_end, style) in &segments {
            let lo = (*seg_start).max(row_start);
            let hi = (*seg_end).min(row_end);
            if lo < hi {
                if let Some(text) = plain.get(lo..hi) {
                    if !text.is_empty() {
                        row_spans.push(Span::styled(text.to_string(), *style));
                    }
                }
            }
        }
        if row_spans.is_empty() {
            row_spans.push(Span::raw(row.to_string()));
        }
        lines.push(Line::from(std::mem::take(&mut row_spans)));
    }

    if lines.is_empty() {
        lines.push(Line::from(spans.to_vec()));
    }
    lines
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
        assert!(
            lines.len() >= 3,
            "code block should have >= 3 lines, got {}",
            lines.len()
        );
        // Top border should contain the language
        let top = line_to_plain_text(&lines[0]);
        assert!(
            top.contains("rust"),
            "top border should contain language label"
        );
        // Code line should contain the code
        let code = line_to_plain_text(&lines[1]);
        assert!(
            code.contains("fn main()"),
            "code line should contain the code"
        );
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
        assert!(
            lines.len() >= 2,
            "list should have >= 2 lines, got {}",
            lines.len()
        );
        let first = line_to_plain_text(&lines[0]);
        let second = line_to_plain_text(&lines[1]);
        assert!(first.contains("\u{2022}"), "first line should have bullet");
        assert!(first.contains("item one"));
        assert!(
            second.contains("\u{2022}"),
            "second line should have bullet"
        );
        assert!(second.contains("item two"));
    }

    #[test]
    fn blockquote() {
        let lines = markdown_to_lines("> quoted text", 80);
        assert!(!lines.is_empty());
        let text = line_to_plain_text(&lines[0]);
        assert!(
            text.contains("\u{250a}"),
            "blockquote should contain ┊ prefix"
        );
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
    fn wrapped_lines_preserve_styling() {
        // A bold run long enough to wrap at width 40 must keep BOLD on every row.
        let input = format!("**{}**", "bold ".repeat(20));
        let lines = markdown_to_lines(&input, 40);
        assert!(
            lines.len() > 1,
            "long bold text should wrap to > 1 line, got {}",
            lines.len()
        );
        for (i, line) in lines.iter().enumerate() {
            assert!(
                has_modifier(line, Modifier::BOLD),
                "row {i} lost BOLD styling after wrapping"
            );
        }
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
        assert!(
            !lines.is_empty(),
            "unterminated code block should produce output"
        );
        // Should contain the code
        let all_text: String = lines
            .iter()
            .map(|l| line_to_plain_text(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all_text.contains("fn main()"), "code should still appear");
    }

    #[test]
    fn incremental_and_full_conversion_produce_identical_lines() {
        let text = "line one\n```rust\nfn f() {}\n```\nline two\n";
        let full = markdown_to_lines(text, 80);

        let mut cache: Option<StreamPrefix> = None;
        let incremental = markdown_to_lines_incremental(text, 80, &mut cache);
        let combined: Vec<Line<'static>> = incremental
            .prefix
            .iter()
            .cloned()
            .chain(incremental.tail.iter().cloned())
            .collect();
        assert_eq!(full, combined);
    }

    #[test]
    fn incremental_conversion_reuses_the_cache_on_a_second_call_with_more_text() {
        let mut cache: Option<StreamPrefix> = None;
        let first_text = "line one\n";
        let first = markdown_to_lines_incremental(first_text, 80, &mut cache);
        let offset1 = cache.as_ref().map(|p| p.safe_offset);
        assert!(offset1.unwrap_or(0) > 0);

        let grown_text = "line one\nline two\n";
        let second = markdown_to_lines_incremental(grown_text, 80, &mut cache);
        let offset2 = cache.as_ref().map(|p| p.safe_offset);
        assert!(offset2 >= offset1);
        let combined: Vec<Line<'static>> = second
            .prefix
            .iter()
            .cloned()
            .chain(second.tail.iter().cloned())
            .collect();
        assert_eq!(combined, markdown_to_lines(grown_text, 80));
        let _ = first;
    }

    #[test]
    fn incremental_conversion_does_not_duplicate_a_tail_baked_into_an_earlier_cache_write() {
        let mut cache: Option<StreamPrefix> = None;
        // First call: boundary advances (closing fence), but there's already
        // a non-empty tail past it ("af") — this must NOT get baked into
        // what's cached for the prefix.
        let text1 = "before\n```rust\ncode\n```\naf";
        markdown_to_lines_incremental(text1, 80, &mut cache);
        let offset1 = cache.as_ref().map(|p| p.safe_offset).unwrap_or(0);
        assert!(offset1 > 0);

        // Second call: boundary does NOT advance further, but the tail
        // grows ("af" -> "after"). The old tail render must not survive
        // into the output alongside the new one.
        let text2 = "before\n```rust\ncode\n```\nafter";
        let second = markdown_to_lines_incremental(text2, 80, &mut cache);
        assert_eq!(cache.as_ref().map(|p| p.safe_offset), Some(offset1)); // boundary hasn't moved
        let combined: Vec<Line<'static>> = second
            .prefix
            .iter()
            .cloned()
            .chain(second.tail.iter().cloned())
            .collect();
        assert_eq!(
            combined,
            markdown_to_lines(text2, 80),
            "must match a full, non-incremental re-render exactly — no stale/duplicated tail"
        );
    }

    #[test]
    fn incremental_conversion_shares_the_frozen_prefix_without_copying() {
        // The Rc-sharing contract: across a no-advance frame, the returned
        // prefix must be the SAME allocation as the cached one (the previous
        // tuple API deep-copied the whole prefix Vec on every call).
        let mut cache: Option<StreamPrefix> = None;
        let text = "line one\npartial";
        markdown_to_lines_incremental(text, 80, &mut cache);
        let cached_lines = Rc::clone(&cache.as_ref().expect("cache populated").lines);
        let grown = "line one\npartially";
        let frame = markdown_to_lines_incremental(grown, 80, &mut cache);
        assert!(
            Rc::ptr_eq(&cached_lines, &frame.prefix),
            "a no-advance frame must reuse the cached prefix allocation"
        );
    }
}
