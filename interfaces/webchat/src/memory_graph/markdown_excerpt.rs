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
    // pulldown splits "[[x]]" into several Text events at the bracket
    // boundaries (failed reference-link candidates), so wikilink scanning
    // must run over CONSECUTIVE Text events joined together, not per event.
    // Code spans arrive as Event::Code (never Text) and thus flush first,
    // keeping `[[..]]` inside backticks literal.
    let mut pending = String::new();

    for event in parser {
        if chars_used >= MAX_LEN {
            flush_wikilinks(&mut pending, &mut out);
            out.push('\u{2026}');
            pending.clear();
            break;
        }
        match event {
            Event::Text(t) => {
                let remaining = MAX_LEN.saturating_sub(chars_used);
                let take = t.chars().take(remaining).collect::<String>();
                chars_used += take.chars().count();
                pending.push_str(&take);
                continue;
            }
            _ => flush_wikilinks(&mut pending, &mut out),
        }
        match event {
            Event::Text(_) => unreachable!("handled above"),
            Event::Code(t) => {
                out.push_str("<code>");
                out.push_str(&html_escape(&t));
                out.push_str("</code>");
                chars_used += t.chars().count();
            }
            Event::Start(Tag::Strong) => out.push_str("<strong>"),
            Event::End(TagEnd::Strong) => out.push_str("</strong>"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                let safe_url = sanitize_link_url(&dest_url);
                out.push_str("<a target=\"_blank\" rel=\"noopener\" href=\"");
                out.push_str(&html_escape(&safe_url));
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
    flush_wikilinks(&mut pending, &mut out);
    out
}

/// Run the wikilink scanner over the joined pending text and emit
/// escaped text / `<a class="wl">` anchors into `out`.
fn flush_wikilinks(pending: &mut String, out: &mut String) {
    if pending.is_empty() {
        return;
    }
    for segment in split_wikilinks(pending) {
        match segment {
            WikiSegment::Text(text) => {
                out.push_str(&html_escape(text));
            }
            WikiSegment::Link { target, label } => {
                out.push_str("<a class=\"wl\" data-wl=\"");
                out.push_str(&html_escape(target));
                out.push_str("\">");
                out.push_str(&html_escape(label.unwrap_or(target)));
                out.push_str("</a>");
            }
        }
    }
    pending.clear();
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Event-delegation helper: walk up from the click target to the nearest
/// element carrying `data-wl` and return its value. Views attach one
/// `on:click` on the inner_html container instead of per-link closures.
#[cfg(target_arch = "wasm32")]
pub fn wikilink_click_target(ev: &web_sys::MouseEvent) -> Option<String> {
    use wasm_bindgen::JsCast;
    let el = ev.target()?.dyn_into::<web_sys::Element>().ok()?;
    let hit = el.closest("[data-wl]").ok()??;
    hit.get_attribute("data-wl")
}

/// Non-wasm (test host): no `web_sys` event delegation off-wasm (`Event::target`
/// / `closest` panic outside a real DOM); never resolves a click target.
#[cfg(not(target_arch = "wasm32"))]
pub fn wikilink_click_target(_ev: &web_sys::MouseEvent) -> Option<String> {
    None
}

/// Allow only a small set of link schemes. Reject `javascript:` and other
/// pseudo-URL schemes to prevent XSS when the excerpt is assigned to innerHTML.
fn sanitize_link_url(url: &str) -> String {
    let trimmed = url.trim();
    // Protocol-relative URLs (`//evil.com/x`) contain no colon, so the
    // `split_once(':')` test below would let them through — yet a browser
    // still navigates them by inheriting the panel's scheme. Reject up-front
    // so a memory excerpt cannot redirect off-origin.
    if trimmed.starts_with("//") {
        return "#disallowed-protocol-relative".to_string();
    }
    if let Some((scheme, _)) = trimmed.split_once(':') {
        let scheme = scheme.to_lowercase();
        if scheme == "http" || scheme == "https" || scheme == "mailto" {
            return trimmed.to_string();
        }
        // Disallowed scheme: render as plain text instead of a link.
        return format!("#disallowed-{}", scheme);
    }
    trimmed.to_string()
}

/// One segment of text after wikilink splitting.
#[derive(Debug, PartialEq)]
pub(crate) enum WikiSegment<'a> {
    Text(&'a str),
    Link {
        target: &'a str,
        label: Option<&'a str>,
    },
}

/// Hand-rolled `[[target]]` / `[[target|label]]` scanner (no regex dep in the
/// panel). Unclosed `[[` and empty targets fall through as plain text.
pub(crate) fn split_wikilinks(text: &str) -> Vec<WikiSegment<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let Some(open) = rest.find("[[") else {
            if !rest.is_empty() {
                out.push(WikiSegment::Text(rest));
            }
            return out;
        };
        let Some(close_rel) = rest[open + 2..].find("]]") else {
            if !rest.is_empty() {
                out.push(WikiSegment::Text(rest));
            }
            return out;
        };
        let inner = &rest[open + 2..open + 2 + close_rel];
        if inner.is_empty() || inner.contains("[[") {
            // Empty or nested-open: emit up to and including "[[" as text and rescan.
            out.push(WikiSegment::Text(&rest[..open + 2]));
            rest = &rest[open + 2..];
            continue;
        }
        if open > 0 {
            out.push(WikiSegment::Text(&rest[..open]));
        }
        let (target, label) = match inner.split_once('|') {
            Some((t, l)) if !l.is_empty() => (t, Some(l)),
            Some((t, _)) => (t, None),
            None => (inner, None),
        };
        out.push(WikiSegment::Link { target, label });
        rest = &rest[open + 2 + close_rel + 2..];
    }
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

    #[test]
    fn renders_wikilink_as_clickable_anchor() {
        let out = render_excerpt("see [[rust-notes]] here");
        assert!(
            out.contains(r#"<a class="wl" data-wl="rust-notes">rust-notes</a>"#),
            "got: {out}"
        );
    }

    #[test]
    fn renders_wikilink_alias_label() {
        let out = render_excerpt("see [[rust-notes|My Rust]] here");
        assert!(
            out.contains(r#"data-wl="rust-notes">My Rust</a>"#),
            "got: {out}"
        );
    }

    #[test]
    fn wikilink_target_is_escaped() {
        let out = render_excerpt(r#"[[a"b]]"#);
        assert!(out.contains("data-wl=\"a&quot;b\""), "got: {out}");
        assert!(!out.contains(r#"data-wl="a"b""#));
    }

    #[test]
    fn split_wikilinks_handles_mixed_text() {
        let segs = split_wikilinks("x [[a]] y [[b|B]] [[unclosed");
        assert_eq!(segs.len(), 5); // "x ", link a, " y ", link b, " [[unclosed"
        assert!(matches!(
            segs[1],
            WikiSegment::Link {
                target: "a",
                label: None
            }
        ));
        assert!(matches!(
            segs[3],
            WikiSegment::Link {
                target: "b",
                label: Some("B")
            }
        ));
        assert!(matches!(segs[4], WikiSegment::Text(" [[unclosed")));
    }

    #[test]
    fn split_wikilinks_handles_cjk_target_and_label() {
        // Multi-byte (CJK) content around and inside the delimiters must not
        // panic on byte-index slicing and must split cleanly.
        let segs = split_wikilinks("]] [[中文|别名]]");
        assert_eq!(segs.len(), 2, "got: {segs:?}");
        assert!(matches!(segs[0], WikiSegment::Text("]] ")));
        assert!(matches!(
            segs[1],
            WikiSegment::Link {
                target: "中文",
                label: Some("别名")
            }
        ));
    }

    #[test]
    fn split_wikilinks_treats_empty_brackets_as_literal() {
        // Empty "[[]]" is not a link: the scanner emits "[[" as text, rescans
        // the remainder, and the trailing "]]" falls through as text.
        let segs = split_wikilinks("[[]]");
        assert_eq!(segs, vec![WikiSegment::Text("[["), WikiSegment::Text("]]")]);
        // The segments concatenate back to the literal input.
        let rejoined: String = segs
            .iter()
            .map(|s| match s {
                WikiSegment::Text(t) => *t,
                WikiSegment::Link { .. } => unreachable!("no links expected"),
            })
            .collect();
        assert_eq!(rejoined, "[[]]");
    }
}
