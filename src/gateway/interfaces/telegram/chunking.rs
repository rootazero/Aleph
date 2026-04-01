//! HTML-safe chunking for Telegram message delivery.
//!
//! Splits long HTML text into chunks that respect tag integrity — unclosed
//! tags at the end of a chunk are closed, and reopened at the start of the
//! next chunk.

/// Telegram-supported HTML tags.
const TELEGRAM_TAGS: &[&str] = &[
    "b",
    "i",
    "s",
    "u",
    "code",
    "pre",
    "blockquote",
    "tg-spoiler",
];

/// Reserve space for closing/reopening tags at chunk boundaries.
const TAG_OVERHEAD: usize = 200;

/// Analyse a chunk of HTML and return the unclosed tags.
///
/// Returns `(tags_to_close, tags_to_reopen)` where:
/// - `tags_to_close` — tag names in reverse nesting order (innermost first)
/// - `tags_to_reopen` — same tag names in original nesting order (outermost first)
pub(crate) fn balance_html_tags(chunk: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut stack: Vec<&'static str> = Vec::new();
    let bytes = chunk.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'<' {
            // Find the end of this tag
            if let Some(close_pos) = chunk[i..].find('>') {
                let tag_content = &chunk[i + 1..i + close_pos];
                let is_closing = tag_content.starts_with('/');
                let tag_name_raw = if is_closing {
                    &tag_content[1..]
                } else {
                    // Strip attributes if any
                    tag_content.split_whitespace().next().unwrap_or("")
                };
                let tag_name_lower = tag_name_raw.to_lowercase();

                // Match against known Telegram tags
                if let Some(&known) = TELEGRAM_TAGS.iter().find(|&&t| t == tag_name_lower) {
                    if is_closing {
                        // Pop from stack if matching
                        if let Some(pos) = stack.iter().rposition(|&t| t == known) {
                            stack.remove(pos);
                        }
                    } else {
                        stack.push(known);
                    }
                }

                i += close_pos + 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // tags_to_close: reverse order (innermost first)
    let mut close = stack.clone();
    close.reverse();
    // tags_to_reopen: original nesting order (outermost first)
    let reopen = stack;
    (close, reopen)
}

/// Split HTML text into chunks that respect tag integrity.
///
/// Each chunk will have balanced HTML tags — unclosed tags at the end of a
/// chunk are closed, and reopened at the start of the next chunk.
/// `max_len` is in Rust chars (not bytes).
///
/// Uses a purely iterative approach: each iteration consumes one chunk from
/// `remaining` and prepends reopening tags for the next iteration.
pub(crate) fn split_html_safe(html: &str, max_len: usize) -> Vec<String> {
    if html.chars().count() <= max_len {
        return vec![html.to_string()];
    }

    let effective_limit = if max_len > TAG_OVERHEAD {
        max_len - TAG_OVERHEAD
    } else {
        max_len
    };

    let mut chunks: Vec<String> = Vec::new();
    let mut remaining = html.to_string();

    while !remaining.is_empty() {
        let char_count = remaining.chars().count();
        if char_count <= max_len {
            // Last chunk fits entirely
            chunks.push(remaining);
            break;
        }

        // Find byte offset of the effective_limit-th char
        let byte_limit = remaining
            .char_indices()
            .nth(effective_limit)
            .map(|(idx, _)| idx)
            .unwrap_or(remaining.len());

        let search_slice = &remaining[..byte_limit];

        // Split priority: \n\n > \n > space > hard cut
        let split_byte_pos = search_slice
            .rfind("\n\n")
            .map(|p| p + 2) // include the double newline in current chunk
            .or_else(|| search_slice.rfind('\n').map(|p| p + 1))
            .or_else(|| search_slice.rfind(' ').map(|p| p + 1))
            .unwrap_or(byte_limit);

        // Safety: ensure we don't split at 0 (infinite loop guard)
        let split_byte_pos = if split_byte_pos == 0 {
            byte_limit
        } else {
            split_byte_pos
        };

        let chunk_text = &remaining[..split_byte_pos];
        let rest = &remaining[split_byte_pos..];

        // Balance HTML tags
        let (close_tags, reopen_tags) = balance_html_tags(chunk_text);

        let mut chunk_str = chunk_text.to_string();
        // Append closing tags to current chunk
        for tag in &close_tags {
            chunk_str.push_str(&format!("</{}>", tag));
        }

        chunks.push(chunk_str);

        // Build next iteration's remaining: reopening tags + rest
        let mut next = String::new();
        for tag in &reopen_tags {
            next.push_str(&format!("<{}>", tag));
        }
        next.push_str(rest);
        remaining = next;
    }

    chunks
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_html_tags_no_tags() {
        let (close, open) = balance_html_tags("hello world");
        assert!(close.is_empty());
        assert!(open.is_empty());
    }

    #[test]
    fn test_balance_html_tags_unclosed() {
        let (close, open) = balance_html_tags("<b>hello");
        assert_eq!(close, vec!["b"]);
        assert_eq!(open, vec!["b"]);
    }

    #[test]
    fn test_balance_html_tags_closed() {
        let (close, open) = balance_html_tags("<b>hello</b>");
        assert!(close.is_empty());
        assert!(open.is_empty());
    }

    #[test]
    fn test_split_html_safe_short_text() {
        let chunks = split_html_safe("<b>hello</b>", 4096);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "<b>hello</b>");
    }

    #[test]
    fn test_split_html_safe_balances_bold() {
        let inner = "x".repeat(100);
        let html = format!("<b>{}</b>", inner);
        let chunks = split_html_safe(&html, 60);
        assert!(
            chunks.len() >= 2,
            "expected >=2 chunks, got {}",
            chunks.len()
        );
        assert!(
            chunks[0].ends_with("</b>"),
            "first chunk should end with </b>: {}",
            chunks[0]
        );
        assert!(
            chunks[1].starts_with("<b>"),
            "second chunk should start with <b>: {}",
            chunks[1]
        );
    }

    #[test]
    fn test_split_html_safe_prefers_newline() {
        let html = format!("{}\n{}", "a".repeat(50), "b".repeat(50));
        let chunks = split_html_safe(&html, 60);
        assert_eq!(
            chunks.len(),
            2,
            "expected 2 chunks, got {}: {:?}",
            chunks.len(),
            chunks
        );
        assert_eq!(chunks[0].trim(), &"a".repeat(50));
    }

    #[test]
    fn test_split_html_safe_nested_tags() {
        let inner = "x".repeat(100);
        let html = format!("<b><i>{}</i></b>", inner);
        let chunks = split_html_safe(&html, 60);
        assert!(
            chunks.len() >= 2,
            "expected >=2 chunks, got {}",
            chunks.len()
        );
        assert!(
            chunks[0].ends_with("</i></b>"),
            "first chunk should end with </i></b>: {}",
            chunks[0]
        );
        assert!(
            chunks[1].starts_with("<b><i>"),
            "second chunk should start with <b><i>: {}",
            chunks[1]
        );
    }

    #[test]
    fn test_split_html_safe_utf8_safety() {
        let text = "你好世界".repeat(30); // 120 CJK chars
        let chunks = split_html_safe(&text, 50);
        assert!(
            chunks.len() >= 2,
            "expected >=2 chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(
                chunk.chars().count() <= 55,
                "chunk too long: {} chars",
                chunk.chars().count()
            );
        }
    }
}
