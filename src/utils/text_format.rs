//! Shared text formatting utilities
//!
//! Common functions used by prompt assemblers across the codebase.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
