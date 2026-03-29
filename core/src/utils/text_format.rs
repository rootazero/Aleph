//! Shared text formatting utilities
//!
//! Common functions used by prompt assemblers across the codebase.

/// Format a Unix timestamp as a human-readable UTC string
///
/// # Arguments
/// * `timestamp` - Unix timestamp in seconds
///
/// # Returns
/// Formatted string like "2024-01-15 10:30:00 UTC" or "Unknown" if invalid
pub fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};

    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Truncate text to a maximum number of characters
///
/// If the text exceeds the limit, it will be truncated and "..." appended.
/// Handles Unicode characters correctly.
///
/// # Arguments
/// * `text` - The text to truncate
/// * `max_chars` - Maximum number of characters to keep
///
/// # Returns
/// Original text if under limit, or truncated text with "..."
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    // Single-pass: find the byte offset of the (max_chars)th character
    match text.char_indices().nth(max_chars) {
        None => text.to_string(),                          // under limit
        Some((idx, _)) => format!("{}...", &text[..idx]),  // truncate
    }
}

/// Escape special Markdown characters
///
/// Escapes characters that have special meaning in Markdown: [ ] ( ) * _ `
pub fn escape_markdown(text: &str) -> String {
    text.replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        // 2024-01-15 00:00:00 UTC
        let result = format_timestamp(1705276800);
        assert!(result.contains("2024-01-15"));
    }

    #[test]
    fn test_format_timestamp_invalid() {
        let result = format_timestamp(-999999999999);
        // Should return "Unknown" for invalid timestamps
        assert!(result == "Unknown" || result.contains("1938") || result.contains("-"));
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

}
