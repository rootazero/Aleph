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
pub(super) fn replace_paired_marker_positional(text: &str, marker: &str, open: &str, close: &str) -> String {
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

/// Strip fenced code block markers, keeping the code content.
pub(super) fn strip_fenced_code_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(fence_start) = rest.find("```") {
        result.push_str(&rest[..fence_start]);
        let after_fence = &rest[fence_start + 3..];

        // Skip language tag line.
        let code_start = if let Some(nl) = after_fence.find('\n') {
            nl + 1
        } else {
            0
        };

        let code_body = &after_fence[code_start..];

        if let Some(close) = code_body.find("```") {
            result.push_str(&code_body[..close]);
            rest = &code_body[close + 3..];
        } else {
            result.push_str(code_body);
            rest = "";
            break;
        }
    }

    result.push_str(rest);
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
                        result = format!(
                            "{}```{}\n{}```{}",
                            &result[..pre_start],
                            lang,
                            code,
                            after
                        );
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
        let end = after
            .find(['"', '\'', ' ', '>'])
            .unwrap_or(after.len());
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
                        result = format!(
                            "{}[{}]({}){}",
                            &result[..a_start],
                            link_text,
                            url,
                            after
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
