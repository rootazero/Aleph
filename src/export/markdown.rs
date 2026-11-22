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
//!
//! 3. The document is rendered by a **single** HTML writer. The writer's
//!    state spans the events it is fed — a table's head/body phase, its
//!    column alignment, an image's alt text — so it has to see the whole
//!    stream.
//!
//! # Images become links, and this is the one place the export parts company
//! with the Panel
//!
//! The Panel serves from an origin whose CSP allows `img-src https:`, so a
//! remote image renders there. The export does not: its document policy is
//! `img-src data:` only, because an exported document is meant to be read
//! offline and a remote image would both break that and tell whoever the URL
//! names when and from where the document was opened
//! (see `crate::export`'s policy note).
//!
//! Emitting `<img src="https://…">` into a document that forbids it produces
//! a broken image: the reader sees an empty frame where the figure was, and
//! the alt text — the only thing that carried the meaning — is gone with it.
//! So a Markdown image is rewritten into a **link**: the destination is
//! reachable by a click, but nothing is fetched until the reader asks for it,
//! and the alt text becomes the link's text.

use pulldown_cmark::{html, Event, LinkType, Options, Parser, Tag, TagEnd};

/// Escape the characters that would let a value turn into markup.
///
/// One rule, one place: [`escape_text`] and [`escape_attr`] differ only in
/// the quotes they additionally escape, so the shared set is written here
/// once instead of being restated in two escapers that can drift apart.
fn escape_markup(s: &str) -> String {
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

/// Escape a value destined for HTML *text* content.
pub(super) fn escape_text(s: &str) -> String {
    escape_markup(s)
}

/// Escape a value destined for a double-quoted HTML *attribute*.
///
/// Quotes matter here in a way they do not for text content: filenames and
/// MIME types are user- and model-controlled and land in `alt=`, `download=`
/// and `href=`, where an unescaped `"` is an attribute breakout.
///
/// # Invariant
///
/// Every value that lands inside an attribute goes through this function.
/// The text rule leaves quotes alone, so an attribute that skipped this one
/// would be a breakout waiting to happen.
pub(super) fn escape_attr(s: &str) -> String {
    // The common rule first — `escape_markup` owns it, so a fix there reaches
    // attributes too — and then the attribute-only quotes.
    //
    // The quote pass runs over the ESCAPED string and rewrites nothing but
    // the two quote characters. That is what keeps the result byte-identical
    // to a single-pass escaper: the common rule introduces `&amp;`, `&lt;`
    // and `&gt;`, none of which contains a quote, and the quote pass leaves
    // every other character — the `&` of `&amp;` included — alone.
    let common = escape_markup(s);
    let mut out = String::with_capacity(common.len() + 16);
    for c in common.chars() {
        match c {
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Allowlist link/image destinations; neutralise everything else.
///
/// The destination is classified **the way a browser classifies it** first,
/// and only then judged against the allowlist. Four classes of input are
/// caught here, all of which can otherwise ride past a naive
/// `split_once(':')` check:
///
/// 1. **Protocol-relative** — `//evil.com/path` has no scheme byte at all,
///    so `split_once(':')` returns `None` and the URL is treated as a benign
///    relative link. The browser resolves `//` against the document's
///    scheme/origin, so an exported page served at `https://you/` would
///    follow this as `https://evil.com/path`. Anchor it to a literal `#` so
///    it cannot navigate. A backslash is a slash to a browser — WHATWG URL
///    resolution normalises `\` to `/` for a special scheme — so
///    `\\evil.com/path` and `/\evil.com/path` are the same destination
///    and are caught the same way.
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
/// 4. **A colon that is not a scheme** — `/docs/a:b`, `?next=https://x`
///    and `#section:v1` are same-origin relative references that merely
///    contain a colon. Treating the prefix before the colon as a scheme
///    rewrote them into inert anchors, which broke legitimate links in the
///    exported document. A colon introduces a scheme only when nothing that
///    ends the first path segment — `/`, `?`, `#` — comes before it, and
///    the prefix has a valid scheme shape (RFC 3986 §3.1). Every other
///    destination that is not an `http`/`https`/`mailto` absolute URL stays
///    neutralised.
fn sanitize_link_url(url: &str) -> String {
    // Trim once into an owned String so the return paths below can move it
    // out without re-allocating — two of them were `trimmed.to_string()`,
    // the others returned a literal, and a percent-decode path never reached
    // the kept copy.
    let trimmed = url.trim().to_owned();
    // Strip ASCII whitespace + control chars from the head so a URL like
    // "\tjavascript:alert(1)" doesn't get past the scheme check by hiding
    // its scheme behind an invisible prefix.
    let head_stripped = trimmed
        .trim_start_matches(|c: char| c.is_whitespace() || (c as u32) < 0x20 || c == '\u{7f}');
    // Classify the destination the way a browser classifies it: for a special
    // scheme WHATWG URL resolution treats a backslash as a forward slash, so
    // `\\evil.example/x` and `/\evil.example/x` are protocol-relative
    // destinations that resolve to `https://evil.example/x`. Classifying the
    // raw string would call them relative and let the guard's "no foreign
    // host" property be defeated by one character.
    let classified = head_stripped.replace('\\', "/");
    // Protocol-relative: starts with `//` after stripping. Resolve it as a
    // same-document anchor so a browser cannot navigate to a foreign host.
    if classified.starts_with("//") {
        return "#disallowed-protocol-relative".into();
    }
    if let Some(colon) = classified.find(':') {
        // The candidate scheme is everything before the first colon. It only
        // introduces a scheme when nothing that ends the first path segment
        // — `/`, `?`, `#` — comes before the colon; `/docs/a:b`,
        // `?next=https://x` and `#section:v1` are relative references whose
        // colon sits inside the path, the query or the fragment.
        let prefix = &classified[..colon];
        let ends_first_segment =
            prefix.contains('/') || prefix.contains('?') || prefix.contains('#');
        if !ends_first_segment {
            // Percent-decode before lowering so encoded forms (e.g. `JaVa%53cript`)
            // do not bypass the allowlist. Anything not in ASCII alnum/% is
            // already invalid in a scheme per RFC 3986 and would only appear
            // here as an evasion attempt.
            let decoded = percent_decode_scheme(prefix).to_lowercase();
            if is_scheme_shaped(&decoded) {
                if matches!(decoded.as_str(), "http" | "https" | "mailto") {
                    return trimmed;
                }
                // Disallowed scheme: render as a no-op anchor instead. The scheme is
                // echoed back for debuggability and is escaped by the HTML writer.
                return format!("#disallowed-{decoded}");
            }
            // A colon that introduces no scheme and sits in no relative
            // reference either: the first path segment of a relative
            // reference may not contain a colon (RFC 3986 §4.2), so this
            // destination is malformed. Fail closed, exactly as the old
            // unconditional `split_once(':')` did.
            return format!("#disallowed-{decoded}");
        }
    }
    // Relative URLs carry no scheme and cannot execute.
    trimmed
}

/// Is `s` shaped like a scheme (RFC 3986 §3.1)?
///
/// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`. The point is to tell a real
/// scheme apart from a colon that merely sits inside a path, so the check is
/// on the ASCII shape and nothing else — a scheme is never percent-encoded.
fn is_scheme_shaped(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
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
        // Advance one full UTF-8 char. The only advance paths are +1 byte
        // (ASCII pass-through) and +3 bytes (consumed `%xx`), both of which
        // land on a char boundary, so `s[i..]` always starts at one and is
        // non-empty while `i < s.len()`.
        let ch = s[i..]
            .chars()
            .next()
            .expect("i is on a char boundary and i < s.len()");
        out.push(ch);
        i += ch.len_utf8();
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

/// What an image was rewritten into.
enum ImageOutcome {
    /// The image became a link, so its end event closes a link.
    BecameLink,
    /// The image already sat inside a link, so only its alt text survives.
    Unwrapped,
}

/// Rewrite the event stream: sanitise destinations, defuse raw HTML, and turn
/// images into links.
///
/// Raw HTML is escaped here rather than by the writer: the writer emits HTML
/// events verbatim, so the escaped text has to travel inside the event.
///
/// An image is rewritten into a **link**. The export's document policy allows
/// `img-src data:` and nothing else, so an `<img>` pointing at a remote host
/// can never load: the reader gets an empty frame and the alt text with it.
/// A link keeps the destination reachable without fetching anything, and the
/// events that carried the alt text become the link's text.
///
/// The start and the end are rewritten as a pair, so the writer sees one
/// link and closes it: `Tag::Image` and `Tag::Link` carry the same fields,
/// and the writer's `End` arm for an image emits nothing at all (it is
/// handled in the start), so an unpaired rewrite would leave the link open.
///
/// An image *inside* a link (`[![alt](figure.png)](page)`) is unwrapped
/// instead: a link cannot nest inside a link, and the HTML parser closes the
/// outer anchor at the inner one — which would cut the outer link's text in
/// half. Only the alt text survives there, which is the part that carried the
/// meaning.
fn sanitize_events<'a>(events: impl Iterator<Item = Event<'a>>) -> Vec<Event<'a>> {
    let mut out = Vec::new();
    // Whether the writer is currently inside a link.
    let mut in_link = false;
    // One entry per open image, so its end event knows what it closes.
    let mut images: Vec<ImageOutcome> = Vec::new();
    for event in events {
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                in_link = true;
                out.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: sanitize_link_url(&dest_url).into(),
                    title,
                    id,
                }));
            }
            Event::Start(Tag::Image {
                link_type: _,
                dest_url,
                title,
                id,
            }) => {
                if in_link {
                    images.push(ImageOutcome::Unwrapped);
                    continue;
                }
                images.push(ImageOutcome::BecameLink);
                in_link = true;
                out.push(Event::Start(Tag::Link {
                    // An image never carries an email link type; forcing
                    // `Inline` keeps the writer's `mailto:` arm out of reach.
                    link_type: LinkType::Inline,
                    dest_url: sanitize_link_url(&dest_url).into(),
                    title,
                    id,
                }));
            }
            Event::End(TagEnd::Image) => {
                if let Some(ImageOutcome::BecameLink) = images.pop() {
                    in_link = false;
                    out.push(Event::End(TagEnd::Link));
                }
            }
            Event::End(TagEnd::Link) => {
                in_link = false;
                out.push(Event::End(TagEnd::Link));
            }
            // Raw HTML in model output renders as literal text.
            Event::Html(text) => out.push(Event::Html(escape_text(&text).into())),
            Event::InlineHtml(text) => out.push(Event::InlineHtml(escape_text(&text).into())),
            other => out.push(other),
        }
    }
    out
}

/// Markdown extensions this exporter parses assistant text with.
///
/// # This is a copy, and it is pinned as one
///
/// The real derivation point is
/// `shared_ui_logic::transcript::md_flags::markdown_options`, which the Panel,
/// the TUI and the CLI all call directly. This crate cannot: `shared-ui-logic`
/// is a client crate and is a **dev**-dependency here, so calling it from
/// production code would point `alephcore` at an interface crate.
///
/// What used to stand here instead was the comment "Same extension set as the
/// Panel renderer" — a claim about another subsystem, restated by hand, with
/// nothing to notice when it stopped being true. (It had already stopped being
/// true for the CLI's copy.) The claim is now made by
/// `tests::the_exporter_parses_with_the_shared_flag_set`, which reads the real
/// constant rather than repeating it.
fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options
}

/// Render Markdown to an HTML fragment with raw HTML escaped and URL schemes
/// allowlisted.
///
/// The whole document goes through **one** writer. The writer keeps state
/// across events — whether it is in a table head or body, and which column it
/// is in — so a writer per event throws that state away: every table body
/// cell came out as a header cell and the column alignment was dropped.
pub(super) fn render_markdown(src: &str) -> String {
    let mut out = String::with_capacity(src.len() * 2);
    html::push_html(
        &mut out,
        sanitize_events(Parser::new_ext(src, markdown_options())).into_iter(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This exporter parses with the same extensions as every other renderer
    /// of assistant text.
    ///
    /// # When this goes red
    ///
    /// Either side changing alone: a flag added to
    /// `shared_ui_logic::transcript::markdown_options` and not here, or here
    /// and not there. That is the whole job — a reader comparing an exported
    /// HTML file against the Panel it was exported from cannot tell which one
    /// is wrong when a table renders in one and not the other.
    ///
    /// Reads the real constant rather than restating its three flags, so this
    /// cannot pass by agreeing with a stale copy of itself (判据 §10: an
    /// assertion that only reads its own literal tests nothing).
    #[test]
    fn the_exporter_parses_with_the_shared_flag_set() {
        assert_eq!(
            markdown_options(),
            shared_ui_logic::transcript::markdown_options(),
            "the exporter's extensions drifted from the shared set"
        );
    }

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

    /// A table is rendered by a writer that remembers whether it is in the
    /// head or the body of the table. A writer per event threw that state
    /// away, so every body cell came out as a header cell and the column
    /// alignment was dropped.
    #[test]
    fn a_table_body_cell_is_a_data_cell_and_keeps_its_alignment() {
        let html = render_markdown("| left | right |\n|:---|---:|\n| a | b |");
        assert!(
            html.contains("<td style=\"text-align: left\">a</td>"),
            "body cell is not a data cell: {html}"
        );
        assert!(
            html.contains("<td style=\"text-align: right\">b</td>"),
            "alignment dropped: {html}"
        );
        assert!(
            html.contains("<th style=\"text-align: left\">"),
            "header cell lost its alignment: {html}"
        );
    }

    /// The export's document policy allows `img-src data:` and nothing else,
    /// so a remote image can never load: the reader would get an empty frame
    /// with the alt text gone with it. The image is therefore rewritten into a
    /// link, and the alt text becomes the link's text — the part that carried
    /// the meaning survives.
    #[test]
    fn a_remote_image_becomes_a_link_that_carries_its_alt_text() {
        let html = render_markdown("![a chart](https://example.com/c.png)");
        assert!(!html.contains("<img"), "an image tag survived: {html}");
        assert!(
            html.contains("href=\"https://example.com/c.png\""),
            "destination lost: {html}"
        );
        assert!(html.contains(">a chart</a>"), "alt text lost: {html}");
    }

    /// An image inside a link cannot become a link: a link does not nest
    /// inside a link, and the HTML parser closes the outer anchor at the
    /// inner one, which would cut the outer link's text in half. The image
    /// is unwrapped and only its alt text survives.
    #[test]
    fn an_image_inside_a_link_does_not_nest_a_link() {
        let html = render_markdown("[see the chart](https://example.com/page) and ![a chart](https://example.com/c.png)");
        assert_eq!(
            html.matches("<a ").count(),
            1,
            "the image opened a second link: {html}"
        );
        assert!(html.contains(">a chart</a>"), "alt text lost: {html}");
    }

    /// The link an image turned into has to be closed by the image's end
    /// event. `Tag::Image` and `Tag::Link` are closed by different arms of
    /// the writer, and the image's arm emits nothing at all — an unpaired
    /// rewrite would leave the link open for the rest of the document.
    #[test]
    fn the_link_an_image_became_is_closed() {
        let html = render_markdown("![a chart](https://example.com/c.png) and text after");
        assert_eq!(html.matches("</a>").count(), 1, "link not closed: {html}");
        assert!(
            html.contains("and text after"),
            "the rest of the document was swallowed: {html}"
        );
    }

    #[test]
    fn escape_attr_closes_the_quote_breakout() {
        let escaped = escape_attr(r#"a" onload="alert(1)"#);
        assert!(!escaped.contains('"'), "raw quote survived: {escaped}");
        assert!(escaped.contains("&quot;"), "{escaped}");
    }

    /// The common `&`, `<`, `>` rule and the attribute-only quote rule have to
    /// agree, byte for byte, with what a single-pass escaper produced — and
    /// they have to agree *because* `escape_attr` goes through the shared
    /// core, so a future fix to the common rule reaches attributes too.
    #[test]
    fn escape_attr_applies_the_shared_rule_and_the_quotes() {
        let escaped = escape_attr(r#"a & b < c > d " e ' f"#);
        assert_eq!(
            escaped, "a &amp; b &lt; c &gt; d &quot; e &#39; f",
            "the attribute escaper drifted from the shared rule"
        );
    }

    /// A backslash is a slash to a browser, so a destination that starts with
    /// two of them resolves to `https://evil.example/x`. The guard has to
    /// classify the destination the way the browser classifies it, or one
    /// character defeats its "no foreign host" property.
    #[test]
    fn a_backslash_protocol_relative_destination_is_neutralised() {
        for raw in ["\\\\evil.example/x", "/\\evil.example/x"] {
            let sanitized = sanitize_link_url(raw);
            assert!(
                sanitized.starts_with("#disallowed-"),
                "a foreign host survived: {raw} -> {sanitized}"
            );
            assert!(
                !sanitized.contains("evil.example"),
                "the foreign host survived: {raw} -> {sanitized}"
            );
        }
    }

    /// The same destination, end to end: the exported document must not carry
    /// the foreign host at all.
    ///
    /// The Markdown source needs four backslashes — CommonMark processes
    /// backslash escapes inside a link destination, so four of them are what
    /// carry a literal pair into the destination the sanitizer sees.
    #[test]
    fn a_backslash_protocol_relative_destination_never_becomes_a_live_link() {
        let html = render_markdown(r"[x](\\\\evil.example/x)");
        assert!(
            !html.contains("evil.example"),
            "foreign host survived: {html}"
        );
        assert!(
            !html.contains("href=\"//"),
            "live protocol-relative href: {html}"
        );
    }

    /// A colon that sits inside the path, the query or the fragment does not
    /// introduce a scheme, so these stay the same-origin relative references
    /// they were written as.
    #[test]
    fn a_relative_reference_with_a_colon_is_preserved() {
        for raw in ["/docs/a:b", "?next=https://example.com", "#section:v1"] {
            assert_eq!(
                sanitize_link_url(raw),
                raw,
                "a relative reference was rewritten: {raw}"
            );
        }
    }

    /// …and the same reference survives the renderer, as a real `href`.
    #[test]
    fn a_relative_link_with_a_colon_renders_as_itself() {
        let html = render_markdown("[docs](/docs/a:b)");
        assert!(
            html.contains("href=\"/docs/a:b\""),
            "relative link rewritten: {html}"
        );
    }

    /// A plain relative link carries no scheme and cannot execute.
    #[test]
    fn a_plain_relative_link_is_preserved() {
        assert_eq!(sanitize_link_url("docs/page.html"), "docs/page.html");
        assert_eq!(sanitize_link_url("../up/one.md"), "../up/one.md");
    }

    /// The allowlist itself did not move: a scheme outside it is still
    /// neutralised rather than kept.
    #[test]
    fn a_javascript_destination_is_still_rejected() {
        let sanitized = sanitize_link_url("javascript:alert(1)");
        assert!(
            sanitized.starts_with("#disallowed-"),
            "the scheme survived: {sanitized}"
        );
        assert!(!sanitized.contains("javascript:"), "{sanitized}");
    }
}
