//! Markdown -> Platform format conversions.

use super::helpers::*;

/// Markdown -> Telegram HTML.
///
/// Handles fenced code blocks, bold, italic, inline code, and links.
pub(super) fn markdown_to_telegram_html(text: &str) -> String {
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
pub(super) fn markdown_to_slack_mrkdwn(text: &str) -> String {
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
pub(super) fn markdown_to_discord(text: &str) -> String {
    // Discord Markdown is very close to standard Markdown.
    text.to_string()
}

/// Markdown -> IRC mIRC control codes.
pub(super) fn markdown_to_irc(text: &str) -> String {
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
pub(super) fn markdown_to_plain(text: &str) -> String {
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
