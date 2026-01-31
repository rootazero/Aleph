//! Text formatting policies
//!
//! Configurable parameters for text truncation and formatting
//! across different contexts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Policy for text truncation and formatting
///
/// Controls truncation lengths for different content types to ensure
/// consistent behavior across the application.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TextFormatPolicy {
    /// Default text truncation length
    /// Default: 200
    #[serde(default = "default_text_truncate_length")]
    pub default_truncate_length: u64,

    /// Search snippet truncation length
    /// Default: 300
    #[serde(default = "default_search_snippet_length")]
    pub search_snippet_length: u64,

    /// MCP tool result truncation length
    /// Default: 2000
    #[serde(default = "default_mcp_result_length")]
    pub mcp_result_length: u64,

    /// System prompt truncation length
    /// Default: 5000
    #[serde(default = "default_system_prompt_length")]
    pub system_prompt_length: u64,

    /// User message truncation length
    /// Default: 8000
    #[serde(default = "default_user_message_length")]
    pub user_message_length: u64,

    /// Truncation suffix to append when content is truncated
    /// Default: "..."
    #[serde(default = "default_truncation_suffix")]
    pub truncation_suffix: String,
}

impl Default for TextFormatPolicy {
    fn default() -> Self {
        Self {
            default_truncate_length: default_text_truncate_length(),
            search_snippet_length: default_search_snippet_length(),
            mcp_result_length: default_mcp_result_length(),
            system_prompt_length: default_system_prompt_length(),
            user_message_length: default_user_message_length(),
            truncation_suffix: default_truncation_suffix(),
        }
    }
}

fn default_text_truncate_length() -> u64 {
    200
}

fn default_search_snippet_length() -> u64 {
    300
}

fn default_mcp_result_length() -> u64 {
    2000
}

fn default_system_prompt_length() -> u64 {
    5000
}

fn default_user_message_length() -> u64 {
    8000
}

fn default_truncation_suffix() -> String {
    "...".to_string()
}

impl TextFormatPolicy {
    /// Truncate text to the default length
    pub fn truncate_default(&self, text: &str) -> String {
        self.truncate_to_length(text, self.default_truncate_length as usize)
    }

    /// Truncate text to the search snippet length
    pub fn truncate_search_snippet(&self, text: &str) -> String {
        self.truncate_to_length(text, self.search_snippet_length as usize)
    }

    /// Truncate text to the MCP result length
    pub fn truncate_mcp_result(&self, text: &str) -> String {
        self.truncate_to_length(text, self.mcp_result_length as usize)
    }

    /// Truncate text to a specific length, appending suffix if truncated
    pub fn truncate_to_length(&self, text: &str, max_length: usize) -> String {
        if text.chars().count() <= max_length {
            text.to_string()
        } else {
            let suffix_len = self.truncation_suffix.chars().count();
            let take_len = max_length.saturating_sub(suffix_len);
            let truncated: String = text.chars().take(take_len).collect();
            format!("{}{}", truncated, self.truncation_suffix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let policy = TextFormatPolicy::default();
        assert_eq!(policy.default_truncate_length, 200);
        assert_eq!(policy.search_snippet_length, 300);
        assert_eq!(policy.mcp_result_length, 2000);
        assert_eq!(policy.truncation_suffix, "...");
    }

    #[test]
    fn test_truncation_no_change() {
        let policy = TextFormatPolicy::default();
        let short_text = "Hello world";
        assert_eq!(policy.truncate_default(short_text), short_text);
    }

    #[test]
    fn test_truncation_with_suffix() {
        let mut policy = TextFormatPolicy::default();
        policy.default_truncate_length = 10;
        let long_text = "This is a very long text that should be truncated";
        let result = policy.truncate_default(long_text);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 10);
    }

    #[test]
    fn test_partial_deserialization() {
        let toml = r#"
            default_truncate_length = 500
            truncation_suffix = "…"
        "#;
        let policy: TextFormatPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.default_truncate_length, 500);
        assert_eq!(policy.truncation_suffix, "…");
        // Defaults for unspecified
        assert_eq!(policy.search_snippet_length, 300);
        assert_eq!(policy.mcp_result_length, 2000);
    }
}
