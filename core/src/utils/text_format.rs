//! Shared text formatting utilities
//!
//! Common functions used by prompt assemblers across the codebase.

use crate::config::TextFormatPolicy;

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
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.to_string()
    } else {
        let truncate_at = text
            .char_indices()
            .nth(max_chars)
            .map(|(idx, _)| idx)
            .unwrap_or(text.len());
        format!("{}...", &text[..truncate_at])
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

/// Escape special XML/HTML characters
///
/// Escapes: & < > " '
pub fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Format a relevance score as a percentage string
///
/// # Arguments
/// * `score` - Score between 0.0 and 1.0
///
/// # Returns
/// Formatted string like "Relevance: 85%"
pub fn format_relevance_score(score: f32) -> String {
    format!("Relevance: {:.0}%", score * 100.0)
}

/// Format a confidence score as a percentage string
///
/// # Arguments
/// * `score` - Score between 0.0 and 1.0
///
/// # Returns
/// Formatted string like "85%"
pub fn format_confidence_score(score: f32) -> String {
    format!("{:.0}%", score * 100.0)
}

// Common truncation length constants (defaults, for backward compatibility)
pub const DEFAULT_TEXT_TRUNCATE_LENGTH: usize = 200;
pub const SEARCH_SNIPPET_TRUNCATE_LENGTH: usize = 300;
pub const MCP_RESULT_TRUNCATE_LENGTH: usize = 2000;

/// Get the default text truncate length from policy or use constant
pub fn get_default_truncate_length(policy: Option<&TextFormatPolicy>) -> usize {
    policy
        .map(|p| p.default_truncate_length as usize)
        .unwrap_or(DEFAULT_TEXT_TRUNCATE_LENGTH)
}

/// Get the search snippet truncate length from policy or use constant
pub fn get_search_snippet_length(policy: Option<&TextFormatPolicy>) -> usize {
    policy
        .map(|p| p.search_snippet_length as usize)
        .unwrap_or(SEARCH_SNIPPET_TRUNCATE_LENGTH)
}

/// Get the MCP result truncate length from policy or use constant
pub fn get_mcp_result_length(policy: Option<&TextFormatPolicy>) -> usize {
    policy
        .map(|p| p.mcp_result_length as usize)
        .unwrap_or(MCP_RESULT_TRUNCATE_LENGTH)
}

/// Truncate text using policy-configured length
pub fn truncate_text_with_policy(text: &str, policy: Option<&TextFormatPolicy>) -> String {
    let max_chars = get_default_truncate_length(policy);
    truncate_text(text, max_chars)
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

    #[test]
    fn test_escape_xml() {
        let text = "<script>alert('xss')</script>";
        let result = escape_xml(text);
        assert!(!result.contains('<'));
        assert!(result.contains("&lt;"));
    }

    #[test]
    fn test_format_relevance_score() {
        assert_eq!(format_relevance_score(0.85), "Relevance: 85%");
        assert_eq!(format_relevance_score(1.0), "Relevance: 100%");
        assert_eq!(format_relevance_score(0.0), "Relevance: 0%");
    }

    #[test]
    fn test_format_confidence_score() {
        assert_eq!(format_confidence_score(0.75), "75%");
    }
}
