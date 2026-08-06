//! Shared XML escaping utilities for prompt-layer output — the single source
//! of truth for neutralizing untrusted text (skill descriptions, tool runtime
//! hints, memory bodies, agent-catalog entries) before it is interpolated into
//! the `<...>`-tagged prompt the model sees. Centralizing this keeps the
//! injection defense uniform: a closing-tag or attribute-escape payload in any
//! untrusted field becomes an inert entity, and there is exactly one place to
//! audit instead of several drifting hand-rolled copies.
//!
//! ## Layer boundary
//!
//! `escape_xml*` are **leaf helpers** — they escape ONE string. Composition
//! helpers ([`push_text_element`], [`push_block_with_attrs`]) sit on top and
//! are what prompt layers should reach for when they want to emit a
//! `<tag attr="…">…</tag>` envelope in one call, so the open/close balance and
//! the boundary (text vs attribute) cannot diverge between call sites.
//!
//! The two boundary cases codex's `<environment_context>` block and our
//! `<environment_context>` derivation both faced: a value containing `"` can
//! not appear inside an attribute without the attr variant, and a value
//! containing `&` must be escaped first either way (the entity it introduces
//! must not itself be re-escaped). The leaf helpers handle both; composition
//! helpers make sure callers don't pick the wrong one.

/// Escape XML special characters for use in **element text content**
/// (`<tag>HERE</tag>`). Escapes `&`, `<`, `>`. `&` is replaced first so the
/// entity references it introduces are not themselves re-escaped.
pub(crate) fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape XML special characters for use in a **double-quoted attribute value**
/// (`<tag attr="HERE">`). Extends [`escape_xml`] with `"` and `'` so a value
/// can never close the attribute or smuggle a second one. This is the full
/// five-character set (`& < > " '`) — match it for any value that lands inside
/// quotes, or when in doubt, since the text-only variant is a strict subset.
pub(crate) fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Append `&lt;name&gt;…escaped…&lt;/name&gt;` to `out`. Element text content
/// (NOT attribute value) — the inner string is escaped with [`escape_xml`] so a
/// `<`/`&`/etc. payload becomes inert. Returns nothing; mutates `out` to allow
/// chained building inside a single `String` buffer.
pub(crate) fn push_text_element(out: &mut String, name: &str, value: &str) {
    out.push('<');
    out.push_str(name);
    out.push('>');
    out.push_str(&escape_xml(value));
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

/// Append `&lt;name attr1="x" attr2="y"&gt;` (open tag, no body, no close) to
/// `out`. Each (name, value) pair lands as a double-quoted attribute whose
/// value is escaped with [`escape_xml_attr`]; the tag is left open so the
/// caller can append text children (escaped via [`escape_xml`]) and finally
/// close with `</name>`. Use [`open_block_with_attrs`] + body + `close_block`
/// when there is also text content. No body, no children — the caller should
/// call this and then push `/>` to emit a self-closed tag (not provided here
/// because no current caller needs it; add when one appears).
pub(crate) fn open_block_with_attrs<'a, I>(out: &mut String, name: &str, attrs: I)
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    out.push('<');
    out.push_str(name);
    for (k, v) in attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape_xml_attr(v));
        out.push('"');
    }
    out.push('>');
}

/// Close a tag previously opened by [`open_block_with_attrs`]. Appends
/// `&lt;/name&gt;` to `out`.
pub(crate) fn close_block(out: &mut String, name: &str) {
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_special_chars() {
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape_xml("clean"), "clean");
    }

    #[test]
    fn attr_escapes_quotes_and_apostrophes_too() {
        // Text variant leaves quotes alone; attr variant must not.
        assert_eq!(escape_xml("say \"hi\""), "say \"hi\"");
        assert_eq!(escape_xml_attr("say \"hi\""), "say &quot;hi&quot;");
        assert_eq!(escape_xml_attr("it's <b>"), "it&apos;s &lt;b&gt;");
        // `&` is still escaped first, exactly once.
        assert_eq!(escape_xml_attr("a & b"), "a &amp; b");
        assert_eq!(escape_xml_attr("clean"), "clean");
    }

    #[test]
    fn push_text_element_wraps_with_text_escape() {
        let mut buf = String::new();
        push_text_element(&mut buf, "cwd", "/work <x> & proj");
        assert_eq!(buf, "<cwd>/work &lt;x&gt; &amp; proj</cwd>");
    }

    #[test]
    fn open_block_with_attrs_escapes_attribute_values_and_quotes() {
        // Attribute value containing `&` must be escaped (and only once); the
        // un-escaped " becomes &quot;. A quote inside the attribute would
        // otherwise close the tag and forge a sibling attribute — this is the
        // exact injection OpenXML-style blocks died from.
        let mut buf = String::new();
        open_block_with_attrs(
            &mut buf,
            "env",
            [("host", "mac\"mini"), ("branch", "feat & x")],
        );
        assert_eq!(buf, "<env host=\"mac&quot;mini\" branch=\"feat &amp; x\">");
    }

    #[test]
    fn open_close_balanced_with_apostrophe_body_does_not_overescape() {
        // The composition pattern callers actually use to render a tagged
        // block with attribute(s) + body: open → text-escape body → close.
        // Element-text escape only covers `&`/`<`/`>` — apostrophes /
        // quotes are inert in **text** content (they DO matter in
        // attributes, hence the separate `escape_xml_attr`).
        let mut buf = String::new();
        open_block_with_attrs(&mut buf, "e", [("k", "v")]);
        buf.push_str(&escape_xml("it's <fine>"));
        close_block(&mut buf, "e");
        assert_eq!(buf, "<e k=\"v\">it's &lt;fine&gt;</e>");
    }
}
