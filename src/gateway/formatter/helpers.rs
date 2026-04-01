//! Shared helper functions for message formatting.

/// Replace paired markers like `**` with open/close tags.
pub(super) fn replace_paired_marker(text: &str, marker: &str, open: &str, close: &str) -> String {
    let mut result = text.to_string();
    while let Some(start) = result.find(marker) {
        let after_start = start + marker.len();
        if after_start >= result.len() {
            break;
        }
        if let Some(rel_end) = result[after_start..].find(marker) {
            let end = after_start + rel_end;
            let inner = &result[after_start..end];
            result = format!(
                "{}{}{}{}{}",
                &result[..start],
                open,
                inner,
                close,
                &result[end + marker.len()..]
            );
        } else {
            break;
        }
    }
    result
}

/// Like `replace_paired_marker` but advances a cursor so the output is never
/// re-scanned. This avoids infinite loops when the replacement contains the
/// marker (e.g. `*` -> `**`).
pub(super) fn replace_paired_marker_positional(
    text: &str,
    marker: &str,
    open: &str,
    close: &str,
) -> String {
    let mlen = marker.len();
    let mut result = text.to_string();
    let mut cursor = 0;

    loop {
        if cursor >= result.len() {
            break;
        }
        if let Some(rel_start) = result[cursor..].find(marker) {
            let start = cursor + rel_start;
            let after_start = start + mlen;
            if after_start >= result.len() {
                break;
            }
            if let Some(rel_end) = result[after_start..].find(marker) {
                let end = after_start + rel_end;
                let inner = result[after_start..end].to_string();
                let replacement = format!("{}{}{}", open, inner, close);
                let new_cursor = start + replacement.len();
                result = format!(
                    "{}{}{}",
                    &result[..start],
                    replacement,
                    &result[end + mlen..]
                );
                cursor = new_cursor;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    result
}

/// Replace single `*` used for italic (not part of `**`).
pub(super) fn replace_single_asterisk_italic(text: &str, open: &str, close: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_italic = false;
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '*'
            && (i == 0 || chars[i - 1] != '*')
            && (i + 1 >= chars.len() || chars[i + 1] != '*')
        {
            if in_italic {
                out.push_str(close);
            } else {
                out.push_str(open);
            }
            in_italic = !in_italic;
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }

    out
}

/// Strip single `*` markers (for plain text conversion).
pub(super) fn strip_single_asterisk(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());

    for (i, &ch) in chars.iter().enumerate() {
        if ch == '*'
            && (i == 0 || chars[i - 1] != '*')
            && (i + 1 >= chars.len() || chars[i + 1] != '*')
        {
            continue;
        }
        out.push(ch);
    }

    out
}

/// Replace `[text](url)` links with a custom format.
///
/// # Known limitations
///
/// - **False positives with bracket-paren adjacency**: patterns like `array[0](foo)`
///   will be misinterpreted as a Markdown link with link text `0` and URL `foo`.
/// - **URLs containing parentheses**: URLs with literal `)` (e.g., Wikipedia links
///   like `https://en.wikipedia.org/wiki/Rust_(programming_language)`) will be
///   truncated at the first `)` because the parser uses a simple greedy `find(')')`.
pub(super) fn replace_links(text: &str, fmt_fn: impl Fn(&str, &str) -> String) -> String {
    let mut result = text.to_string();

    loop {
        if let Some(bracket_start) = result.find('[') {
            if let Some(rel_bracket_end) = result[bracket_start..].find("](") {
                let bracket_end = bracket_start + rel_bracket_end;
                if let Some(rel_paren_end) = result[bracket_end + 2..].find(')') {
                    let paren_end = bracket_end + 2 + rel_paren_end;
                    let link_text = &result[bracket_start + 1..bracket_end];
                    let url = &result[bracket_end + 2..paren_end];
                    let replacement = fmt_fn(link_text, url);
                    result = format!(
                        "{}{}{}",
                        &result[..bracket_start],
                        replacement,
                        &result[paren_end + 1..]
                    );
                    continue;
                }
            }
        }
        break;
    }

    result
}

/// Escape HTML special characters.
pub(super) fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Replace a simple HTML tag pair with open/close markers.
pub(super) fn replace_html_tag(html: &str, tag: &str, open: &str, close: &str) -> String {
    let open_tag = format!("<{tag}>");
    let close_tag = format!("</{tag}>");
    let mut result = html.to_string();

    loop {
        if let Some(start) = result.find(&open_tag) {
            if let Some(rel_end) = result[start + open_tag.len()..].find(&close_tag) {
                let content_start = start + open_tag.len();
                let content_end = content_start + rel_end;
                let inner = &result[content_start..content_end];
                result = format!(
                    "{}{}{}{}{}",
                    &result[..start],
                    open,
                    inner,
                    close,
                    &result[content_end + close_tag.len()..]
                );
                continue;
            }
        }
        break;
    }

    result
}

/// Replace `<pre><code ...>text</code></pre>` with fenced code blocks.
pub(super) fn replace_pre_code_blocks(html: &str) -> String {
    let mut result = html.to_string();

    loop {
        if let Some(pre_start) = result.find("<pre><code") {
            // Find the end of the <code ...> opening tag.
            let after_code = &result[pre_start + 10..]; // skip "<pre><code"
            if let Some(tag_close) = after_code.find('>') {
                let attrs = &after_code[..tag_close];
                let lang = extract_language_from_attrs(attrs);

                let content_start = pre_start + 10 + tag_close + 1;
                let remaining = &result[content_start..];

                if let Some(close_pos) = remaining.find("</code></pre>") {
                    let code = &remaining[..close_pos];
                    let after = &remaining[close_pos + 13..]; // "</code></pre>".len() == 13

                    if lang.is_empty() {
                        result = format!("{}```\n{}```{}", &result[..pre_start], code, after);
                    } else {
                        result =
                            format!("{}```{}\n{}```{}", &result[..pre_start], lang, code, after);
                    }
                    continue;
                }
            }
        }
        break;
    }

    result
}

/// Extract language from `class="language-xxx"` attribute string.
fn extract_language_from_attrs(attrs: &str) -> String {
    if let Some(class_start) = attrs.find("language-") {
        let after = &attrs[class_start + 9..];
        let end = after.find(['"', '\'', ' ', '>']).unwrap_or(after.len());
        after[..end].to_string()
    } else {
        String::new()
    }
}

/// Replace `<a href="url">text</a>` with `[text](url)`.
pub(super) fn replace_html_links(html: &str) -> String {
    let mut result = html.to_string();

    loop {
        if let Some(a_start) = result.find("<a href=\"") {
            let url_start = a_start + 9; // "<a href=\"".len()
            if let Some(rel_quote_end) = result[url_start..].find('"') {
                let url = &result[url_start..url_start + rel_quote_end];

                // Find the closing > of the <a> tag.
                let tag_rest = &result[url_start + rel_quote_end..];
                if let Some(tag_close) = tag_rest.find('>') {
                    let text_start = url_start + rel_quote_end + tag_close + 1;
                    if let Some(rel_a_close) = result[text_start..].find("</a>") {
                        let link_text = &result[text_start..text_start + rel_a_close];
                        let after = &result[text_start + rel_a_close + 4..];
                        result = format!("{}[{}]({}){}", &result[..a_start], link_text, url, after);
                        continue;
                    }
                }
            }
        }
        break;
    }

    result
}

// ---------------------------------------------------------------------------
// Block-level Markdown parsing (shared across all platform converters)
// ---------------------------------------------------------------------------

/// A block-level element parsed from standard Markdown.
///
/// Platform converters consume these to produce channel-specific output.
#[derive(Debug, PartialEq)]
pub(super) enum BlockElement<'a> {
    /// Fenced code block with optional language tag.
    CodeBlock { lang: &'a str, code: &'a str },
    /// Heading (level 1-6).
    Heading { level: u8, text: &'a str },
    /// Horizontal rule (`---`, `***`, `___`).
    HorizontalRule,
    /// One or more consecutive blockquote lines.
    Blockquote(Vec<&'a str>),
    /// Unordered list item (`- text` or `* text`).
    UnorderedListItem(&'a str),
    /// Regular text line (may contain inline formatting).
    Text(&'a str),
}

/// Parse Markdown text into a sequence of block-level elements.
///
/// Code blocks are extracted first (they span multiple lines and their inner
/// content must not be interpreted). The remaining text is split into lines
/// and classified into headings, horizontal rules, blockquotes, list items,
/// or plain text.
pub(super) fn parse_markdown_blocks(text: &str) -> Vec<BlockElement<'_>> {
    let mut blocks = Vec::new();

    // First pass: split on fenced code blocks.
    let mut rest = text;
    while let Some(fence_start) = rest.find("```") {
        let before = &rest[..fence_start];
        if !before.is_empty() {
            parse_line_blocks(before, &mut blocks);
        }

        let after_fence = &rest[fence_start + 3..];

        // Detect optional language tag (until newline).
        let (lang, code_start) = if let Some(nl) = after_fence.find('\n') {
            let tag = after_fence[..nl].trim();
            (tag, nl + 1)
        } else {
            ("", 0)
        };

        let code_body = &after_fence[code_start..];

        if let Some(close) = code_body.find("```") {
            let code = &code_body[..close];
            blocks.push(BlockElement::CodeBlock { lang, code });
            rest = &code_body[close + 3..];
        } else {
            // Unclosed fence — treat remainder as code block.
            blocks.push(BlockElement::CodeBlock {
                lang,
                code: code_body,
            });
            rest = "";
            break;
        }
    }

    // Remaining text after all code blocks.
    if !rest.is_empty() {
        parse_line_blocks(rest, &mut blocks);
    }

    blocks
}

/// Classify individual lines into block elements (everything except code blocks).
fn parse_line_blocks<'a>(text: &'a str, out: &mut Vec<BlockElement<'a>>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Empty line → preserve as empty text
        if trimmed.is_empty() {
            out.push(BlockElement::Text(line));
            i += 1;
            continue;
        }

        // Horizontal rule: ---, ***, ___  (3+ repeating chars, possibly spaces)
        if is_horizontal_rule(trimmed) {
            out.push(BlockElement::HorizontalRule);
            i += 1;
            continue;
        }

        // Heading: # text
        if let Some((level, heading_text)) = parse_heading(trimmed) {
            out.push(BlockElement::Heading {
                level,
                text: heading_text,
            });
            i += 1;
            continue;
        }

        // Blockquote: > text (collect consecutive lines)
        if parse_blockquote_line(trimmed).is_some() {
            let mut quote_lines = Vec::new();
            while i < lines.len() {
                if let Some(inner) = parse_blockquote_line(lines[i].trim()) {
                    quote_lines.push(inner);
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(BlockElement::Blockquote(quote_lines));
            continue;
        }

        // Unordered list item: - text or * text
        if let Some(item_text) = parse_list_item(trimmed) {
            out.push(BlockElement::UnorderedListItem(item_text));
            i += 1;
            continue;
        }

        // Regular text line
        out.push(BlockElement::Text(line));
        i += 1;
    }
}

/// Check if a trimmed line is a Markdown horizontal rule.
pub(super) fn is_horizontal_rule(trimmed: &str) -> bool {
    if trimmed.len() < 3 {
        return false;
    }
    let chars: Vec<char> = trimmed.chars().filter(|c| *c != ' ').collect();
    if chars.len() < 3 {
        return false;
    }
    let first = chars[0];
    (first == '-' || first == '*' || first == '_') && chars.iter().all(|c| *c == first)
}

/// Parse a heading line, returning (level, text) if it is one.
pub(super) fn parse_heading(trimmed: &str) -> Option<(u8, &str)> {
    // Count leading '#' characters
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    // Must be followed by a space
    let rest = &trimmed[hashes..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest[1..].trim()))
}

/// Parse a blockquote line. Returns the inner text (after `> `), or `""` for bare `>`.
pub(super) fn parse_blockquote_line(trimmed: &str) -> Option<&str> {
    if let Some(rest) = trimmed.strip_prefix("> ") {
        Some(rest)
    } else if trimmed == ">" {
        Some("")
    } else {
        None
    }
}

/// Parse an unordered list item. Returns the item text after `- ` or `* `.
pub(super) fn parse_list_item(trimmed: &str) -> Option<&str> {
    if (trimmed.starts_with("- ") || trimmed.starts_with("* ")) && trimmed.len() > 2 {
        Some(&trimmed[2..])
    } else {
        None
    }
}

/// Repair mismatched HTML tag nesting for Telegram's strict parser.
///
/// Telegram rejects HTML with improperly nested tags (e.g. `<b>...<i>...</b>...</i>`).
/// This function ensures all tags are properly closed in LIFO order by tracking
/// a stack of open tags and closing them as needed.
pub(super) fn repair_html_tags(html: &str) -> String {
    // Telegram only supports a small set of tags
    const TAGS: &[&str] = &["b", "i", "s", "u", "code", "pre"];

    let mut result = String::with_capacity(html.len());
    let mut stack: Vec<&str> = Vec::new();
    let mut rest = html;

    while !rest.is_empty() {
        // Find next '<'
        let Some(lt_pos) = rest.find('<') else {
            result.push_str(rest);
            break;
        };

        // Copy text before '<'
        result.push_str(&rest[..lt_pos]);
        rest = &rest[lt_pos..];

        // Try to parse a tag
        let Some(gt_pos) = rest.find('>') else {
            result.push_str(rest);
            break;
        };

        let full_tag = &rest[..gt_pos + 1]; // e.g. "<b>" or "</b>" or "<code class=...>"
        let tag_content = &rest[1..gt_pos]; // e.g. "b" or "/b" or "code class=..."
        let is_close = tag_content.starts_with('/');
        let tag_name = if is_close {
            tag_content[1..].split_whitespace().next().unwrap_or("")
        } else {
            tag_content.split_whitespace().next().unwrap_or("")
        };

        if let Some(&matched_tag) = TAGS.iter().find(|&&t| t == tag_name) {
            if is_close {
                // Close tag: unwind stack to find matching open tag
                if let Some(idx) = stack.iter().rposition(|&t| t == matched_tag) {
                    let to_reopen: Vec<&str> = stack[idx + 1..].iter().copied().collect();
                    for &t in stack[idx + 1..].iter().rev() {
                        result.push_str(&format!("</{t}>"));
                    }
                    result.push_str(&format!("</{matched_tag}>"));
                    stack.truncate(idx);
                    for &t in &to_reopen {
                        result.push_str(&format!("<{t}>"));
                        stack.push(t);
                    }
                }
                // else: orphan close tag — drop silently
            } else {
                // Open tag — pass through as-is (preserves attributes)
                result.push_str(full_tag);
                stack.push(matched_tag);
            }
        } else {
            // Not a known tag — pass through as-is
            result.push_str(full_tag);
        }

        rest = &rest[gt_pos + 1..];
    }

    // Close any remaining open tags
    for &t in stack.iter().rev() {
        result.push_str(&format!("</{t}>"));
    }

    result
}

/// Replace Slack-style `<url|text>` links with `[text](url)`.
pub(super) fn replace_slack_links(text: &str) -> String {
    let mut result = text.to_string();

    loop {
        if let Some(start) = result.find('<') {
            let after = &result[start + 1..];
            if let Some(pipe) = after.find('|') {
                if let Some(close) = after.find('>') {
                    if pipe < close {
                        let url = &after[..pipe];
                        let link_text = &after[pipe + 1..close];
                        result = format!(
                            "{}[{}]({}){}",
                            &result[..start],
                            link_text,
                            url,
                            &after[close + 1..]
                        );
                        continue;
                    }
                }
            }
        }
        break;
    }

    result
}
