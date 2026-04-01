//! Markdown -> Platform format conversions.
//!
//! All converters share the same block-level parsing via `parse_markdown_blocks`
//! (defined in helpers.rs). Each platform function only decides how to *render*
//! the parsed blocks, keeping parsing logic universal.

use super::helpers::*;

// ---------------------------------------------------------------------------
// Telegram HTML
// ---------------------------------------------------------------------------

/// Markdown -> Telegram HTML.
pub(super) fn markdown_to_telegram_html(text: &str) -> String {
    let blocks = parse_markdown_blocks(text);
    let mut out = String::with_capacity(text.len());

    for block in &blocks {
        match block {
            BlockElement::CodeBlock { lang, code } => {
                if lang.is_empty() {
                    out.push_str(&format!("<pre><code>{}</code></pre>", escape_html(code)));
                } else {
                    out.push_str(&format!(
                        "<pre><code class=\"language-{lang}\">{}</code></pre>",
                        escape_html(code)
                    ));
                }
            }
            BlockElement::Heading { text: h, .. } => {
                ensure_newline(&mut out);
                let formatted = telegram_inline(&escape_html(h));
                out.push_str(&format!("<b>{formatted}</b>"));
                out.push('\n');
            }
            BlockElement::HorizontalRule => {
                ensure_newline(&mut out);
                out.push_str("———\n");
            }
            BlockElement::Blockquote(lines) => {
                ensure_newline(&mut out);
                let inner = lines.join("\n");
                let formatted = telegram_inline(&escape_html(&inner));
                out.push_str(&format!("<blockquote>{formatted}</blockquote>"));
                out.push('\n');
            }
            BlockElement::UnorderedListItem(item) => {
                ensure_newline(&mut out);
                let formatted = telegram_inline(&escape_html(item));
                out.push_str(&format!("• {formatted}\n"));
            }
            BlockElement::Text(line) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&telegram_inline(&escape_html(line)));
            }
        }
    }

    // Match trailing-newline behavior of the input.
    trim_trailing_newline(&mut out, text);
    // Repair mismatched HTML tag nesting before sending to Telegram
    repair_html_tags(&out)
}

/// Apply Telegram-specific inline formatting (bold, italic, strikethrough,
/// code, links) to already-HTML-escaped text.
fn telegram_inline(s: &str) -> String {
    let mut s = s.to_string();
    s = replace_paired_marker(&s, "**", "<b>", "</b>");
    s = replace_single_asterisk_italic(&s, "<i>", "</i>");
    s = replace_paired_marker(&s, "~~", "<s>", "</s>");
    s = replace_paired_marker(&s, "`", "<code>", "</code>");
    s = replace_links(&s, |link_text, url| {
        format!("<a href=\"{url}\">{link_text}</a>")
    });
    s
}

// ---------------------------------------------------------------------------
// Slack mrkdwn
// ---------------------------------------------------------------------------

/// Markdown -> Slack mrkdwn.
pub(super) fn markdown_to_slack_mrkdwn(text: &str) -> String {
    let blocks = parse_markdown_blocks(text);
    let mut out = String::with_capacity(text.len());

    for block in &blocks {
        match block {
            BlockElement::CodeBlock { lang, code } => {
                // Slack supports ``` code blocks natively.
                if lang.is_empty() {
                    out.push_str(&format!("```\n{}```", code));
                } else {
                    // Slack ignores lang tag but we include it for readability.
                    out.push_str(&format!("```\n{}```", code));
                }
            }
            BlockElement::Heading { text: h, .. } => {
                ensure_newline(&mut out);
                let formatted = slack_inline(h);
                out.push_str(&format!("*{formatted}*\n"));
            }
            BlockElement::HorizontalRule => {
                ensure_newline(&mut out);
                out.push_str("———\n");
            }
            BlockElement::Blockquote(lines) => {
                ensure_newline(&mut out);
                for line in lines {
                    out.push_str(&format!("&gt; {}\n", slack_inline(line)));
                }
            }
            BlockElement::UnorderedListItem(item) => {
                ensure_newline(&mut out);
                out.push_str(&format!("• {}\n", slack_inline(item)));
            }
            BlockElement::Text(line) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&slack_inline(line));
            }
        }
    }

    trim_trailing_newline(&mut out, text);
    out
}

/// Slack-specific inline formatting.
fn slack_inline(text: &str) -> String {
    let mut s = text.to_string();
    // Bold: **text** -> *text*
    s = replace_paired_marker(&s, "**", "*", "*");
    // Links: [text](url) -> <url|text>
    s = replace_links(&s, |link_text, url| format!("<{url}|{link_text}>"));
    s
}

// ---------------------------------------------------------------------------
// Discord (mostly passthrough)
// ---------------------------------------------------------------------------

/// Markdown -> Discord (mostly passthrough, Discord understands standard MD).
pub(super) fn markdown_to_discord(text: &str) -> String {
    // Discord Markdown is very close to standard Markdown.
    text.to_string()
}

// ---------------------------------------------------------------------------
// IRC mIRC control codes
// ---------------------------------------------------------------------------

/// Markdown -> IRC mIRC control codes.
pub(super) fn markdown_to_irc(text: &str) -> String {
    let blocks = parse_markdown_blocks(text);
    let mut out = String::with_capacity(text.len());

    for block in &blocks {
        match block {
            BlockElement::CodeBlock { code, .. } => {
                // IRC has no code block formatting; just emit the code content.
                out.push_str(code);
            }
            BlockElement::Heading { text: h, .. } => {
                ensure_newline(&mut out);
                let formatted = irc_inline(h);
                // Bold heading in IRC
                out.push_str(&format!("\x02{formatted}\x02\n"));
            }
            BlockElement::HorizontalRule => {
                ensure_newline(&mut out);
                out.push_str("---\n");
            }
            BlockElement::Blockquote(lines) => {
                ensure_newline(&mut out);
                for line in lines {
                    out.push_str(&format!("| {}\n", irc_inline(line)));
                }
            }
            BlockElement::UnorderedListItem(item) => {
                ensure_newline(&mut out);
                out.push_str(&format!("• {}\n", irc_inline(item)));
            }
            BlockElement::Text(line) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&irc_inline(line));
            }
        }
    }

    trim_trailing_newline(&mut out, text);
    out
}

/// IRC-specific inline formatting.
fn irc_inline(text: &str) -> String {
    let mut s = text.to_string();
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

// ---------------------------------------------------------------------------
// Plain text
// ---------------------------------------------------------------------------

/// Markdown -> Plain text (strip all formatting).
pub(super) fn markdown_to_plain(text: &str) -> String {
    let blocks = parse_markdown_blocks(text);
    let mut out = String::with_capacity(text.len());

    for block in &blocks {
        match block {
            BlockElement::CodeBlock { code, .. } => {
                out.push_str(code);
            }
            BlockElement::Heading { text: h, .. } => {
                ensure_newline(&mut out);
                let formatted = plain_inline(h);
                out.push_str(&format!("{formatted}\n"));
            }
            BlockElement::HorizontalRule => {
                ensure_newline(&mut out);
                out.push_str("---\n");
            }
            BlockElement::Blockquote(lines) => {
                ensure_newline(&mut out);
                for line in lines {
                    out.push_str(&format!("  {}\n", plain_inline(line)));
                }
            }
            BlockElement::UnorderedListItem(item) => {
                ensure_newline(&mut out);
                out.push_str(&format!("• {}\n", plain_inline(item)));
            }
            BlockElement::Text(line) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&plain_inline(line));
            }
        }
    }

    trim_trailing_newline(&mut out, text);
    out
}

/// Plain-text inline: strip all formatting markers.
fn plain_inline(text: &str) -> String {
    let mut s = text.to_string();
    s = s.replace("**", "");
    s = strip_single_asterisk(&s);
    s = s.replace('`', "");
    s = replace_links(&s, |link_text, url| format!("{link_text} ({url})"));
    s
}

// ---------------------------------------------------------------------------
// Shared rendering helpers
// ---------------------------------------------------------------------------

/// Ensure the output ends with a newline (for block-level elements that need
/// to start on their own line).
fn ensure_newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Match trailing-newline behavior: output should end with `\n` iff input does.
fn trim_trailing_newline(out: &mut String, original: &str) {
    if original.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    if !original.ends_with('\n') && out.ends_with('\n') {
        out.truncate(out.trim_end_matches('\n').len());
    }
}
