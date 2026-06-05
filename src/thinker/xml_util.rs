//! Shared XML escaping utilities for prompt-layer output — the single source
//! of truth for neutralizing untrusted text (skill descriptions, tool runtime
//! hints, memory bodies, agent-catalog entries) before it is interpolated into
//! the `<...>`-tagged prompt the model sees. Centralizing this keeps the
//! injection defense uniform: a closing-tag or attribute-escape payload in any
//! untrusted field becomes an inert entity, and there is exactly one place to
//! audit instead of several drifting hand-rolled copies.

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
}
