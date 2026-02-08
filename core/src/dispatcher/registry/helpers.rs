//! Helper Functions for ToolRegistry
//!
//! Utility functions used across the registry module.

/// Extract command name from regex pattern
///
/// Examples:
/// - "^/translate" -> "translate"
/// - "^/(?i)code" -> "code"
/// - "^/draw\\s+" -> "draw"
pub fn extract_command_name(pattern: &str) -> String {
    // Remove common regex prefixes and patterns sequentially
    let mut cleaned = pattern;
    cleaned = cleaned.strip_prefix("^/").unwrap_or(cleaned);
    cleaned = cleaned.strip_prefix("(?i)").unwrap_or(cleaned);
    cleaned = cleaned.strip_prefix('(').unwrap_or(cleaned);

    // Take characters until we hit a regex special character
    cleaned
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// Truncate description to max length, adding ellipsis
pub fn truncate_description(s: &str, max_len: usize) -> String {
    let s = s.trim();
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_command_name() {
        assert_eq!(extract_command_name("^/translate"), "translate");
        assert_eq!(extract_command_name("^/(?i)code"), "code");
        assert_eq!(extract_command_name("^/draw\\s+"), "draw");
        assert_eq!(extract_command_name("^/my-command"), "my-command");
        assert_eq!(extract_command_name("^/test_cmd"), "test_cmd");
    }

    #[test]
    fn test_truncate_description() {
        assert_eq!(truncate_description("Short", 100), "Short");
        assert_eq!(
            truncate_description(
                "This is a very long description that should be truncated",
                20
            ),
            "This is a very lo..."
        );
    }
}
