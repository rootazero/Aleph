//! Unified cross-platform message formatting.
//!
//! Converts standard Markdown to/from platform-specific markup formats.
//! This is Phase 0 infrastructure used by all social bot channel implementations.
//!
//! # Supported formats
//!
//! - **Markdown** (passthrough, canonical internal format)
//! - **TelegramHtml**: `<b>`, `<i>`, `<code>`, `<pre><code>`, `<a href="">`
//! - **SlackMrkdwn**: `*bold*`, `_italic_`, `` `code` ``, `<url|text>`
//! - **DiscordMarkdown**: Discord-flavored Markdown (close to standard)
//! - **IrcFormatting**: mIRC control codes (`\x02` bold, `\x1D` italic)
//! - **PlainText**: all formatting stripped
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::gateway::formatter::{MessageFormatter, MarkupFormat};
//!
//! let html = MessageFormatter::format("**hello**", MarkupFormat::TelegramHtml);
//! assert_eq!(html, "<b>hello</b>");
//!
//! let chunks = MessageFormatter::split("long message...", 4096);
//! let md = MessageFormatter::normalize("<b>hello</b>", MarkupFormat::TelegramHtml);
//! ```

use std::fmt;

// ---------------------------------------------------------------------------
// MarkupFormat enum
// ---------------------------------------------------------------------------

/// Target/source markup format for message conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkupFormat {
    /// Standard Markdown (canonical internal format).
    Markdown,
    /// Telegram Bot API HTML subset.
    TelegramHtml,
    /// Slack mrkdwn format.
    SlackMrkdwn,
    /// Discord-flavored Markdown.
    DiscordMarkdown,
    /// IRC mIRC formatting codes.
    IrcFormatting,
    /// Plain text with all formatting stripped.
    PlainText,
}

impl fmt::Display for MarkupFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Markdown => write!(f, "markdown"),
            Self::TelegramHtml => write!(f, "telegram_html"),
            Self::SlackMrkdwn => write!(f, "slack_mrkdwn"),
            Self::DiscordMarkdown => write!(f, "discord_markdown"),
            Self::IrcFormatting => write!(f, "irc_formatting"),
            Self::PlainText => write!(f, "plain_text"),
        }
    }
}

// ---------------------------------------------------------------------------
// MessageFormatter
// ---------------------------------------------------------------------------

/// Unified cross-platform message formatter.
///
/// All methods are stateless and exposed as associated functions.
pub struct MessageFormatter;

impl MessageFormatter {
    /// Convert standard Markdown to the given target format.
    pub fn format(markdown: &str, target: MarkupFormat) -> String {
        match target {
            MarkupFormat::Markdown => markdown.to_string(),
            MarkupFormat::TelegramHtml => markdown_to_telegram_html(markdown),
            MarkupFormat::SlackMrkdwn => markdown_to_slack_mrkdwn(markdown),
            MarkupFormat::DiscordMarkdown => markdown_to_discord(markdown),
            MarkupFormat::IrcFormatting => markdown_to_irc(markdown),
            MarkupFormat::PlainText => markdown_to_plain(markdown),
        }
    }

    /// Smart message splitting that respects paragraph and code block boundaries.
    ///
    /// Guarantees:
    /// - Each chunk is at most `max_len` bytes.
    /// - Code blocks (triple-backtick fences) are never split mid-block
    ///   (unless a single code block exceeds `max_len`).
    /// - Splits prefer paragraph boundaries (`\n\n`), then line boundaries (`\n`).
    pub fn split(text: &str, max_len: usize) -> Vec<String> {
        if text.len() <= max_len {
            return vec![text.to_string()];
        }
        split_message(text, max_len)
    }

    /// Normalize platform-specific markup back to standard Markdown (inbound direction).
    pub fn normalize(platform_text: &str, source: MarkupFormat) -> String {
        match source {
            MarkupFormat::Markdown | MarkupFormat::DiscordMarkdown => {
                platform_text.to_string()
            }
            MarkupFormat::TelegramHtml => telegram_html_to_markdown(platform_text),
            MarkupFormat::SlackMrkdwn => slack_mrkdwn_to_markdown(platform_text),
            MarkupFormat::IrcFormatting => irc_to_markdown(platform_text),
            MarkupFormat::PlainText => platform_text.to_string(),
        }
    }
}

// ===========================================================================
// Private conversion: Markdown -> Platform
// ===========================================================================

/// Markdown -> Telegram HTML.
///
/// Handles fenced code blocks, bold, italic, inline code, and links.
fn markdown_to_telegram_html(text: &str) -> String {
    // First pass: extract and convert fenced code blocks so inner content is not
    // touched by inline formatting passes.
    let mut result = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(fence_start) = rest.find("```") {
        // Push everything before the fence through inline conversion.
        let before = &rest[..fence_start];
        result.push_str(&inline_md_to_telegram_html(before));

        let after_fence = &rest[fence_start + 3..];

        // Detect optional language tag (until newline).
        let (lang, code_start) = if let Some(nl) = after_fence.find('\n') {
            let tag = after_fence[..nl].trim();
            if tag.is_empty() {
                ("".to_string(), nl + 1)
            } else {
                (tag.to_string(), nl + 1)
            }
        } else {
            // No newline after opening fence -- treat entire remaining as code.
            ("".to_string(), 0)
        };

        let code_body = &after_fence[code_start..];

        if let Some(close) = code_body.find("```") {
            let code = &code_body[..close];
            if lang.is_empty() {
                result.push_str(&format!("<pre><code>{}</code></pre>", escape_html(code)));
            } else {
                result.push_str(&format!(
                    "<pre><code class=\"language-{lang}\">{}</code></pre>",
                    escape_html(code)
                ));
            }
            rest = &code_body[close + 3..];
        } else {
            // Unclosed fence -- render remainder as code block.
            let code = code_body;
            if lang.is_empty() {
                result.push_str(&format!("<pre><code>{}</code></pre>", escape_html(code)));
            } else {
                result.push_str(&format!(
                    "<pre><code class=\"language-{lang}\">{}</code></pre>",
                    escape_html(code)
                ));
            }
            rest = "";
            break;
        }
    }

    // Remaining text (no more fences).
    result.push_str(&inline_md_to_telegram_html(rest));
    result
}

/// Convert inline Markdown (bold, italic, code, links) to Telegram HTML.
/// Does NOT handle fenced code blocks -- the caller strips those first.
fn inline_md_to_telegram_html(text: &str) -> String {
    // Escape HTML special characters FIRST, before any Markdown-to-HTML tag
    // replacements. Markdown markers (**  *  `  []()) don't contain < > &, so
    // escaping first is safe and prevents user text like "1 < 2" from breaking
    // Telegram's HTML parser.
    let mut s = escape_html(text);

    // Bold: **text** -> <b>text</b>
    s = replace_paired_marker(&s, "**", "<b>", "</b>");

    // Italic: *text* -> <i>text</i> (single asterisks not adjacent to another *)
    s = replace_single_asterisk_italic(&s, "<i>", "</i>");

    // Inline code: `text` -> <code>text</code>
    s = replace_paired_marker(&s, "`", "<code>", "</code>");

    // Links: [text](url) -> <a href="url">text</a>
    s = replace_links(&s, |link_text, url| {
        format!("<a href=\"{url}\">{link_text}</a>")
    });

    s
}

/// Markdown -> Slack mrkdwn.
fn markdown_to_slack_mrkdwn(text: &str) -> String {
    let mut s = text.to_string();

    // Bold: **text** -> *text*
    s = replace_paired_marker(&s, "**", "*", "*");

    // Italic stays as *text* (Slack uses _italic_ but Markdown single * is
    // already understood by Slack as bold, so we leave single * as-is for now;
    // the bold conversion already consumed **).

    // Links: [text](url) -> <url|text>
    s = replace_links(&s, |link_text, url| format!("<{url}|{link_text}>"));

    s
}

/// Markdown -> Discord (mostly passthrough, Discord understands standard MD).
fn markdown_to_discord(text: &str) -> String {
    // Discord Markdown is very close to standard Markdown.
    text.to_string()
}

/// Markdown -> IRC mIRC control codes.
fn markdown_to_irc(text: &str) -> String {
    let mut s = text.to_string();

    // Fenced code blocks -> just the code content.
    s = strip_fenced_code_blocks(&s);

    // Bold: **text** -> \x02text\x02
    s = replace_paired_marker(&s, "**", "\x02", "\x02");

    // Italic: *text* -> \x1Dtext\x1D
    s = replace_single_asterisk_italic(&s, "\x1D", "\x1D");

    // Inline code: strip backticks.
    s = s.replace('`', "");

    // Links: [text](url) -> text (url)
    s = replace_links(&s, |link_text, url| format!("{link_text} ({url})"));

    s
}

/// Markdown -> Plain text (strip all formatting).
fn markdown_to_plain(text: &str) -> String {
    let mut s = text.to_string();

    // Strip fenced code blocks -> just the code content.
    s = strip_fenced_code_blocks(&s);

    // Bold: remove **
    s = s.replace("**", "");

    // Italic: remove single * (not adjacent to another *)
    s = strip_single_asterisk(&s);

    // Inline code: remove backticks.
    s = s.replace('`', "");

    // Links: [text](url) -> text (url)
    s = replace_links(&s, |link_text, url| format!("{link_text} ({url})"));

    s
}

// ===========================================================================
// Private conversion: Platform -> Markdown (normalize)
// ===========================================================================

/// Telegram HTML -> Markdown.
fn telegram_html_to_markdown(html: &str) -> String {
    let mut s = html.to_string();

    // <b>text</b> -> **text**
    s = replace_html_tag(&s, "b", "**", "**");

    // <strong>text</strong> -> **text**
    s = replace_html_tag(&s, "strong", "**", "**");

    // <i>text</i> -> *text*
    s = replace_html_tag(&s, "i", "*", "*");

    // <em>text</em> -> *text*
    s = replace_html_tag(&s, "em", "*", "*");

    // <code>text</code> -> `text`
    s = replace_html_tag(&s, "code", "`", "`");

    // <pre><code>text</code></pre> -> ```\ntext\n```
    // Also handles <pre><code class="language-xxx">
    s = replace_pre_code_blocks(&s);

    // <a href="url">text</a> -> [text](url)
    s = replace_html_links(&s);

    s
}

/// Slack mrkdwn -> Markdown.
fn slack_mrkdwn_to_markdown(text: &str) -> String {
    let mut s = text.to_string();

    // *bold* -> **bold** (Slack bold uses single *)
    // Cannot use replace_paired_marker here because marker="*" and open="**"
    // would cause an infinite loop (the output contains the marker).
    s = replace_paired_marker_positional(&s, "*", "**", "**");

    // <url|text> -> [text](url)
    s = replace_slack_links(&s);

    s
}

/// IRC formatting codes -> Markdown.
fn irc_to_markdown(text: &str) -> String {
    let mut s = text.to_string();

    // \x02text\x02 -> **text**
    s = replace_paired_marker(&s, "\x02", "**", "**");

    // \x1Dtext\x1D -> *text*
    s = replace_paired_marker(&s, "\x1D", "*", "*");

    s
}

// ===========================================================================
// Smart message splitting
// ===========================================================================

/// Split a message into chunks of at most `max_len` bytes.
fn split_message(text: &str, max_len: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        // Try to find the best split point within max_len.
        let candidate = &remaining[..max_len];

        let mut split_pos = find_split_point(candidate);

        // Character-level fallback: if find_split_point returned 0 (no viable
        // boundary found), force a hard split at max_len to guarantee forward
        // progress and the max_len contract.
        if split_pos == 0 {
            split_pos = max_len;
        }

        let (chunk, rest) = remaining.split_at(split_pos);
        let chunk = chunk.trim_end();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        remaining = rest.trim_start_matches('\n');
        if remaining.is_empty() {
            break;
        }
    }

    if chunks.is_empty() {
        chunks.push(String::new());
    }

    chunks
}

/// Find the best byte offset to split `candidate`.
///
/// Prefers paragraph boundaries, then line boundaries. Avoids splitting inside
/// fenced code blocks.
fn find_split_point(candidate: &str) -> usize {
    // Count fence openings/closings in the candidate to detect if we're mid-block.
    let fence_count = candidate.matches("```").count();
    let in_code_block = fence_count % 2 != 0;

    if in_code_block {
        // We're in the middle of a code block. Try to split BEFORE the opening
        // fence of the last unclosed block.
        if let Some(pos) = candidate.rfind("```") {
            if pos > 0 {
                return pos;
            }
        }
    }

    // Prefer double newline (paragraph boundary).
    if let Some(pos) = candidate.rfind("\n\n") {
        if pos > 0 {
            return pos;
        }
    }

    // Prefer single newline (line boundary).
    if let Some(pos) = candidate.rfind('\n') {
        if pos > 0 {
            return pos;
        }
    }

    // Last resort: split at max_len.
    candidate.len()
}

// ===========================================================================
// Shared helpers
// ===========================================================================

/// Replace paired markers like `**` with open/close tags.
fn replace_paired_marker(text: &str, marker: &str, open: &str, close: &str) -> String {
    let mut result = text.to_string();
    loop {
        if let Some(start) = result.find(marker) {
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
        } else {
            break;
        }
    }
    result
}

/// Like `replace_paired_marker` but advances a cursor so the output is never
/// re-scanned. This avoids infinite loops when the replacement contains the
/// marker (e.g. `*` -> `**`).
fn replace_paired_marker_positional(text: &str, marker: &str, open: &str, close: &str) -> String {
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
fn replace_single_asterisk_italic(text: &str, open: &str, close: &str) -> String {
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
fn strip_single_asterisk(text: &str) -> String {
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
fn replace_links(text: &str, fmt_fn: impl Fn(&str, &str) -> String) -> String {
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
fn strip_fenced_code_blocks(text: &str) -> String {
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
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Replace a simple HTML tag pair with open/close markers.
fn replace_html_tag(html: &str, tag: &str, open: &str, close: &str) -> String {
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
fn replace_pre_code_blocks(html: &str) -> String {
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
            .find(|c: char| c == '"' || c == '\'' || c == ' ' || c == '>')
            .unwrap_or(after.len());
        after[..end].to_string()
    } else {
        String::new()
    }
}

/// Replace `<a href="url">text</a>` with `[text](url)`.
fn replace_html_links(html: &str) -> String {
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
fn replace_slack_links(text: &str) -> String {
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

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Telegram HTML
    // -----------------------------------------------------------------------

    #[test]
    fn test_bold_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("**hello**", MarkupFormat::TelegramHtml),
            "<b>hello</b>"
        );
    }

    #[test]
    fn test_italic_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("*hello*", MarkupFormat::TelegramHtml),
            "<i>hello</i>"
        );
    }

    #[test]
    fn test_code_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("`code`", MarkupFormat::TelegramHtml),
            "<code>code</code>"
        );
    }

    #[test]
    fn test_link_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("[text](https://example.com)", MarkupFormat::TelegramHtml),
            "<a href=\"https://example.com\">text</a>"
        );
    }

    #[test]
    fn test_code_block_to_telegram_html() {
        let input = "```rust\nlet x = 1;\n```";
        let expected = "<pre><code class=\"language-rust\">let x = 1;\n</code></pre>";
        assert_eq!(
            MessageFormatter::format(input, MarkupFormat::TelegramHtml),
            expected
        );
    }

    #[test]
    fn test_code_block_no_lang_to_telegram_html() {
        let input = "```\nsome code\n```";
        let expected = "<pre><code>some code\n</code></pre>";
        assert_eq!(
            MessageFormatter::format(input, MarkupFormat::TelegramHtml),
            expected
        );
    }

    #[test]
    fn test_mixed_content_telegram_html() {
        let input = "**bold** and *italic* and `code`";
        let expected = "<b>bold</b> and <i>italic</i> and <code>code</code>";
        assert_eq!(
            MessageFormatter::format(input, MarkupFormat::TelegramHtml),
            expected
        );
    }

    #[test]
    fn test_html_escape_in_code_block() {
        let input = "```\n<div>test</div>\n```";
        let expected = "<pre><code>&lt;div&gt;test&lt;/div&gt;\n</code></pre>";
        assert_eq!(
            MessageFormatter::format(input, MarkupFormat::TelegramHtml),
            expected
        );
    }

    #[test]
    fn test_html_escape_inline_text() {
        // Plain text containing HTML special characters must be escaped so that
        // Telegram's HTML parser does not choke on them.
        assert_eq!(
            MessageFormatter::format("1 < 2 & 3 > 0", MarkupFormat::TelegramHtml),
            "1 &lt; 2 &amp; 3 &gt; 0"
        );
    }

    // -----------------------------------------------------------------------
    // Slack mrkdwn
    // -----------------------------------------------------------------------

    #[test]
    fn test_bold_to_slack() {
        assert_eq!(
            MessageFormatter::format("**hello**", MarkupFormat::SlackMrkdwn),
            "*hello*"
        );
    }

    #[test]
    fn test_link_to_slack() {
        assert_eq!(
            MessageFormatter::format("[text](https://example.com)", MarkupFormat::SlackMrkdwn),
            "<https://example.com|text>"
        );
    }

    #[test]
    fn test_code_preserved_slack() {
        assert_eq!(
            MessageFormatter::format("`code`", MarkupFormat::SlackMrkdwn),
            "`code`"
        );
    }

    // -----------------------------------------------------------------------
    // IRC formatting
    // -----------------------------------------------------------------------

    #[test]
    fn test_bold_to_irc() {
        assert_eq!(
            MessageFormatter::format("**hello**", MarkupFormat::IrcFormatting),
            "\x02hello\x02"
        );
    }

    #[test]
    fn test_italic_to_irc() {
        assert_eq!(
            MessageFormatter::format("*hello*", MarkupFormat::IrcFormatting),
            "\x1Dhello\x1D"
        );
    }

    #[test]
    fn test_link_to_irc() {
        assert_eq!(
            MessageFormatter::format("[text](https://example.com)", MarkupFormat::IrcFormatting),
            "text (https://example.com)"
        );
    }

    // -----------------------------------------------------------------------
    // Discord (passthrough)
    // -----------------------------------------------------------------------

    #[test]
    fn test_discord_passthrough() {
        let input = "**bold** and *italic* and `code`";
        assert_eq!(
            MessageFormatter::format(input, MarkupFormat::DiscordMarkdown),
            input
        );
    }

    // -----------------------------------------------------------------------
    // Plain text
    // -----------------------------------------------------------------------

    #[test]
    fn test_strip_to_plain() {
        assert_eq!(
            MessageFormatter::format("**bold** and *italic*", MarkupFormat::PlainText),
            "bold and italic"
        );
    }

    #[test]
    fn test_plain_strips_code() {
        assert_eq!(
            MessageFormatter::format("`code`", MarkupFormat::PlainText),
            "code"
        );
    }

    #[test]
    fn test_plain_converts_links() {
        assert_eq!(
            MessageFormatter::format("[click](https://example.com)", MarkupFormat::PlainText),
            "click (https://example.com)"
        );
    }

    #[test]
    fn test_plain_strips_code_block() {
        let input = "before\n```rust\nlet x = 1;\n```\nafter";
        let result = MessageFormatter::format(input, MarkupFormat::PlainText);
        assert!(result.contains("let x = 1;"));
        assert!(!result.contains("```"));
    }

    // -----------------------------------------------------------------------
    // Split
    // -----------------------------------------------------------------------

    #[test]
    fn test_split_short_message() {
        let result = MessageFormatter::split("short", 100);
        assert_eq!(result, vec!["short"]);
    }

    #[test]
    fn test_split_at_paragraph_boundary() {
        let text = "paragraph one\n\nparagraph two\n\nparagraph three";
        let chunks = MessageFormatter::split(text, 25);
        // Should split at paragraph boundaries.
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 25, "chunk too long: {}", chunk.len());
        }
    }

    #[test]
    fn test_split_respects_code_blocks() {
        let text = format!(
            "intro\n\n```rust\n{}\n```\n\noutro",
            "let x = 1;\n".repeat(5)
        );
        let chunks = MessageFormatter::split(&text, 200);
        // The code block should not be split across chunks.
        let mut found_partial_fence = false;
        for chunk in &chunks {
            let opens = chunk.matches("```").count();
            // If a chunk contains an opening fence, it should also contain the close.
            if opens % 2 != 0 {
                found_partial_fence = true;
            }
        }
        // This is best-effort; if the code block itself is smaller than max_len,
        // it should remain intact.
        if text.len() > 200 {
            // We expect the code block to fit in one chunk since it's ~80 bytes.
            assert!(
                !found_partial_fence,
                "code block was split across chunks"
            );
        }
    }

    #[test]
    fn test_split_at_line_boundary() {
        let text = "line one\nline two\nline three\nline four";
        let chunks = MessageFormatter::split(text, 20);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 20);
        }
    }

    #[test]
    fn test_split_long_line_hard_split() {
        // A single very long line with no newlines or other split opportunities
        // must still produce chunks that are all <= max_len.
        let long_line = "x".repeat(150);
        let max_len = 40;
        let chunks = MessageFormatter::split(&long_line, max_len);
        assert!(chunks.len() >= 4, "expected at least 4 chunks, got {}", chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.len() <= max_len,
                "chunk {} has length {} which exceeds max_len {}",
                i,
                chunk.len(),
                max_len
            );
        }
        // Verify all content is preserved.
        let reassembled: String = chunks.concat();
        assert_eq!(reassembled, long_line);
    }

    // -----------------------------------------------------------------------
    // Normalize (reverse direction)
    // -----------------------------------------------------------------------

    #[test]
    fn test_normalize_telegram_html() {
        assert_eq!(
            MessageFormatter::normalize("<b>hello</b>", MarkupFormat::TelegramHtml),
            "**hello**"
        );
    }

    #[test]
    fn test_normalize_telegram_html_italic() {
        assert_eq!(
            MessageFormatter::normalize("<i>hello</i>", MarkupFormat::TelegramHtml),
            "*hello*"
        );
    }

    #[test]
    fn test_normalize_telegram_html_code() {
        assert_eq!(
            MessageFormatter::normalize("<code>x</code>", MarkupFormat::TelegramHtml),
            "`x`"
        );
    }

    #[test]
    fn test_normalize_telegram_html_link() {
        assert_eq!(
            MessageFormatter::normalize(
                "<a href=\"https://example.com\">text</a>",
                MarkupFormat::TelegramHtml
            ),
            "[text](https://example.com)"
        );
    }

    #[test]
    fn test_normalize_telegram_html_pre_code() {
        assert_eq!(
            MessageFormatter::normalize(
                "<pre><code class=\"language-rust\">let x = 1;\n</code></pre>",
                MarkupFormat::TelegramHtml
            ),
            "```rust\nlet x = 1;\n```"
        );
    }

    #[test]
    fn test_normalize_slack_mrkdwn() {
        // Slack *hello* -> **hello** (Slack uses single * for bold)
        assert_eq!(
            MessageFormatter::normalize("*hello*", MarkupFormat::SlackMrkdwn),
            "**hello**"
        );
    }

    #[test]
    fn test_normalize_slack_link() {
        assert_eq!(
            MessageFormatter::normalize(
                "<https://example.com|text>",
                MarkupFormat::SlackMrkdwn
            ),
            "[text](https://example.com)"
        );
    }

    #[test]
    fn test_normalize_irc_bold() {
        assert_eq!(
            MessageFormatter::normalize("\x02hello\x02", MarkupFormat::IrcFormatting),
            "**hello**"
        );
    }

    #[test]
    fn test_normalize_irc_italic() {
        assert_eq!(
            MessageFormatter::normalize("\x1Dhello\x1D", MarkupFormat::IrcFormatting),
            "*hello*"
        );
    }

    #[test]
    fn test_normalize_plain_passthrough() {
        let text = "just plain text";
        assert_eq!(
            MessageFormatter::normalize(text, MarkupFormat::PlainText),
            text
        );
    }

    #[test]
    fn test_normalize_markdown_passthrough() {
        let text = "**bold** and *italic*";
        assert_eq!(
            MessageFormatter::normalize(text, MarkupFormat::Markdown),
            text
        );
    }

    // -----------------------------------------------------------------------
    // Roundtrip tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_roundtrip_telegram_bold() {
        let original = "**hello**";
        let html = MessageFormatter::format(original, MarkupFormat::TelegramHtml);
        let back = MessageFormatter::normalize(&html, MarkupFormat::TelegramHtml);
        assert_eq!(back, original);
    }

    #[test]
    fn test_roundtrip_telegram_link() {
        let original = "[click](https://example.com)";
        let html = MessageFormatter::format(original, MarkupFormat::TelegramHtml);
        let back = MessageFormatter::normalize(&html, MarkupFormat::TelegramHtml);
        assert_eq!(back, original);
    }

    #[test]
    fn test_roundtrip_slack_bold() {
        let original = "**hello**";
        let slack = MessageFormatter::format(original, MarkupFormat::SlackMrkdwn);
        let back = MessageFormatter::normalize(&slack, MarkupFormat::SlackMrkdwn);
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // Display
    // -----------------------------------------------------------------------

    #[test]
    fn test_markup_format_display() {
        assert_eq!(MarkupFormat::TelegramHtml.to_string(), "telegram_html");
        assert_eq!(MarkupFormat::PlainText.to_string(), "plain_text");
    }
}
