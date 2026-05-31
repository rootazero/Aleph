//! Convert a Markdown excerpt to a tightly-whitelisted HTML string.
//!
//! Supports: **bold**, `inline code`, [link](url), hard line breaks.
//! Everything else (headers, lists, blockquotes, images, html) is
//! stripped to plain text. Output is safe to feed into `inner_html=`.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

const MAX_LEN: usize = 180;

/// Render `src` (raw Markdown) into a 180-char whitelisted HTML string.
/// Truncates with an ellipsis if the source is longer.
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
