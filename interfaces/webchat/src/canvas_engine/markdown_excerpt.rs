//! Convert a Markdown excerpt to a tightly-whitelisted HTML string.
//!
//! Supports: **bold**, `inline code`, [link](url), hard line breaks, and
//! readable block separation — headings/paragraphs are split with `<br>` and
//! list items are prefixed with a `•` bullet (instead of mashing every block's
//! text into one run-on line). All other structure (blockquotes, images, raw
//! html) is stripped to plain text. Output is safe to feed into `inner_html=`.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

const MAX_LEN: usize = 180;

/// Render `src` (raw Markdown) into a 180-char whitelisted HTML string.
/// Truncates with an ellipsis if the source is longer.
#[must_use]
pub fn render_excerpt(src: &str) -> String {
    let parser = Parser::new(src);
    let mut out = String::with_capacity(src.len().min(MAX_LEN * 2));
    let mut chars_used = 0_usize;

    for event in parser {
        if chars_used >= MAX_LEN {
            out.push('\u{2026}');
            break;
        }
        match event {
            Event::Text(t) => {
                let remaining = MAX_LEN.saturating_sub(chars_used);
                let take = t.chars().take(remaining).collect::<String>();
                chars_used += take.chars().count();
                out.push_str(&html_escape(&take));
            }
            Event::Code(t) => {
                out.push_str("<code>");
                out.push_str(&html_escape(&t));
                out.push_str("</code>");
                chars_used += t.chars().count();
            }
            Event::Start(Tag::Strong) => out.push_str("<strong>"),
            Event::End(TagEnd::Strong) => out.push_str("</strong>"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str("<a target=\"_blank\" rel=\"noopener\" href=\"");
                out.push_str(&html_escape(&dest_url));
                out.push_str("\">");
            }
            Event::End(TagEnd::Link) => out.push_str("</a>"),
            Event::HardBreak | Event::SoftBreak => out.push_str("<br>"),
            // Block boundaries: separate headings/paragraphs with a line break so
            // multi-block notes read cleanly instead of running together. The
            // leading separator is suppressed when `out` is still empty so a
            // single-block note (the common case) is byte-identical to before.
            Event::Start(Tag::Paragraph) | Event::Start(Tag::Heading { .. }) => {
                if !out.is_empty() {
                    out.push_str("<br>");
                }
            }
            // List items get a bullet prefix in addition to the line break.
            Event::Start(Tag::Item) => {
                if !out.is_empty() {
                    out.push_str("<br>");
                }
                out.push_str("\u{2022} ");
            }
            Event::Html(h) => {
                // Raw HTML is escaped and treated as plain text
                chars_used += h.chars().count();
                out.push_str(&html_escape(&h));
            }
            // Everything else: ignore the tag, the inner Text events still emit
            _ => {}
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_plain_text() {
        assert_eq!(render_excerpt("hello world"), "hello world");
    }

    #[test]
    fn renders_bold_inline_code_and_link() {
        let out = render_excerpt("**bold** and `code` and [x](https://example.com)");
        assert!(out.contains("<strong>bold</strong>"));
        assert!(out.contains("<code>code</code>"));
        assert!(out
            .contains("<a target=\"_blank\" rel=\"noopener\" href=\"https://example.com\">x</a>"));
    }

    #[test]
    fn strips_headers_and_lists_to_text() {
        let out = render_excerpt("# Title\n- item\n- item");
        assert!(!out.contains("<h1>"));
        assert!(!out.contains("<ul>"));
        assert!(out.contains("Title"));
        assert!(out.contains("item"));
    }

    #[test]
    fn separates_blocks_for_readability() {
        // Heading + two list items: blocks must not mash together. The heading
        // is followed by a break; each item is bulleted on its own line.
        let out = render_excerpt("# Title\n- item one\n- item two");
        assert!(out.contains("Title<br>"));
        assert!(out.contains("\u{2022} item one"));
        assert!(out.contains("\u{2022} item two"));
        // No leading separator before the first block.
        assert!(!out.starts_with("<br>"));
    }

    #[test]
    fn two_paragraphs_get_a_break() {
        let out = render_excerpt("first para\n\nsecond para");
        assert_eq!(out, "first para<br>second para");
    }

    #[test]
    fn escapes_raw_html_attempts() {
        let out = render_excerpt("<script>alert(1)</script> ok");
        assert!(!out.contains("<script>"));
        assert!(out.contains("ok"));
    }

    #[test]
    fn truncates_long_input_with_ellipsis() {
        let long = "x".repeat(300);
        let out = render_excerpt(&long);
        assert!(out.ends_with('\u{2026}'));
        assert!(out.chars().count() <= MAX_LEN + 1);
    }
}
