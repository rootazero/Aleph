//! XML helper functions for XMPP stanza building and parsing.

/// Escape special characters for XML content.
pub(super) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Extract the content between opening and closing XML tags.
///
/// For `<body>Hello world</body>`, returns `Some("Hello world")`.
/// Handles self-closing tags by returning `None`.
pub(super) fn extract_tag_content<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open_tag = format!("<{tag}");
    let close_tag = format!("</{tag}>");

    let start_pos = xml.find(&open_tag)?;
    // Find the end of the opening tag
    let tag_end = xml[start_pos..].find('>')?;
    let content_start = start_pos + tag_end + 1;

    // Check for self-closing tag
    if xml[start_pos..start_pos + tag_end + 1].ends_with("/>") {
        return None;
    }

    let content_end = xml[content_start..].find(&close_tag)?;

    Some(&xml[content_start..content_start + content_end])
}

/// Extract an XML attribute value from a tag.
///
/// Supports both single and double quoted attributes:
/// - `from="user@example.com"` -> `Some("user@example.com")`
/// - `from='user@example.com'` -> `Some("user@example.com")`
pub(super) fn extract_attribute<'a>(xml: &'a str, attr: &str) -> Option<&'a str> {
    // Find the root element opening tag (first '<' followed by non-'/' character)
    let root_start = xml.trim_start().strip_prefix('<')?;
    let root_tag_end = root_start.find('>')?;
    let root_tag = &root_start[..root_tag_end];

    // Try double quotes first: attr="value"
    let dq_pattern = format!("{attr}=\"");
    if let Some(start) = root_tag.find(&dq_pattern) {
        let value_start = start + dq_pattern.len();
        if let Some(value_end) = root_tag[value_start..].find('"') {
            return Some(&root_tag[value_start..value_start + value_end]);
        }
    }

    // Try single quotes: attr='value'
    let sq_pattern = format!("{attr}='");
    if let Some(start) = root_tag.find(&sq_pattern) {
        let value_start = start + sq_pattern.len();
        if let Some(value_end) = root_tag[value_start..].find('\'') {
            return Some(&root_tag[value_start..value_start + value_end]);
        }
    }

    None
}

/// Unescape XML entities in text content.
pub(super) fn xml_unescape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();

    while let Some(ch) = chars.next() {
        if ch == '&' {
            let remaining: String = chars.clone().collect();
            if let Some(end) = remaining.find(';') {
                let entity = &remaining[..end];
                let replacement = match entity {
                    "lt" => Some("<"),
                    "gt" => Some(">"),
                    "quot" => Some("\""),
                    "apos" => Some("'"),
                    "amp" => Some("&"),
                    _ => None,
                };
                if let Some(repl) = replacement {
                    result.push_str(repl);
                    for _ in 0..=end {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        result.push(ch);
    }

    result
}
/// Simple base64 encoding for SASL PLAIN.
pub(super) fn base64_encode(data: &[u8]) -> String {
    // Use the base64 crate that's already a dependency
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}
