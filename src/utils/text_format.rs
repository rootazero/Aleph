//! Shared text formatting utilities
//!
//! Common functions used by prompt assemblers across the codebase.

use chrono::{DateTime, Utc};

/// Format a Unix timestamp as a human-readable UTC string
#[must_use]
pub fn format_timestamp(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0).map_or_else(
        || "Unknown".to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )
}

#[must_use]
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    // Single-pass: find the byte offset of the (max_chars)th character
    match text.char_indices().nth(max_chars) {
        None => text.to_string(),                         // under limit
        Some((idx, _)) => format!("{}...", &text[..idx]), // truncate
    }
}

#[must_use]
pub fn escape_markdown(text: &str) -> String {
    // Prefix each Markdown metacharacter with a backslash. A previous
    // implementation used '\0' as a "no-prefix" sentinel and filtered it
    // out afterwards, which silently dropped any literal NUL present in the
    // input. Pushing directly avoids the sentinel collision and preserves
    // every input character verbatim.
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '[' | ']' | '(' | ')' | '*' | '_' | '`' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        // 2024-01-15 00:00:00 UTC
        let result = format_timestamp(1_705_276_800);
        assert!(result.contains("2024-01-15"));
    }

    #[test]
    fn test_format_timestamp_invalid() {
        let result = format_timestamp(i64::MIN);
        assert_eq!(result, "Unknown");
    }

    #[test]
    fn test_truncate_text_zero_limit() {
        let text = "Hello world";
        assert_eq!(truncate_text(text, 0), "");
    }

    #[test]
    fn test_truncate_text_under_limit() {
        let text = "Hello world";
        assert_eq!(truncate_text(text, 20), "Hello world");
    }

    #[test]
    fn test_truncate_text_over_limit() {
        let text = "Hello world, this is a longer text";
        let result = truncate_text(text, 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 13 + 3); // 10 chars + "..."
    }

    #[test]
    fn test_truncate_text_unicode() {
        let text = "你好世界，这是一段中文";
        let result = truncate_text(text, 5);
        assert!(result.ends_with("..."));
        assert_eq!(result, "你好世界，...");
    }

    #[test]
    fn test_escape_markdown() {
        let text = "[link](url) *bold* _italic_";
        let result = escape_markdown(text);
        assert!(!result.contains("[link]"));
        assert!(result.contains("\\["));
        assert!(result.contains("\\*"));
    }

    #[test]
    fn test_escape_markdown_preserves_nul() {
        // A literal NUL must survive escaping (it was previously dropped by a
        // '\0' sentinel used to mark "no backslash needed").
        let text = "a\0b*c";
        let result = escape_markdown(text);
        assert_eq!(result, "a\0b\\*c");
    }
}
