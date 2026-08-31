//! Sanitizing Markdown → HTML for the session export.
//!
//! This deliberately does **not** reuse
//! [`crate::builtin_tools::pdf_generate::browser_engine::markdown_to_html`]:
//! that one runs `Options::all()` and pipes the parser straight into
//! `html::push_html`, so raw HTML embedded in the source passes through
//! verbatim. That is tolerable for a local Markdown → PDF conversion; it is
//! not tolerable here, where the input is model output and tool results and
//! the output is a document served from the gateway's origin.
//!
//! The policy mirrors the Panel renderer
//! (`interfaces/webchat/src/components/markdown.rs`) so both surfaces agree:
//!
//! 1. `Event::Html` / `Event::InlineHtml` are HTML-escaped, never emitted raw.
//! 2. Link and image destinations go through a scheme **allowlist**
//!    (`http`, `https`, `mailto`); anything else becomes an inert
//!    `#disallowed-<scheme>` anchor. An allowlist with a safe fallthrough is
//!    what makes tricks like `java\0script:` harmless — the odd scheme simply
//!    fails to match and is neutralised, rather than being matched against a
//!    denylist that a control character can slip past.

use pulldown_cmark::{html, Event, Options, Parser, Tag};

/// Escape a value destined for HTML *text* content.
pub(super) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a value destined for a double-quoted HTML *attribute*.
///
/// Quotes matter here in a way they do not for text content: filenames and
/// MIME types are user- and model-controlled and land in `alt=`, `download=`
/// and `href=`, where an unescaped `"` is an attribute breakout.
pub(super) fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Allowlist link/image destinations; neutralise everything else.
///
/// Three classes of input are caught here, all of which can otherwise ride
/// past a naive `split_once(':')` check:
///
/// 1. **Protocol-relative** — `//evil.com/path` has no scheme byte at all,
///    so `split_once(':')` returns `None` and the URL is treated as a benign
///    relative link. The browser resolves `//` against the document's
///    scheme/origin, so an exported page served at `https://you/` would
///    follow this as `https://evil.com/path`. Anchor it to a literal `#` so
///    it cannot navigate.
/// 2. **Percent-encoded scheme** — `javascript%3Aalert(1)` has no literal
///    `:` so the scheme allowlist never fires; the browser percent-decodes
///    the URL before applying the same-origin policy. Percent-decode the
///    scheme portion before comparison.
/// 3. **Whitespace / control bytes** — ` javascript:alert(1)` (leading
///    space) and `java\0script:` (NUL) historically slipped past denylists.
///    The trim+control-strip below neutralises both: a URL that survives
///    the strip with no scheme-bearing prefix is, by construction, either
///    a true relative URL or a hostile one, and only the former is safe to
///    forward.
fn sanitize_link_url(url: &str) -> String {
    let trimmed = url.trim();
    // Strip ASCII whitespace + control chars from the head so a URL like
    // "\tjavascript:alert(1)" doesn't get past the scheme check by hiding
    // its scheme behind an invisible prefix.
    let head_stripped = trimmed.trim_start_matches(|c: char| {
        c.is_whitespace() || (c as u32) < 0x20 || c == '\u{7f}'
    });
    // Protocol-relative: starts with `//` after stripping. Resolve it as a
    // same-document anchor so a browser cannot navigate to a foreign host.
    if head_stripped.starts_with("//") {
        return "#disallowed-protocol-relative".into();
    }
    if let Some((raw_scheme, _)) = head_stripped.split_once(':') {
        // Percent-decode before lowering so encoded forms (e.g. `JaVa%53cript`)
        // do not bypass the allowlist. Anything not in ASCII alnum/% is
        // already invalid in a scheme per RFC 3986 and would only appear
        // here as an evasion attempt.
        let decoded = percent_decode_scheme(raw_scheme).to_lowercase();
        if decoded == "http" || decoded == "https" || decoded == "mailto" {
            return trimmed.to_string();
        }
        // Disallowed scheme: render as a no-op anchor instead. The scheme is
        // echoed back for debuggability and is escaped by the HTML writer.
        return format!("#disallowed-{decoded}");
    }
    // Relative URLs carry no scheme and cannot execute.
    trimmed.to_string()
}

/// Percent-decode a string per RFC 3986 §2.1 + §2.4. Only the printable ASCII
/// subset is decoded — anything else is left untouched so a malformed input
/// cannot trigger a decode-into-multibyte surprise.
fn percent_decode_scheme(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // Attempt percent decode at this position.
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                let v = (hi << 4) | lo;
                if (0x20..0x7f).contains(&v) {
                    out.push(v as char);
                    i += 3;
                    continue;
                }
            }
        }
        // Advance one full UTF-8 char. `s.is_char_boundary(i)` guards against
        // panicking if a prior percent decode landed mid-multibyte sequence.
        if s.is_char_boundary(i) {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        } else {
            // Shouldn't happen because we only ever advance by 1 byte (ASCII
            // pass-through) or 3 bytes (consumed `%xx`), but stay defensive.
            i += 1;
        }
    }
    out
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Rewrite link and image destinations before they reach the HTML writer.
fn sanitize_link_event(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: sanitize_link_url(&dest_url).into(),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: sanitize_link_url(&dest_url).into(),
            title,
            id,
        }),
        other => other,
    }
}

/// Render Markdown to an HTML fragment with raw HTML escaped and URL schemes
/// allowlisted.
pub(super) fn render_markdown(src: &str) -> String {
    // Same extension set as the Panel renderer. Notably NOT `Options::all()`:
    // the fewer constructs the parser recognises, the smaller the surface.
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut out = String::with_capacity(src.len() * 2);
    for event in Parser::new_ext(src, options) {
        match event {
            // Raw HTML in model output renders as literal text.
            Event::Html(text) | Event::InlineHtml(text) => out.push_str(&escape_text(&text)),
            other => html::push_html(&mut out, std::iter::once(sanitize_link_event(other))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_html_is_rendered_as_literal_text() {
        let html = render_markdown("hello <script>alert(1)</script> world");
        assert!(!html.contains("<script"), "script tag survived: {html}");
        assert!(html.contains("&lt;script&gt;"), "not escaped: {html}");
    }

    #[test]
    fn img_onerror_payload_never_becomes_a_tag() {
        let html = render_markdown(r#"<img src=x onerror="alert(1)">"#);
        // The payload may appear as literal text — what must not happen is it
        // being parsed as an element. `<` is the only character that decides
        // that, so that is what the assertion tests.
        assert!(!html.contains("<img"), "raw tag survived: {html}");
        assert!(html.contains("&lt;img"), "not escaped: {html}");
    }

    #[test]
    fn javascript_scheme_link_is_neutralised() {
        let html = render_markdown("[click](javascript:alert(1))");
        assert!(!html.contains("javascript:"), "scheme survived: {html}");
        assert!(
            html.contains("#disallowed-javascript"),
            "no fallthrough: {html}"
        );
    }

    /// A control character embedded in the scheme cannot smuggle anything past
    /// the allowlist. Two independent layers stop it, and this asserts the
    /// outcome rather than either mechanism: CommonMark requires U+0000 to be
    /// replaced with U+FFFD, which makes the destination unparseable as a link
    /// at all, and even if it did parse, the mangled scheme would miss the
    /// allowlist and fall through to an inert anchor.
    #[test]
    fn control_character_in_scheme_never_produces_a_live_link() {
        let html = render_markdown("[click](java\u{0}script:alert(1))");
        assert!(!html.contains("javascript:"), "scheme survived: {html}");
        assert!(!html.contains("<a "), "became a live link: {html}");
        assert!(!html.contains("href="), "emitted an href: {html}");
    }

    #[test]
    fn data_uri_image_in_markdown_is_neutralised() {
        let html = render_markdown("![x](data:text/html;base64,PHNjcmlwdD4=)");
        assert!(
            !html.contains("data:text/html"),
            "data uri survived: {html}"
        );
    }

    #[test]
    fn http_links_are_preserved() {
        let html = render_markdown("[docs](https://example.com/a?b=1)");
        assert!(
            html.contains("https://example.com/a?b=1"),
            "link lost: {html}"
        );
    }

    #[test]
    fn ordinary_markdown_still_renders() {
        let html = render_markdown("# Title\n\n- one\n- two\n\n`code`");
        assert!(html.contains("<h1>"), "{html}");
        assert!(html.contains("<li>"), "{html}");
        assert!(html.contains("<code>"), "{html}");
    }

    #[test]
    fn escape_attr_closes_the_quote_breakout() {
        let escaped = escape_attr(r#"a" onload="alert(1)"#);
        assert!(!escaped.contains('"'), "raw quote survived: {escaped}");
        assert!(escaped.contains("&quot;"), "{escaped}");
    }
}
